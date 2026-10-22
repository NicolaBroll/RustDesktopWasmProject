//! Test di integrazione di `CliBackend::diff` e dei comandi `diff.*` sugli scenari di
//! `common`.

mod common;

use common::{
    Repo, repo_with_changes, repo_with_file_changes, repo_with_history, repo_with_merge_conflict,
    repo_with_special_names,
};
use project_domain::{Change, Diff, FileDiff, LineKind, Oid, RepoPath};
use project_engine::{DiffSpec, EngineError, GitBackend, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn worktree(path: Option<&str>) -> DiffSpec {
    DiffSpec::Worktree {
        path: path.map(RepoPath::from),
    }
}

fn index(path: Option<&str>) -> DiffSpec {
    DiffSpec::Index {
        path: path.map(RepoPath::from),
    }
}

fn commit(rev: impl ToString, path: Option<&str>) -> DiffSpec {
    DiffSpec::Commit {
        rev: rev.to_string(),
        path: path.map(RepoPath::from),
    }
}

/// `(stato, percorso vecchio, percorso nuovo)` di ogni file.
fn summary(diff: &Diff) -> Vec<(Change, String, String)> {
    diff.files
        .iter()
        .map(|f| (f.change, f.old_path.to_string(), f.new_path.to_string()))
        .collect()
}

fn same(change: Change, path: &str) -> (Change, String, String) {
    (change, path.to_owned(), path.to_owned())
}

/// Il contenuto delle righe di un tipo, in ordine.
fn lines(file: &FileDiff, kind: LineKind) -> Vec<String> {
    file.hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == kind)
        .map(|l| l.content.to_string())
        .collect()
}

#[test]
fn worktree_and_index_split_staged_and_unstaged_changes() {
    let repo = repo_with_changes();
    let backend = repo.open();

    // Fuori stage solo `a.txt`: il file non tracciato non compare.
    let unstaged = backend.diff(&worktree(None)).unwrap();
    assert_eq!(summary(&unstaged), [same(Change::Modified, "a.txt")]);
    let a = &unstaged.files[0];
    assert_eq!(lines(a, LineKind::Removed), ["uno"]);
    assert_eq!(lines(a, LineKind::Added), ["modificato"]);
    assert_eq!(a.old_oid, Some(repo.rev(":a.txt")));
    assert_eq!(a.hunks[0].header(), b"@@ -1 +1 @@");

    // In stage: il file nuovo e la rinomina esatta.
    let staged = backend.diff(&index(None)).unwrap();
    assert_eq!(
        summary(&staged),
        [
            same(Change::Added, "nuovo.txt"),
            (
                Change::Renamed,
                "vecchio.txt".to_owned(),
                "rinominato.txt".to_owned()
            ),
        ]
    );
    let added = &staged.files[0];
    assert_eq!(added.new_oid, Some(repo.rev(":nuovo.txt")));
    assert_eq!(lines(added, LineKind::Added), ["in stage"]);
    let renamed = &staged.files[1];
    assert_eq!((renamed.similarity, renamed.hunks.len()), (Some(100), 0));

    // Messo in stage, `a.txt` passa da un diff all'altro.
    repo.stage(&["a.txt"]);
    assert!(backend.diff(&worktree(None)).unwrap().files.is_empty());
    let staged = backend.diff(&index(Some("a.txt"))).unwrap();
    assert_eq!(staged.files, unstaged.files);
}

#[test]
fn diff_reads_do_not_touch_the_index() {
    let repo = repo_with_changes();
    let index_file = repo.path().join(".git/index");
    let before = std::fs::read(&index_file).unwrap();
    // Un file toccato senza cambiare il contenuto: `git diff` normale riscriverebbe l'indice.
    repo.write(
        "rinominato.txt",
        "contenuto abbastanza lungo da riconoscere\n",
    );
    let backend = repo.open();
    backend.diff(&worktree(None)).unwrap();
    backend.diff(&index(None)).unwrap();
    assert_eq!(std::fs::read(&index_file).unwrap(), before);
}

#[test]
fn index_of_a_branch_without_commits_compares_with_the_empty_tree() {
    let repo = Repo::new();
    let backend = repo.open();
    assert_eq!(backend.diff(&index(None)).unwrap(), Diff::default());
    assert_eq!(backend.diff(&worktree(None)).unwrap(), Diff::default());
    assert!(matches!(
        backend.diff(&commit("HEAD", None)),
        Err(EngineError::UnknownRev(_))
    ));

    repo.write("primo.txt", "uno\ndue\n");
    repo.stage(&["primo.txt"]);
    let staged = backend.diff(&index(None)).unwrap();
    assert_eq!(summary(&staged), [same(Change::Added, "primo.txt")]);
    assert_eq!(staged.files[0].hunks[0].header(), b"@@ -0,0 +1,2 @@");
}

