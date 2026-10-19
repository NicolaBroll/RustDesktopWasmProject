use std::io::Write;
use std::process::{Command, Stdio};

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_project-cli"))
}

#[test]
fn run_writes_document_and_reopens_it() {
    let out = std::env::temp_dir().join(format!("project-cli-{}.json", std::process::id()));
    let status = cli()
        .args(["run", "--cmd", r#"item.add={"text":"latte"}"#, "--cmd"])
        .arg(r#"document.set_title={"title":"Spesa"}"#)
        .arg("--out")
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success());

    let output = cli()
        .args(["run", "--cmd", "document.get", "--in"])
        .arg(&out)
        .output()
        .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["title"], "Spesa");
    assert_eq!(doc["items"][0]["text"], "latte");
    std::fs::remove_file(out).ok();
}

#[test]
fn run_fails_on_bad_command() {
    let output = cli().args(["run", "--cmd", "nope"]).output().unwrap();
    assert!(!output.status.success());
}

#[test]
fn commands_search() {
    let output = cli()
        .args(["commands", "--search", "undo"])
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("edit.undo"), "{text}");
    assert_eq!(text.lines().count(), 1);
}

#[test]
fn eval_keeps_going_after_errors() {
    let mut child = cli()
        .arg("eval")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"item.add={\"text\":\"\"}\nitem.add={\"text\":\"a\"}\n# commento\nedit.undo\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let lines: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0]["error"].is_string());
    assert_eq!(lines[1]["ok"]["index"], 0);
    assert_eq!(lines[2]["ok"]["changed"], true);
}
