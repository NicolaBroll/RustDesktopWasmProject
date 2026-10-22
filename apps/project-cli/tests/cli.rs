//! Test end-to-end di `project-cli` su repo git temporanei.

use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Ambiente deterministico per git, sia per i repo dei test sia per la CLI.
const GIT_ENV: &[(&str, &str)] = &[
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_AUTHOR_NAME", "Ada"),
    ("GIT_AUTHOR_EMAIL", "ada@example.com"),
    ("GIT_COMMITTER_NAME", "Bob"),
    ("GIT_COMMITTER_EMAIL", "bob@example.com"),
    ("GIT_AUTHOR_DATE", "1700000000 +0200"),
    ("GIT_COMMITTER_DATE", "1700000000 +0200"),
];

fn cli() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_project-cli"));
    cmd.envs(GIT_ENV.iter().copied());
    cmd
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(GIT_ENV.iter().copied())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// Repo con `commits` commit su `main` (ognuno aggiunge una riga a `a.txt`) in `dir`.
fn init_repo(dir: &Path, commits: usize) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["-c", "init.defaultBranch=main", "init", "--quiet"]);
    for n in 1..=commits {
        let text = (1..=n).map(|i| format!("riga {i}\n")).collect::<String>();
        std::fs::write(dir.join("a.txt"), text).unwrap();
        git(dir, &["add", "a.txt"]);
        git(dir, &["commit", "--quiet", "-m", &format!("commit {n}")]);
    }
}

