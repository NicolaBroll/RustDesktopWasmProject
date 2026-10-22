use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_project-cli"))
}

/// Repo git vuoto in una cartella temporanea, senza configurazione globale o di sistema.
fn empty_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new("git")
        .args(["-c", "init.defaultBranch=main", "init", "--quiet"])
        .arg(dir.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap();
    assert!(status.success());
    dir
}

fn open_cmd(path: &Path) -> String {
    format!("repo.open={}", serde_json::json!({ "path": path }))
}

fn json_lines(stdout: Vec<u8>) -> Vec<serde_json::Value> {
    String::from_utf8(stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn run_opens_a_repo_and_reads_it() {
    let repo = empty_repo();
    let output = cli()
        .args(["run", "--cmd", &open_cmd(repo.path())])
        .args(["--cmd", "repo.log", "--cmd", "repo.status"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let lines = json_lines(output.stdout);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["head"]["Branch"]["name"], "main");
    assert_eq!(
        lines[1],
        serde_json::json!({ "commits": [], "has_more": false })
    );
    assert_eq!(lines[2]["entries"], serde_json::json!([]));
}

#[test]
fn run_fails_on_bad_command() {
    let output = cli().args(["run", "--cmd", "nope"]).output().unwrap();
    assert!(!output.status.success());
    let output = cli()
        .args(["run", "--cmd", "repo.status"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "nessun repo aperto");
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
fn eval_keeps_going_after_errors() {
    let repo = empty_repo();
    let mut child = cli()
        .arg("eval")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let input = format!(
        "repo.status\n{}\n# commento\nrepo.refs\n",
        open_cmd(repo.path())
    );
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let lines = json_lines(output.stdout);
    assert_eq!(lines.len(), 3);
    assert!(lines[0]["error"].as_str().unwrap().contains("repo.open"));
    assert_eq!(lines[1]["ok"]["head"]["Branch"]["name"], "main");
    assert_eq!(lines[2]["ok"], serde_json::json!([]));
}