#[test]
fn commit_diff_root_modify_and_merge_match_show_commit() {
    let h = repo_with_history();
    let backend = h.repo.open();

    // Commit radice: confronto con l'albero vuoto.
    let root = backend.diff(&commit(h.c1, None)).unwrap();
    assert_eq!(summary(&root), [same(Change::Added, "a.txt")]);
    assert_eq!(lines(&root.files[0], LineKind::Added), ["uno"]);
    assert_eq!(root.files[0].old_oid, Some(Oid::from_bytes([0; Oid::LEN])));

    let c2 = backend.diff(&commit(h.c2, None)).unwrap();
    assert_eq!(lines(&c2.files[0], LineKind::Removed), ["uno"]);
    assert_eq!(lines(&c2.files[0], LineKind::Added), ["due"]);

    // Il merge rispetto al primo genitore: solo `b.txt`, portato da `feature`.
    let merge = backend.diff(&commit("HEAD", None)).unwrap();
    assert_eq!(summary(&merge), [same(Change::Added, "b.txt")]);
    assert_eq!(lines(&merge.files[0], LineKind::Added), ["bb"]);

    // Stessi file, id e modi di `commit.show`, per ogni commit.
    for id in [h.c1, h.c2, h.f1, h.f2, h.m] {
        let details = backend.show_commit(&id.to_string()).unwrap();
        let diff = backend.diff(&commit(id, None)).unwrap();
        assert_eq!(diff.files.len(), details.files.len(), "{id}");
        for (f, changed) in diff.files.iter().zip(&details.files) {
            assert_eq!(f.change, changed.change);
            assert_eq!(&f.new_path, &changed.path);
            assert_eq!(f.old_oid, Some(changed.old_oid));
            assert_eq!(f.new_oid, Some(changed.new_oid));
        }
    }

    // Nome di branch e id abbreviato.
    let short = h.f2.to_string()[..7].to_owned();
    for rev in ["feature", short.as_str()] {
        let diff = backend.diff(&commit(rev, None)).unwrap();
        assert_eq!(lines(&diff.files[0], LineKind::Added), ["bb"], "{rev}");
    }
}

#[test]
fn commit_diff_every_kind_of_change_and_non_utf8_names() {
    let fc = repo_with_file_changes();
    let backend = fc.repo.open();
    let diff = backend.diff(&commit(fc.changes, None)).unwrap();
    let details = backend.show_commit(&fc.changes.to_string()).unwrap();
    let expected: Vec<_> = details
        .files
        .iter()
        .map(|f| (f.change, f.old_path().to_string(), f.path.to_string()))
        .collect();
    assert_eq!(summary(&diff), expected);

    let file = |name: &str| {
        diff.files
            .iter()
            .find(|f| f.new_path.to_str() == Some(name))
            .unwrap()
    };
    let moved = file("spostato.txt");
    assert_eq!(moved.change, Change::Renamed);
    assert_eq!(lines(moved, LineKind::Added), ["quinta riga"]);
    let script = file("script.sh");
    assert_eq!(
        (script.old_mode, script.new_mode),
        (Some(0o100644), Some(0o100755))
    );
    assert!(script.hunks.is_empty());

    // Il nome non UTF-8 resta in byte, anche come filtro.
    #[cfg(unix)]
    {
        let name = RepoPath::from_bytes(common::NON_UTF8_NAME);
        let only = backend
            .diff(&DiffSpec::Commit {
                rev: fc.changes.to_string(),
                path: Some(name.clone()),
            })
            .unwrap();
        assert_eq!(only.files.len(), 1);
        assert_eq!(only.files[0].new_path, name);
        assert_eq!(lines(&only.files[0], LineKind::Added), ["latin"]);
    }

    let empty = backend.diff(&commit(fc.empty, None)).unwrap();
    assert!(empty.files.is_empty());
}

#[test]
fn path_keeps_renames_and_matches_only_that_file() {
    let fc = repo_with_file_changes();
    let backend = fc.repo.open();
    let full = backend.diff(&commit(fc.changes, None)).unwrap();
    let moved = full
        .files
        .iter()
        .find(|f| f.new_path.to_str() == Some("spostato.txt"))
        .unwrap();
    // Chiesta per la destinazione o per l'origine, la rinomina resta tale.
    for path in ["spostato.txt", "sposta.txt"] {
        let only = backend.diff(&commit(fc.changes, Some(path))).unwrap();
        assert_eq!(only.files, std::slice::from_ref(moved), "{path}");
    }
    let only = backend
        .diff(&commit(fc.changes, Some("cartella/con spazio.txt")))
        .unwrap();
    assert_eq!(
        summary(&only),
        [same(Change::Added, "cartella/con spazio.txt")]
    );
    // Una cartella non comprende i suoi file; un file non cambiato dà un diff vuoto.
    for path in ["cartella", "a.txt", "nessuno.txt"] {
        let none = backend.diff(&commit(fc.changes, Some(path))).unwrap();
        assert!(none.files.is_empty(), "{path}");
    }

    // Anche per l'indice.
    let repo = repo_with_changes();
    let backend = repo.open();
    for path in ["rinominato.txt", "vecchio.txt"] {
        let only = backend.diff(&index(Some(path))).unwrap();
        assert_eq!(
            summary(&only),
            [(
                Change::Renamed,
                "vecchio.txt".to_owned(),
                "rinominato.txt".to_owned()
            )],
            "{path}"
        );
    }
}

