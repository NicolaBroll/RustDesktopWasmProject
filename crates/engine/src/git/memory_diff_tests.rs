//! Test di `GitBackend::diff` su `MemoryBackend`.

use super::{DiffSpec, GitBackend, LogQuery, MemoryBackend};
use crate::EngineError;
use project_domain::{
    Change, Diff, EntryKind, FileDiff, LineKind, Oid, RepoPath, StatusEntry, TrackedInfo,
    parse_diff,
};

fn sample() -> MemoryBackend {
    MemoryBackend::sample()
}

fn commit(rev: &str) -> DiffSpec {
    DiffSpec::Commit {
        rev: rev.to_owned(),
        path: None,
    }
}

fn commit_path(rev: &str, path: &str) -> DiffSpec {
    DiffSpec::Commit {
        rev: rev.to_owned(),
        path: Some(RepoPath::from(path)),
    }
}

fn worktree() -> DiffSpec {
    DiffSpec::Worktree { path: None }
}

fn index() -> DiffSpec {
    DiffSpec::Index { path: None }
}

/// `(stato, percorso vecchio, percorso nuovo)` di ogni file.
fn summary(diff: &Diff) -> Vec<(Change, String, String)> {
    diff.files
        .iter()
        .map(|f| (f.change, f.old_path.to_string(), f.new_path.to_string()))
        .collect()
}

fn file<'a>(diff: &'a Diff, path: &str) -> &'a FileDiff {
    diff.files
        .iter()
        .find(|f| f.new_path.to_str() == Some(path))
        .unwrap_or_else(|| panic!("manca {path}"))
}

/// Il diff riscritto nel formato di git e riletto dal parser è identico: i campi sono quelli
/// che darebbe `git diff`.
fn assert_round_trip(diff: &Diff) {
    assert_eq!(&parse_diff(&diff.to_patch()).unwrap(), diff);
}

#[test]
fn ogni_commit_ha_i_file_di_commit_show_con_hunk() {
    let repo = sample();
    let zero = Oid::from_bytes([0; Oid::LEN]);
    for c in repo.log(&LogQuery::all()).unwrap() {
        let rev = c.id.to_string();
        let details = repo.show_commit(&rev).unwrap();
        let diff = repo.diff(&commit(&rev)).unwrap();
        let expected: Vec<_> = details
            .files
            .iter()
            .map(|f| (f.change, f.old_path().to_string(), f.path.to_string()))
            .collect();
        assert_eq!(summary(&diff), expected, "{}", c.summary);
        for (f, changed) in diff.files.iter().zip(&details.files) {
            assert!(!f.binary, "{}", f.new_path);
            assert_eq!(f.similarity, changed.score);
            assert_eq!(f.old_oid, Some(changed.old_oid));
            assert_eq!(f.new_oid, Some(changed.new_oid));
            // Solo `.gitkeep` è vuoto: tutti gli altri file hanno righe.
            let empty = f.new_path.to_str() == Some(".gitkeep");
            assert_eq!(f.hunks.is_empty(), empty, "{}", f.new_path);
            match f.change {
                Change::Added => {
                    assert_eq!((f.old_mode, f.new_mode), (None, Some(0o100644)));
                    assert_eq!(f.old_oid, Some(zero));
                }
                Change::Deleted => {
                    assert_eq!((f.old_mode, f.new_mode), (Some(0o100644), None));
                    assert_eq!(f.new_oid, Some(zero));
                }
                _ => assert_eq!((f.old_mode, f.new_mode), (Some(0o100644), Some(0o100644))),
            }
        }
        assert_round_trip(&diff);
    }
}