fn temp_repo(commits: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path(), commits);
    dir
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn json_lines(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8(stdout.to_vec())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn with_stdin(mut cmd: Command, input: &str) -> Output {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn run_with_repo_reads_status_and_log() {
    let repo = temp_repo(3);
    std::fs::write(repo.path().join("nuovo.txt"), "?\n").unwrap();
    let output = cli()
        .args(["--repo", path_str(repo.path()), "run"])
        .args(["--cmd", "repo.status", "--cmd", r#"repo.log={"limit":2}"#])
        .args(["--cmd", r#"repo.log={"skip":2}"#])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let lines = json_lines(&output.stdout);
    assert_eq!(lines.len(), 3);

    assert_eq!(lines[0]["head"]["Branch"]["name"], "main");
    assert_eq!(lines[0]["entries"].as_array().unwrap().len(), 1);

    let summaries = |v: &Value| -> Vec<String> {
        v["commits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["summary"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(summaries(&lines[1]), ["commit 3", "commit 2"]);
    assert_eq!(lines[1]["has_more"], true);
    assert_eq!(summaries(&lines[2]), ["commit 1"]);
    assert_eq!(lines[2]["has_more"], false);
}

#[test]
fn repo_option_goes_before_or_after_the_subcommand() {
    let repo = temp_repo(1);
    let output = cli()
        .args(["run", "--repo", path_str(repo.path()), "--cmd", "repo.refs"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let refs = &json_lines(&output.stdout)[0];
    assert_eq!(refs[0]["name"], "refs/heads/main");
}

#[test]
fn run_without_a_repo_fails() {
    let dir = tempfile::tempdir().unwrap();
    let output = cli()
        .args([
            "--repo",
            path_str(dir.path()),
            "run",
            "--cmd",
            "repo.status",
        ])
        .env("GIT_CEILING_DIRECTORIES", dir.path().parent().unwrap())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--repo"), "{stderr}");

    let output = cli()
        .args(["run", "--cmd", "repo.status"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "nessun repo aperto");
    let output = cli().args(["run", "--cmd", "nope"]).output().unwrap();
    assert!(!output.status.success());
}

#[test]
fn commands_lists_the_catalog() {
    let output = cli().arg("commands").output().unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    let ids: Vec<&str> = text
        .lines()
        .map(|l| l.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(ids, ["repo.log", "repo.open", "repo.refs", "repo.status"]);

    let output = cli()
        .args(["commands", "--search", "cronologia"])
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("repo.log"), "{text}");
    assert_eq!(text.lines().count(), 1);
}

#[test]
fn eval_with_repo_keeps_going_after_errors() {
    let repo = temp_repo(2);
    let mut cmd = cli();
    cmd.args(["--repo", path_str(repo.path()), "eval"]);
    let output = with_stdin(
        cmd,
        "repo.log={\"rev\":\"nessuno\"}\n# commento\n\nrepo.log={\"limit\":1}\n",
    );
    assert!(output.status.success(), "{output:?}");
    let lines = json_lines(&output.stdout);
    assert_eq!(lines.len(), 2);
    assert!(lines[0]["error"].as_str().unwrap().contains("nessuno"));
    assert_eq!(lines[1]["ok"]["commits"][0]["summary"], "commit 2");
}

/// Risposte del server MCP a `requests`, una per richiesta con id.
fn mcp(root: &Path, requests: &[Value]) -> (Vec<Value>, String) {
    let mut cmd = cli();
    cmd.args(["mcp", "--root", path_str(root)]);
    let input: String = requests.iter().map(|r| format!("{r}\n")).collect();
    let output = with_stdin(cmd, &input);
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    (json_lines(&output.stdout), stderr)
}

fn tool_call(id: u32, name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    })
}

/// Il testo del risultato di un tool, e se è un errore.
fn tool_text(response: &Value) -> (String, bool) {
    let result = &response["result"];
    (
        result["content"][0]["text"].as_str().unwrap().to_owned(),
        result["isError"].as_bool().unwrap(),
    )
}

#[test]
fn mcp_opens_the_repo_in_the_root() {
    let repo = temp_repo(2);
    let (responses, stderr) = mcp(
        repo.path(),
        &[
            tool_call(1, "execute_command", json!({ "id": "repo.log" })),
            tool_call(
                2,
                "execute_command",
                json!({ "id": "repo.open", "params": { "path": "/" } }),
            ),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
        ],
    );
    assert!(stderr.contains("repository aperto"), "{stderr}");

    let (text, is_error) = tool_text(&responses[0]);
    assert!(!is_error, "{text}");
    let log: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(log["commits"].as_array().unwrap().len(), 2);

    let (text, is_error) = tool_text(&responses[1]);
    assert!(is_error && text.contains("open_repo"), "{text}");

    let tools: Vec<&str> = responses[2]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(tools, ["list_commands", "execute_command", "open_repo"]);
}

#[test]
fn mcp_stays_inside_the_root() {
    // La radice è una cartella qualunque dentro un repo: git non deve risalire fino a quello.
    let outer = temp_repo(1);
    let root = outer.path().join("radice");
    init_repo(&root.join("interno"), 1);
    std::fs::create_dir_all(root.join("cartella")).unwrap();

    let (responses, stderr) = mcp(
        &root,
        &[
            tool_call(1, "execute_command", json!({ "id": "repo.status" })),
            tool_call(2, "open_repo", json!({ "path": "." })),
            tool_call(3, "open_repo", json!({ "path": "cartella" })),
            tool_call(4, "open_repo", json!({ "path": ".." })),
            tool_call(5, "open_repo", json!({ "path": "interno" })),
            tool_call(6, "execute_command", json!({ "id": "repo.refs" })),
        ],
    );
    assert!(stderr.contains("nessun repository aperto"), "{stderr}");
    for (i, response) in responses[..4].iter().enumerate() {
        let (text, is_error) = tool_text(response);
        assert!(is_error, "richiesta {}: {text}", i + 1);
    }
    let (text, is_error) = tool_text(&responses[4]);
    assert!(!is_error, "{text}");
    let (text, is_error) = tool_text(&responses[5]);
    assert!(!is_error, "{text}");
    assert!(text.contains("refs/heads/main"), "{text}");
}

#[test]
fn mcp_rejects_the_repo_option() {
    let repo = temp_repo(1);
    let output = cli()
        .args(["--repo", path_str(repo.path()), "mcp", "--root", "."])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
}
