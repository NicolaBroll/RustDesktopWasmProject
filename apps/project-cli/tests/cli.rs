//! End-to-end tests of `project-cli` on temporary git repos.

use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Deterministic environment for git, both for the test repos and for the CLI.
/// `GIT_CONFIG_GLOBAL` exists from git 2.32: with older git `HOME` and `XDG_CONFIG_HOME` keep
/// the user's configuration out.
const GIT_ENV: &[(&str, &str)] = &[
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("HOME", "/dev/null"),
    ("XDG_CONFIG_HOME", "/dev/null"),
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

/// Repo with `commits` commits on `main` (each adds a line to `a.txt`) in `dir`.
fn init_repo(dir: &Path, commits: usize) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["-c", "init.defaultBranch=main", "init", "--quiet"]);
    for n in 1..=commits {
        let text = (1..=n).map(|i| format!("line {i}\n")).collect::<String>();
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
    std::fs::write(repo.path().join("new.txt"), "?\n").unwrap();
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
fn run_commit_show() {
    let repo = temp_repo(2);
    let output = cli()
        .args(["--repo", path_str(repo.path()), "run"])
        .args(["--cmd", r#"commit.show={"id":"HEAD"}"#])
        .args(["--cmd", r#"commit.show={"id":"HEAD~1"}"#])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let lines = json_lines(&output.stdout);
    assert_eq!(lines[0]["commit"]["summary"], "commit 2");
    assert_eq!(lines[0]["compared_to"], lines[1]["commit"]["id"]);
    assert_eq!(lines[0]["files"][0]["path"], "a.txt");
    assert_eq!(lines[0]["files"][0]["change"], "Modified");
    assert_eq!(lines[1]["compared_to"], Value::Null);
    assert_eq!(lines[1]["files"][0]["change"], "Added");
}

#[test]
fn run_diff_commands() {
    let repo = temp_repo(2);
    std::fs::write(repo.path().join("a.txt"), "line 1\nline 2\nline 3\n").unwrap();
    let output = cli()
        .args(["--repo", path_str(repo.path()), "run"])
        .args(["--cmd", "diff.worktree", "--cmd", "diff.index"])
        .args(["--cmd", r#"diff.commit={"id":"HEAD","path":"a.txt"}"#])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let lines = json_lines(&output.stdout);
    let file = &lines[0]["files"][0];
    assert_eq!(file["new_path"], "a.txt");
    let added = &file["hunks"][0]["lines"][2];
    assert_eq!(added["kind"], "Added");
    assert_eq!(added["content"], "line 3");
    assert_eq!(added["new_line"], 3);
    assert_eq!(lines[1], json!({ "files": [] }));
    let hunk = &lines[2]["files"][0]["hunks"][0];
    assert_eq!(hunk["old_start"], 1);
    assert_eq!(hunk["new_count"], 2);
}

#[test]
fn run_stage_and_unstage() {
    let repo = temp_repo(1);
    std::fs::write(repo.path().join("a.txt"), "changed\n").unwrap();
    std::fs::write(repo.path().join("b.txt"), "new\n").unwrap();
    let output = cli()
        .args(["--repo", path_str(repo.path()), "run"])
        .args(["--cmd", r#"index.stage={"paths":["a.txt","b.txt"]}"#])
        .args(["--cmd", r#"index.unstage={"paths":["a.txt"]}"#])
        .args(["--cmd", "diff.index"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let lines = json_lines(&output.stdout);
    // Every write returns the new state and the possible index backup (here nothing
    // at risk).
    assert_eq!(lines[0]["backup"], Value::Null);
    let entries = &lines[0]["status"]["entries"];
    assert_eq!(entries[0]["path"], "a.txt");
    assert_eq!(entries[0]["index"], "Modified");
    assert_eq!(entries[1]["path"], "b.txt");
    assert_eq!(entries[1]["index"], "Added");
    let entries = &lines[1]["status"]["entries"];
    assert_eq!(entries[0]["index"], "Unmodified");
    assert_eq!(entries[0]["worktree"], "Modified");
    let files = lines[2]["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["new_path"], "b.txt");
    // The worktree does not change.
    let a = std::fs::read_to_string(repo.path().join("a.txt")).unwrap();
    assert_eq!(a, "changed\n");

    let output = cli()
        .args(["--repo", path_str(repo.path()), "run"])
        .args(["--cmd", r#"index.stage={"paths":[]}"#])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "an empty list does not mean everything"
    );
}

#[test]
fn run_commit_create() {
    let repo = temp_repo(1);
    std::fs::write(repo.path().join("b.txt"), "new\n").unwrap();
    let output = cli()
        .args(["--repo", path_str(repo.path()), "run"])
        .args(["--cmd", r#"index.stage={"paths":["b.txt"]}"#])
        .args([
            "--cmd",
            r#"commit.create={"message":"Add b\n\nWith body."}"#,
        ])
        .args(["--cmd", r#"repo.log={"limit":1}"#])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let lines = json_lines(&output.stdout);
    let out = &lines[1];
    assert_eq!(out["commit"]["summary"], "Add b");
    assert_eq!(out["commit"]["body"], "With body.");
    assert_eq!(out["branch"], "refs/heads/main");
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["status"]["entries"], json!([]));
    assert_eq!(out["commit"]["parents"][0], out["previous"]);
    assert_eq!(lines[2]["commits"][0], out["commit"]);

    // Nothing to commit: an error, and the CLI exits with an error code.
    let output = cli()
        .args(["--repo", path_str(repo.path()), "run"])
        .args(["--cmd", r#"commit.create={"message":"again"}"#])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nothing to commit"), "{stderr}");

    // `amend`: the rewritten commit stays in a backup ref.
    let output = cli()
        .args(["--repo", path_str(repo.path()), "run"])
        .args([
            "--cmd",
            r#"commit.create={"message":"Add b, fixed","amend":true}"#,
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let out = &json_lines(&output.stdout)[0];
    assert!(
        out["backup"]
            .as_str()
            .unwrap()
            .starts_with("refs/project/backup/amend-")
    );
    assert_eq!(out["commit"]["summary"], "Add b, fixed");
}

#[test]
fn run_undo_and_redo_in_one_invocation() {
    let repo = temp_repo(1);
    std::fs::write(repo.path().join("b.txt"), "new\n").unwrap();
    let output = cli()
        .args(["--repo", path_str(repo.path()), "run"])
        .args(["--cmd", r#"index.stage={"paths":["b.txt"]}"#])
        .args(["--cmd", r#"commit.create={"message":"Add b"}"#])
        .args(["--cmd", "edit.undo"])
        .args(["--cmd", "edit.history"])
        .args(["--cmd", "edit.redo"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let lines = json_lines(&output.stdout);
    let commit = &lines[1];
    let undo = &lines[2];
    assert_eq!(undo["undone"]["command"], "commit.create");
    assert_eq!(
        undo["status"]["head"]["Branch"]["target"],
        commit["previous"]
    );
    assert_eq!(undo["status"]["entries"][0]["index"], "Added");
    assert_eq!(undo["can_redo"], true);
    assert!(
        undo["backups"][0]
            .as_str()
            .unwrap()
            .starts_with("refs/project/backup/undo-")
    );
    assert_eq!(lines[3]["undone"][0]["id"], undo["undone"]["id"]);
    let redo = &lines[4];
    assert_eq!(
        redo["status"]["head"]["Branch"]["target"],
        commit["commit"]["id"]
    );
    assert_eq!(redo["can_undo"], true);

    // The journal does not survive between two invocations: nothing to undo.
    let output = cli()
        .args(["--repo", path_str(repo.path()), "run", "--cmd", "edit.undo"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nothing to undo"), "{stderr}");
}

#[test]
fn run_discard_needs_confirm_and_undo_restores() {
    let repo = temp_repo(1);
    let file = repo.path().join("a.txt");
    std::fs::write(&file, "do not lose\n").unwrap();
    // Without `confirm`: an error and nothing changes.
    let output = cli()
        .args(["--repo", path_str(repo.path()), "run"])
        .args(["--cmd", r#"worktree.discard={"paths":["a.txt"]}"#])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("\"confirm\": true"), "{stderr}");
    assert_eq!(std::fs::read(&file).unwrap(), b"do not lose\n");

    let output = cli()
        .args(["--repo", path_str(repo.path()), "run"])
        .args([
            "--cmd",
            r#"worktree.discard={"paths":["a.txt"],"confirm":true}"#,
        ])
        .args(["--cmd", "edit.undo"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let lines = json_lines(&output.stdout);
    assert_eq!(lines[0]["files"][0]["path"], "a.txt");
    assert!(
        lines[0]["backup"]
            .as_str()
            .unwrap()
            .starts_with("refs/project/backup/discard-")
    );
    assert_eq!(lines[1]["undone"]["command"], "worktree.discard");
    assert_eq!(std::fs::read(&file).unwrap(), b"do not lose\n");
}

#[test]
fn run_stage_and_unstage_lines() {
    let repo = temp_repo(3);
    std::fs::write(repo.path().join("a.txt"), "line 1\nTWO\nline 3\nfour\n").unwrap();
    let run = |cmd: String| {
        let output = cli()
            .args(["--repo", path_str(repo.path()), "run", "--cmd", &cmd])
            .output()
            .unwrap();
        (output.status.success(), json_lines(&output.stdout), output)
    };
    // First the diff, which gives the line numbers and the digest.
    let (ok, lines, output) = run(r#"diff.worktree={"path":"a.txt"}"#.to_owned());
    assert!(ok, "{output:?}");
    let digest = lines[0]["files"][0]["digest"].as_str().unwrap().to_owned();
    let params = json!({ "path": "a.txt", "lines": [{ "new": 4 }], "digest": digest });
    let (ok, lines, output) = run(format!("index.stage_lines={params}"));
    assert!(ok, "{output:?}");
    assert_eq!(lines[0]["backup"], Value::Null);
    assert_eq!(lines[0]["status"]["entries"][0]["index"], "Modified");
    assert_eq!(lines[0]["status"]["entries"][0]["worktree"], "Modified");
    // The worktree does not change.
    let a = std::fs::read_to_string(repo.path().join("a.txt")).unwrap();
    assert_eq!(a, "line 1\nTWO\nline 3\nfour\n");

    // The same digest, after the write, is stale.
    let (ok, _, output) = run(format!("index.stage_lines={params}"));
    assert!(!ok, "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("reload"), "{stderr}");

    let (ok, lines, output) = run(r#"diff.index={"path":"a.txt"}"#.to_owned());
    assert!(ok, "{output:?}");
    let digest = lines[0]["files"][0]["digest"].as_str().unwrap().to_owned();
    let params = json!({ "path": "a.txt", "lines": [{ "new": 4 }], "digest": digest });
    let (ok, lines, output) = run(format!("index.unstage_lines={params}"));
    assert!(ok, "{output:?}");
    // `MM`: the staged version was only in the index.
    assert!(
        lines[0]["backup"]
            .as_str()
            .unwrap()
            .starts_with("refs/project/backup/")
    );
    assert_eq!(lines[0]["status"]["entries"][0]["index"], "Unmodified");
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
    assert!(!output.status.success(), "no repository open");
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
    assert_eq!(
        ids,
        [
            "branch.checkout",
            "branch.create",
            "branch.delete",
            "branch.rename",
            "branch.reset",
            "branch.reset_preview",
            "branch.set_upstream",
            "cherry_pick.abort",
            "cherry_pick.continue",
            "commit.checkout",
            "commit.cherry_pick",
            "commit.create",
            "commit.revert",
            "commit.show",
            "conflict.resolve",
            "conflict.show",
            "diff.commit",
            "diff.index",
            "diff.range",
            "diff.worktree",
            "edit.history",
            "edit.redo",
            "edit.undo",
            "file.blame",
            "file.log",
            "index.stage",
            "index.stage_lines",
            "index.unstage",
            "index.unstage_lines",
            "merge.abort",
            "merge.continue",
            "merge.start",
            "rebase.abort",
            "rebase.continue",
            "rebase.plan",
            "rebase.start",
            "remote.add",
            "remote.delete_branch",
            "remote.delete_tag",
            "remote.fetch",
            "remote.list",
            "remote.pull",
            "remote.push",
            "remote.push_tag",
            "remote.remove",
            "repo.clone",
            "repo.init",
            "repo.log",
            "repo.open",
            "repo.operation",
            "repo.optimize",
            "repo.refs",
            "repo.search",
            "repo.status",
            "revert.abort",
            "revert.continue",
            "stash.apply",
            "stash.drop",
            "stash.list",
            "stash.pop",
            "stash.push",
            "tag.create",
            "tag.delete",
            "worktree.clean",
            "worktree.discard"
        ]
    );
    // Commands that modify the repo are marked.
    let marked: Vec<&str> = text
        .lines()
        .filter(|l| l.ends_with("[writes]"))
        .map(|l| l.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(
        marked,
        [
            "branch.checkout",
            "branch.create",
            "branch.delete",
            "branch.rename",
            "branch.reset",
            "branch.set_upstream",
            "cherry_pick.abort",
            "cherry_pick.continue",
            "commit.checkout",
            "commit.cherry_pick",
            "commit.create",
            "commit.revert",
            "conflict.resolve",
            "edit.redo",
            "edit.undo",
            "index.stage",
            "index.stage_lines",
            "index.unstage",
            "index.unstage_lines",
            "merge.abort",
            "merge.continue",
            "merge.start",
            "rebase.abort",
            "rebase.continue",
            "rebase.start",
            "remote.add",
            "remote.delete_branch",
            "remote.delete_tag",
            "remote.fetch",
            "remote.pull",
            "remote.push",
            "remote.push_tag",
            "remote.remove",
            "repo.clone",
            "repo.init",
            "repo.optimize",
            "revert.abort",
            "revert.continue",
            "stash.apply",
            "stash.drop",
            "stash.pop",
            "stash.push",
            "tag.create",
            "tag.delete",
            "worktree.clean",
            "worktree.discard"
        ]
    );

    let output = cli()
        .args(["commands", "--search", "commit history"])
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
        "repo.log={\"rev\":\"no-such-rev\"}\n# comment\n\nrepo.log={\"limit\":1}\n",
    );
    assert!(output.status.success(), "{output:?}");
    let lines = json_lines(&output.stdout);
    assert_eq!(lines.len(), 2);
    assert!(lines[0]["error"].as_str().unwrap().contains("no-such-rev"));
    assert_eq!(lines[1]["ok"]["commits"][0]["summary"], "commit 2");
}

/// Responses of the MCP server to `requests`, one per request with an id.
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

/// The text of a tool's result, and whether it is an error.
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
    assert!(stderr.contains("repository open"), "{stderr}");

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
    // The root is any folder inside a repo: git must not climb up to it.
    let outer = temp_repo(1);
    let root = outer.path().join("root");
    init_repo(&root.join("interno"), 1);
    std::fs::create_dir_all(root.join("folder")).unwrap();

    let (responses, stderr) = mcp(
        &root,
        &[
            tool_call(1, "execute_command", json!({ "id": "repo.status" })),
            tool_call(2, "open_repo", json!({ "path": "." })),
            tool_call(3, "open_repo", json!({ "path": "folder" })),
            tool_call(4, "open_repo", json!({ "path": ".." })),
            tool_call(5, "open_repo", json!({ "path": "interno" })),
            tool_call(6, "execute_command", json!({ "id": "repo.refs" })),
        ],
    );
    assert!(stderr.contains("no repository open"), "{stderr}");
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

#[test]
fn run_fetch_from_a_local_remote() {
    // `source` has two commits and the remote `origin` (bare); `work` has the same remote and nothing.
    let source = temp_repo(2);
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare"]);
    git(
        source.path(),
        &["remote", "add", "origin", path_str(remote.path())],
    );
    git(source.path(), &["push", "--quiet", "origin", "main"]);
    let work = temp_repo(0);
    git(
        work.path(),
        &["remote", "add", "origin", path_str(remote.path())],
    );

    let output = cli()
        .args(["--repo", path_str(work.path()), "run"])
        .args([
            "--cmd",
            "remote.fetch",
            "--cmd",
            r#"remote.fetch={"remote":"origin"}"#,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = json_lines(&output.stdout);
    assert_eq!(lines[0]["remote"], "origin");
    assert_eq!(lines[0]["updated"][0]["name"], "refs/remotes/origin/main");
    assert_eq!(lines[0]["updated"][0]["before"], Value::Null);
    assert_eq!(lines[1]["updated"], json!([]));

    let output = cli()
        .args(["--repo", path_str(work.path()), "run"])
        .args(["--cmd", r#"remote.fetch={"remote":"nowhere"}"#])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("there is no remote `nowhere`"), "{stderr}");
}

#[test]
fn run_push_and_pull_with_a_local_remote() {
    // `source` publishes (first push: the upstream is set), `work` downloads with the pull.
    let source = temp_repo(1);
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare"]);
    git(
        source.path(),
        &["remote", "add", "origin", path_str(remote.path())],
    );
    let run = |repo: &Path, cmds: &[&str]| {
        let mut cmd = cli();
        cmd.args(["--repo", path_str(repo), "run"]);
        for c in cmds {
            cmd.args(["--cmd", c]);
        }
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        json_lines(&output.stdout)
    };
    let lines = run(source.path(), &["remote.push"]);
    assert_eq!(lines[0]["kind"], "new");
    assert_eq!(lines[0]["set_upstream"], true);

    let work = temp_repo(0);
    git(
        work.path(),
        &["remote", "add", "origin", path_str(remote.path())],
    );
    git(work.path(), &["fetch", "--quiet", "origin"]);
    git(work.path(), &["reset", "--quiet", "--hard", "origin/main"]);
    git(
        work.path(),
        &["branch", "--quiet", "--set-upstream-to=origin/main", "main"],
    );
    git(
        source.path(),
        &["commit", "--quiet", "--allow-empty", "-m", "second"],
    );
    let lines = run(source.path(), &[r#"remote.push={"remote":"origin"}"#]);
    assert_eq!(lines[0]["kind"], "fast_forward");

    let lines = run(work.path(), &["remote.pull", "edit.undo", "remote.pull"]);
    assert_eq!(lines[0]["strategy"], "fast_forward");
    assert_eq!(lines[1]["undone"]["command"], "remote.pull");
    assert_eq!(lines[2]["after"], lines[0]["after"]);
}