#[test]
fn rinomina_con_modifiche_nel_commit_di_head() {
    let repo = sample();
    let diff = repo.diff(&commit("HEAD")).unwrap();
    let renamed = file(&diff, "src/bin/app.rs");
    assert_eq!(renamed.change, Change::Renamed);
    assert_eq!(renamed.old_path.to_str(), Some("src/main.rs"));
    assert_eq!(renamed.similarity, Some(92));
    // Una riga cambiata e una aggiunta, con il contesto intorno.
    assert_eq!(renamed.line_counts(), (2, 1));
    let hunk = &renamed.hunks[0];
    assert_eq!((hunk.old_start, hunk.new_start), (1, 1));
    let removed: Vec<_> = hunk
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Removed)
        .map(|l| (l.old_line, l.content.to_string()))
        .collect();
    assert_eq!(
        removed,
        [(Some(2), "// ultima modifica: commit 3".to_owned())]
    );
    let last = hunk.lines.last().unwrap();
    assert_eq!(last.kind, LineKind::Added);
    assert_eq!(
        last.content.to_str(),
        Some("// riga aggiunta da: commit 12")
    );

    // Chiesto per uno dei due percorsi, resta una rinomina.
    for path in ["src/bin/app.rs", "src/main.rs"] {
        let only = repo.diff(&commit_path("HEAD", path)).unwrap();
        assert_eq!(only.files, std::slice::from_ref(renamed), "{path}");
    }
}

#[test]
fn merge_radice_e_file_vuoto() {
    let repo = sample();
    // Il merge porta il Cargo.toml di `feature/grafo`: il diff è quello del branch, a partire
    // dal Cargo.toml del primo genitore.
    let merge = repo.diff(&commit("v0.1.0")).unwrap();
    let feature = repo.diff(&commit("feature/grafo")).unwrap();
    let cargo = file(&merge, "Cargo.toml");
    assert_eq!(cargo.new_oid, file(&feature, "Cargo.toml").new_oid);
    assert_eq!(cargo.line_counts(), (2, 1));

    let root = repo.log(&LogQuery::all()).unwrap().pop().unwrap();
    let diff = repo.diff(&commit(&root.id.to_string())).unwrap();
    assert_eq!(
        summary(&diff),
        [
            (Change::Added, ".gitignore".into(), ".gitignore".into()),
            (Change::Added, ".gitkeep".into(), ".gitkeep".into()),
        ]
    );
    let gitignore = file(&diff, ".gitignore");
    assert_eq!(gitignore.hunks.len(), 1);
    assert_eq!(gitignore.hunks[0].old_count, 0);
    assert_eq!(gitignore.hunks[0].header(), b"@@ -0,0 +1,4 @@");
    // Un file nuovo vuoto non ha hunk e non è binario, come in git.
    let gitkeep = file(&diff, ".gitkeep");
    assert!(gitkeep.hunks.is_empty() && !gitkeep.binary);

    // Eliminato nel commit 3 (il terzo dal fondo del log).
    let structure = repo.log(&LogQuery::all()).unwrap();
    let structure = &structure[structure.len() - 3];
    assert_eq!(structure.summary, "Struttura del progetto");
    let deleted = repo
        .diff(&commit_path(&structure.id.to_string(), ".gitkeep"))
        .unwrap();
    assert_eq!(
        summary(&deleted),
        [(Change::Deleted, ".gitkeep".into(), ".gitkeep".into())]
    );
    assert!(deleted.files[0].hunks.is_empty());
}

