//! Test di integrazione di `conflict.show` e `conflict.resolve` (S30) con il `CliBackend`:
//! merge in conflitto → versioni → risoluzione → `repo.operation` pronto → `merge.continue`;
//! digest vecchio, file non in conflitto, indice bloccato, link nel worktree (niente scritto);
//! `delete` per `DeletedByThem`, file assente per `DeletedByUs`; backup leggibile con `git show`;
//! stile diff3 e `conflict-marker-size`; file non UTF-8; nomi con spazi e `-` iniziale; modo
//! eseguibile e permessi; backup che fallisce; scrittura che fallisce dopo il backup.

mod common;

use common::{Repo, repo_with_merge_conflict};
use project_engine::{EngineError, Registry, Session, worktree_digest};
use serde_json::{Value, json};
use std::sync::Arc;

fn open(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn backups(repo: &Repo) -> Vec<String> {
    let out = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/project/backup/conflict-*",
    ]);
    out.lines().map(str::to_owned).collect()
}

fn unmerged(repo: &Repo) -> String {
    repo.git(&["ls-files", "-u"])
}

fn show(r: &Registry, s: &mut Session, path: &str) -> Value {
    run(r, s, "conflict.show", json!({ "path": path }))
}

fn resolve(r: &Registry, s: &mut Session, path: &str, digest: &Value, content: &str) -> Value {
    run(
        r,
        s,
        "conflict.resolve",
        json!({ "path": path, "digest": digest, "content": content }),
    )
}

#[test]
fn show_resolve_and_continue() {
    let repo = repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    assert_eq!(shown["kind"], "BothModified");
    assert_eq!(shown["base"]["text"], "base\n");
    assert_eq!(shown["ours"]["text"], "nostra\n");
    assert_eq!(shown["theirs"]["text"], "loro\n");
    let worktree = std::fs::read(repo.path().join("a.txt")).unwrap();
    assert_eq!(
        shown["worktree"]["text"],
        String::from_utf8(worktree.clone()).unwrap()
    );
    assert!(
        shown["worktree"]["text"]
            .as_str()
            .unwrap()
            .contains("<<<<<<< HEAD\n")
    );
    assert_eq!(shown["marker_size"], 7);
    assert_eq!(shown["digest"], worktree_digest(Some(&worktree)));

    let out = resolve(&r, &mut s, "a.txt", &shown["digest"], "unita\n");
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(
        backup.starts_with("refs/project/backup/conflict-"),
        "{backup}"
    );
    assert_eq!(out["index_backup"], Value::Null);
    assert_eq!(backups(&repo), std::slice::from_ref(&backup));
    // Il backup: il file con i marcatori, senza genitori.
    assert_eq!(
        repo.git(&["show", &format!("{backup}:a.txt")]).into_bytes(),
        worktree.trim_ascii_end()
    );
    assert_eq!(repo.git(&["rev-list", "--count", &backup]), "1");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "unita\n"
    );
    assert_eq!(unmerged(&repo), "");
    assert_eq!(repo.git(&["status", "--porcelain"]), "M  a.txt");

    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["ready"], true);
    run(&r, &mut s, "merge.continue", json!({}));
    assert_eq!(repo.git(&["show", "HEAD:a.txt"]), "unita");
    assert_eq!(repo.git(&["rev-list", "--count", "--merges", "HEAD"]), "1");
}