#[test]
fn paths_with_glob_and_magic_characters_are_literal() {
    let repo = repo_with_special_names();
    let backend = repo.open();
    assert_eq!(backend.diff(&worktree(None)).unwrap().files.len(), 6);
    for name in [
        "file*.txt",
        "file1.txt",
        "a[1].txt",
        "a1.txt",
        ":(top)magia.txt",
        "-trattino.txt",
    ] {
        let only = backend.diff(&worktree(Some(name))).unwrap();
        assert_eq!(summary(&only), [same(Change::Modified, name)], "{name}");
        assert_eq!(
            lines(&only.files[0], LineKind::Added),
            [format!("dopo: {name}")]
        );
    }
    // Pathspec che come glob o magici comprenderebbero dei file: qui nessuno.
    for spec in ["*.txt", "file?.txt", ":(glob)*.txt", ":/file1.txt"] {
        let none = backend.diff(&worktree(Some(spec))).unwrap();
        assert!(none.files.is_empty(), "{spec}");
    }
}

#[test]
fn merge_conflict_files_have_no_hunks() {
    let repo = repo_with_merge_conflict();
    let backend = repo.open();
    for spec in [worktree(None), index(None), worktree(Some("a.txt"))] {
        let diff = backend.diff(&spec).unwrap();
        assert_eq!(
            summary(&diff),
            [same(Change::Unmerged, "a.txt")],
            "{spec:?}"
        );
        assert!(diff.files[0].hunks.is_empty());
    }
}

#[test]
fn bad_revs_are_unknown_and_never_options() {
    let h = repo_with_history();
    let backend = h.repo.open();
    for rev in [
        "nessuno",
        "--all",
        "-p",
        "--output=/tmp/x",
        "",
        "main..feature",
    ] {
        let err = backend.diff(&commit(rev, None)).unwrap_err();
        assert!(
            matches!(err, EngineError::UnknownRev(ref r) if r == rev),
            "{rev}: {err}"
        );
    }
    let tree = h.repo.rev("HEAD^{tree}").to_string();
    let err = backend.diff(&commit(&tree, None)).unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err}");
}

fn run(registry: &Registry, session: &mut Session, id: &str, params: Value) -> Value {
    registry
        .run(id, session, params)
        .unwrap_or_else(|e| panic!("{id}: {e}"))
}

#[test]
fn diff_commands_on_a_real_repo() {
    let fc = repo_with_file_changes();
    fc.repo.write("nuovo.txt", "cambiato fuori stage\n");
    fc.repo.write("cartella/con spazio.txt", "in stage\n");
    fc.repo.stage(&["cartella/con spazio.txt"]);
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(Repo::opener()));
    let err = registry
        .run("diff.worktree", &mut session, json!({}))
        .unwrap_err();
    assert!(matches!(err, EngineError::NoRepo), "{err}");
    run(
        &registry,
        &mut session,
        "repo.open",
        json!({ "path": fc.repo.path_str() }),
    );

    // Anche `script.sh`: eseguibile nell'indice (`update-index --chmod`), non nel worktree.
    let out = run(&registry, &mut session, "diff.worktree", json!({}));
    let files = out["files"].as_array().unwrap();
    let paths: Vec<_> = files.iter().map(|f| f["new_path"].clone()).collect();
    assert_eq!(paths, [json!("nuovo.txt"), json!("script.sh")]);
    assert_eq!(files[0]["change"], "Modified");
    assert_eq!(files[1]["hunks"], json!([]));
    let line = &files[0]["hunks"][0]["lines"][1];
    assert_eq!(line["kind"], "Added");
    assert_eq!(line["content"], "cambiato fuori stage");
    assert_eq!(line["new_line"], 1);

    let out = run(
        &registry,
        &mut session,
        "diff.index",
        json!({ "path": "cartella/con spazio.txt" }),
    );
    assert_eq!(out["files"][0]["change"], "Modified");

    let out = run(
        &registry,
        &mut session,
        "diff.commit",
        json!({ "id": fc.changes.to_string(), "path": "spostato.txt" }),
    );
    assert_eq!(out["files"][0]["change"], "Renamed");
    assert_eq!(out["files"][0]["old_path"], "sposta.txt");
    let diff: Diff = serde_json::from_value(out).unwrap();
    assert_eq!(
        diff,
        fc.repo
            .open()
            .diff(&commit(fc.changes, Some("sposta.txt")))
            .unwrap()
    );

    // In JSON un percorso non UTF-8 è un array di byte, anche come parametro.
    #[cfg(unix)]
    {
        let name = json!(common::NON_UTF8_NAME);
        let out = run(
            &registry,
            &mut session,
            "diff.commit",
            json!({ "id": fc.changes.to_string(), "path": name }),
        );
        assert_eq!(out["files"][0]["new_path"], name);
    }

    let err = registry
        .run("diff.commit", &mut session, json!({ "id": "--all" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    let err = registry
        .run("diff.index", &mut session, json!({ "path": "../fuori" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
}