#[test]
fn indice_e_worktree_coerenti_con_lo_stato() {
    let repo = sample();
    let status = repo.status().unwrap();
    let info = |path: &str| match &status
        .entries
        .iter()
        .find(|e| e.path.to_str() == Some(path))
        .unwrap()
        .kind
    {
        EntryKind::Ordinary(info) => info.clone(),
        other => panic!("{other:?}"),
    };

    let staged = repo.diff(&index()).unwrap();
    assert_eq!(
        summary(&staged),
        [
            (Change::Modified, "README.md".into(), "README.md".into()),
            (
                Change::Added,
                "docs/guida.md".into(),
                "docs/guida.md".into()
            ),
        ]
    );
    let readme = file(&staged, "README.md");
    assert_eq!(readme.old_oid, Some(info("README.md").oid_head));
    assert_eq!(readme.new_oid, Some(info("README.md").oid_index));
    assert_eq!(readme.line_counts(), (2, 1));
    // HEAD ha il README del commit 6, l'ultimo che lo ha cambiato.
    assert_eq!(
        readme.hunks[0].lines[1].content.to_str(),
        Some("ultima modifica: commit 6")
    );
    let guide = file(&staged, "docs/guida.md");
    assert_eq!(guide.new_oid, Some(info("docs/guida.md").oid_index));
    assert!(!guide.hunks.is_empty());

    // Il file non tracciato (`note.txt`) non compare.
    let unstaged = repo.diff(&worktree()).unwrap();
    assert_eq!(
        summary(&unstaged),
        [
            (Change::Modified, "README.md".into(), "README.md".into()),
            (Change::Modified, "src/lib.rs".into(), "src/lib.rs".into()),
        ]
    );
    // Il worktree parte dall'indice.
    let readme_wt = file(&unstaged, "README.md");
    assert_eq!(readme_wt.old_oid, Some(info("README.md").oid_index));
    let added: Vec<_> = readme_wt
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == LineKind::Added)
        .map(|l| l.content.to_string())
        .collect();
    assert_eq!(
        added,
        [
            "ultima modifica: modifica non in stage",
            "riga aggiunta da: modifica non in stage"
        ]
    );
    let lib = file(&unstaged, "src/lib.rs");
    assert_eq!(lib.old_oid, Some(info("src/lib.rs").oid_index));
    assert!(
        lib.new_oid
            .is_some_and(|id| id != Oid::from_bytes([0; Oid::LEN]))
    );
    assert_round_trip(&staged);
    assert_round_trip(&unstaged);

    // Limitati a un file.
    let path = |p: &str| Some(RepoPath::from(p));
    let only = repo.diff(&DiffSpec::Index {
        path: path("README.md"),
    });
    assert_eq!(only.unwrap().files, std::slice::from_ref(readme));
    let only = repo.diff(&DiffSpec::Worktree {
        path: path("src/lib.rs"),
    });
    assert_eq!(only.unwrap().files, std::slice::from_ref(lib));
    for p in ["note.txt", "src/lib.rs", "src", "nessuno"] {
        let none = repo.diff(&DiffSpec::Index { path: path(p) }).unwrap();
        assert!(none.files.is_empty(), "{p}");
    }
}

#[test]
fn repo_vuoto_contenuto_sconosciuto_e_rev_sconosciuta() {
    let mut repo = MemoryBackend::new();
    assert_eq!(repo.diff(&worktree()).unwrap(), Diff::default());
    assert_eq!(repo.diff(&index()).unwrap(), Diff::default());
    assert!(matches!(
        repo.diff(&commit("HEAD")),
        Err(EngineError::UnknownRev(_))
    ));

    // Branch senza commit: un file in stage è aggiunto rispetto all'albero vuoto. Senza il
    // contenuto del blob il file compare come binario.
    let blob = MemoryBackend::fake_oid(77);
    let entry = |path: &str| StatusEntry {
        path: RepoPath::from(path),
        index: Change::Added,
        worktree: Change::Unmodified,
        kind: EntryKind::Ordinary(TrackedInfo {
            submodule: None,
            mode_head: 0,
            mode_index: 0o100644,
            mode_worktree: 0o100644,
            oid_head: Oid::from_bytes([0; Oid::LEN]),
            oid_index: blob,
        }),
    };
    repo.set_entries(vec![entry("a.txt")]);
    let diff = repo.diff(&index()).unwrap();
    assert_eq!(diff.files[0].change, Change::Added);
    assert!(diff.files[0].binary && diff.files[0].hunks.is_empty());

    repo.set_blob(blob, "uno\ndue");
    let diff = repo.diff(&index()).unwrap();
    let f = &diff.files[0];
    assert!(!f.binary);
    assert_eq!(f.hunks[0].header(), b"@@ -0,0 +1,2 @@");
    assert!(f.hunks[0].lines[1].no_newline);
    assert_round_trip(&diff);

    // Un NUL nel contenuto: binario, come per git.
    repo.set_blob(blob, b"\0\x01".to_vec());
    assert!(repo.diff(&index()).unwrap().files[0].binary);

    let sample = sample();
    for rev in ["nessuno", ""] {
        let err = sample.diff(&commit(rev)).unwrap_err();
        assert!(
            matches!(err, EngineError::UnknownRev(ref r) if r == rev),
            "{err}"
        );
    }
}