#[test]
fn stale_digest_writes_nothing() {
    let repo = repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    repo.write("a.txt", "cambiato da fuori\n");
    let err = r
        .run(
            "conflict.resolve",
            &mut s,
            json!({ "path": "a.txt", "digest": shown["digest"], "content": "x\n" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "cambiato da fuori\n"
    );
    assert!(backups(&repo).is_empty());
    assert_ne!(unmerged(&repo), "");
}

#[test]
fn not_in_conflict_is_refused() {
    let repo = repo_with_merge_conflict();
    repo.write("nuovo.txt", "n\n");
    let (r, mut s) = open(&repo);
    let digest = worktree_digest(Some(b"n\n"));
    for path in ["nuovo.txt", "manca.txt"] {
        let err = r
            .run("conflict.show", &mut s, json!({ "path": path }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        let err = r
            .run(
                "conflict.resolve",
                &mut s,
                json!({ "path": path, "digest": digest, "content": "x" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    }
    assert_eq!(
        std::fs::read_to_string(repo.path().join("nuovo.txt")).unwrap(),
        "n\n"
    );
    assert!(backups(&repo).is_empty());
}

#[test]
fn index_locked_writes_nothing() {
    let repo = repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    let lock = repo.path().join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();
    let err = r
        .run(
            "conflict.resolve",
            &mut s,
            json!({ "path": "a.txt", "digest": shown["digest"], "content": "x\n" }),
        )
        .unwrap_err();
    std::fs::remove_file(&lock).unwrap();
    assert!(matches!(err, EngineError::IndexLocked { .. }), "{err}");
    assert!(backups(&repo).is_empty());
    assert_eq!(
        shown["worktree"]["text"],
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn link_in_worktree_is_refused() {
    let repo = repo_with_merge_conflict();
    let file = repo.path().join("a.txt");
    std::fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink("b.txt", &file).unwrap();
    let (r, mut s) = open(&repo);
    let err = r
        .run("conflict.show", &mut s, json!({ "path": "a.txt" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let err = r
        .run(
            "conflict.resolve",
            &mut s,
            json!({ "path": "a.txt", "digest": worktree_digest(None), "content": "x" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(file.symlink_metadata().unwrap().is_symlink());
    assert!(backups(&repo).is_empty());
}

#[test]
fn delete_for_deleted_by_them() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "base\n", "base");
    repo.branch("via");
    repo.checkout("via");
    repo.git(&["rm", "--quiet", "a.txt"]);
    repo.commit("via a.txt");
    repo.checkout("main");
    repo.commit_file("a.txt", "nostra\n", "nostra");
    repo.conflict("via");
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    assert_eq!(shown["kind"], "DeletedByThem");
    assert_eq!(shown["theirs"], Value::Null);
    assert_eq!(shown["ours"]["text"], "nostra\n");
    let out = run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": shown["digest"], "delete": true }),
    );
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(repo.git(&["show", &format!("{backup}:a.txt")]), "nostra");
    assert!(!repo.path().join("a.txt").exists());
    assert_eq!(repo.git(&["status", "--porcelain"]), "D  a.txt");
    assert_eq!(run(&r, &mut s, "repo.operation", json!({}))["ready"], true);
    run(&r, &mut s, "merge.continue", json!({}));
    repo.git_fails(&["cat-file", "-e", "HEAD:a.txt"]);
}

#[test]
fn deleted_by_us_with_missing_file() {
    let repo = Repo::new();
    repo.commit_file("dir/a.txt", "base\n", "base");
    repo.branch("altro");
    repo.checkout("altro");
    repo.commit_file("dir/a.txt", "loro\n", "loro");
    repo.checkout("main");
    repo.git(&["rm", "--quiet", "dir/a.txt"]);
    repo.commit("via");
    repo.conflict("altro");
    std::fs::remove_dir_all(repo.path().join("dir")).unwrap();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "dir/a.txt");
    assert_eq!(shown["kind"], "DeletedByUs");
    assert_eq!(shown["ours"], Value::Null);
    assert_eq!(shown["worktree"], Value::Null);
    assert_eq!(shown["digest"], worktree_digest(None));
    let out = resolve(&r, &mut s, "dir/a.txt", &shown["digest"], "tenuto\n");
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("dir/a.txt")).unwrap(),
        "tenuto\n"
    );
    assert_eq!(repo.git(&["status", "--porcelain"]), "A  dir/a.txt");
}

#[test]
fn diff3_style_and_marker_size() {
    let repo = Repo::new();
    repo.commit_file(".gitattributes", "*.txt conflict-marker-size=10\n", "attr");
    repo.commit_file("a.txt", "base\n", "base");
    repo.diverge("altro", "a.txt", "nostra\n", "loro\n");
    repo.git_fails(&[
        "-c",
        "merge.conflictStyle=diff3",
        "merge",
        "--no-edit",
        "altro",
    ]);
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    assert_eq!(shown["marker_size"], 10);
    let text = shown["worktree"]["text"].as_str().unwrap();
    assert!(text.contains("||||||||||"), "{text}");
    assert!(text.contains("\nbase\n"), "{text}");
    assert_eq!(shown["base"]["text"], "base\n");
}

#[test]
fn non_utf8_is_binary() {
    let repo = Repo::new();
    let write = |bytes: &[u8]| std::fs::write(repo.path().join("l.txt"), bytes).unwrap();
    write(b"caf\xe9\n");
    repo.stage(&["l.txt"]);
    repo.commit("base");
    repo.branch("altro");
    repo.checkout("altro");
    write(b"caf\xe8 loro\n");
    repo.stage(&["l.txt"]);
    repo.commit("loro");
    repo.checkout("main");
    write(b"caf\xe0 nostra\n");
    repo.stage(&["l.txt"]);
    repo.commit("nostra");
    repo.conflict("altro");
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "l.txt");
    for side in ["base", "ours", "theirs", "worktree"] {
        assert_eq!(shown[side]["text"], Value::Null, "{side}");
        assert_eq!(shown[side]["binary"], true, "{side}");
    }
    assert_eq!(shown["base"]["size"], 5);
    let before = std::fs::read(repo.path().join("l.txt")).unwrap();
    let out = resolve(&r, &mut s, "l.txt", &shown["digest"], "caffè\n");
    let backup = out["backup"].as_str().unwrap();
    let saved = repo
        .git_output(&["show", &format!("{backup}:l.txt")])
        .stdout;
    assert_eq!(saved, before);
    assert_eq!(
        std::fs::read(repo.path().join("l.txt")).unwrap(),
        "caffè\n".as_bytes()
    );
}

#[test]
fn special_names() {
    let repo = Repo::new();
    let name = "-a b.txt";
    repo.commit_file(name, "base\n", "base");
    repo.diverge("altro", name, "nostra\n", "loro\n");
    repo.conflict("altro");
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, name);
    assert_eq!(shown["ours"]["text"], "nostra\n");
    resolve(&r, &mut s, name, &shown["digest"], "unita\n");
    assert_eq!(unmerged(&repo), "");
    assert_eq!(
        std::fs::read_to_string(repo.path().join(name)).unwrap(),
        "unita\n"
    );
    let entries = repo.git(&["ls-files", "-s"]);
    assert_eq!(entries.lines().count(), 1, "{entries}");
}

#[cfg(unix)]
#[test]
fn executable_bit_is_kept() {
    use std::os::unix::fs::PermissionsExt;
    let repo = repo_with_merge_conflict();
    let file = repo.path().join("a.txt");
    let mut perms = std::fs::metadata(&file).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&file, perms).unwrap();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    resolve(&r, &mut s, "a.txt", &shown["digest"], "unita\n");
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111, "{mode:o}");
}

/// La cartella in sola lettura: il backup è fatto, la scrittura fallisce, l'errore lo nomina e
/// il file resta com'era.
#[cfg(unix)]
#[test]
fn write_failure_after_backup_names_it() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.commit_file("d/a.txt", "base\n", "base");
    repo.diverge("altro", "d/a.txt", "nostra\n", "loro\n");
    repo.conflict("altro");
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "d/a.txt");
    let dir = repo.path().join("d");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(dir.join("prova"), "").is_ok() {
        // Root: la cartella resta scrivibile.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let err = r
        .run(
            "conflict.resolve",
            &mut s,
            json!({ "path": "d/a.txt", "digest": shown["digest"], "content": "x\n" }),
        )
        .unwrap_err();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let backup = backups(&repo);
    assert_eq!(backup.len(), 1);
    assert!(err.to_string().contains(&backup[0]), "{err}");
    assert_eq!(
        shown["worktree"]["text"],
        std::fs::read_to_string(dir.join("a.txt")).unwrap()
    );
    assert_ne!(unmerged(&repo), "");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}

/// I permessi del file sostituito restano quelli (0600 non diventa leggibile da altri).
#[cfg(unix)]
#[test]
fn permissions_are_kept() {
    use std::os::unix::fs::PermissionsExt;
    let repo = repo_with_merge_conflict();
    let file = repo.path().join("a.txt");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    resolve(&r, &mut s, "a.txt", &shown["digest"], "unita\n");
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
}

/// `DeletedByUs` con il file assente e il lato loro eseguibile: il file nuovo è eseguibile.
#[cfg(unix)]
#[test]
fn missing_file_takes_mode_of_remaining_stage() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.commit_file("a.sh", "base\n", "base");
    repo.branch("altro");
    repo.checkout("altro");
    repo.write("a.sh", "loro\n");
    let file = repo.path().join("a.sh");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    repo.git(&["add", "a.sh"]);
    repo.commit("loro");
    repo.checkout("main");
    repo.git(&["rm", "--quiet", "a.sh"]);
    repo.commit("via");
    repo.conflict("altro");
    std::fs::remove_file(&file).unwrap();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.sh");
    resolve(&r, &mut s, "a.sh", &shown["digest"], "tenuto\n");
    assert_eq!(
        repo.git(&["ls-files", "-s", "a.sh"]).split(' ').next(),
        Some("100755")
    );
}

/// Il ref del backup non si può creare: niente si scrive, il conflitto resta, nessun temporaneo.
#[test]
fn backup_failure_writes_nothing() {
    let repo = repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    let refs = repo.path().join(".git/refs/project");
    std::fs::create_dir_all(&refs).unwrap();
    std::fs::write(refs.join("backup"), "").unwrap();
    r.run(
        "conflict.resolve",
        &mut s,
        json!({ "path": "a.txt", "digest": shown["digest"], "content": "x\n" }),
    )
    .unwrap_err();
    assert_eq!(
        shown["worktree"]["text"],
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap()
    );
    assert_ne!(unmerged(&repo), "");
    let temps = std::fs::read_dir(repo.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".project-conflict-")
        })
        .count();
    assert_eq!(temps, 0);
}
