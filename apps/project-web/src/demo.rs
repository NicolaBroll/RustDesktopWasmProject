//! Il repo della demo web: il repo di esempio del `MemoryBackend` con in più il branch
//! `esperimento`, che cambia `src/lib.rs` come `main` (conflitto) e aggiunge `docs/layout.md`.
//! Così nella demo si prova un merge fermo in conflitto: «Stash…» delle modifiche locali, poi
//! sul branch `esperimento` «Unisci in «main»» → compare il riquadro del merge in corso.

use project_domain::{Change, ChangedFile, Commit, Oid, RepoPath, Signature};
use project_engine::{GitBackend, MemoryBackend, MemoryOpener};

/// Il branch che entra in conflitto con `main`.
pub const BRANCH: &str = "esperimento";

/// Seed degli id finti di commit e contenuti aggiunti, lontani da quelli dell'esempio.
const COMMIT_SEED: u64 = 50;
const BLOB_SEED: u64 = 5_000;
const FILE_MODE: u32 = 0o100644;

/// L'opener della demo: solo il repo di [`repo`], al percorso del repo di esempio.
pub fn opener() -> MemoryOpener {
    let mut opener = MemoryOpener::new();
    opener.insert(MemoryOpener::SAMPLE_PATH, repo());
    opener
}

/// Il repo di esempio con il branch [`BRANCH`]: un commit sul genitore di `main` che cambia
/// `src/lib.rs` (cambiato anche dall'ultimo commit di `main`) e aggiunge `docs/layout.md`. Se
/// l'esempio cambiasse e non si potesse costruire, resta il repo di esempio.
pub fn repo() -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    if let Some((parent, lib_old)) = lib_before_main(&repo) {
        add_branch(&mut repo, parent, lib_old);
    }
    repo
}

/// Il genitore di `main` e l'id di `src/lib.rs` lì (cioè prima dell'ultimo commit di `main`).
fn lib_before_main(repo: &MemoryBackend) -> Option<(Oid, Oid)> {
    let details = repo.show_commit("main").ok()?;
    let parent = *details.commit.parents.first()?;
    let lib = details
        .files
        .iter()
        .find(|f| f.path == RepoPath::from("src/lib.rs"))?;
    Some((parent, lib.old_oid))
}

fn add_branch(repo: &mut MemoryBackend, parent: Oid, lib_old: Oid) {
    let old = repo
        .blob(lib_old)
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    let lib_new = format!("{old}// esperimento: il pannello dei commit in due colonne\n");
    let layout = "# Layout\n\nIl pannello dei commit in due colonne: grafo a sinistra, \
                  dettagli a destra.\n";
    let lib_id = MemoryBackend::fake_oid(BLOB_SEED);
    let layout_id = MemoryBackend::fake_oid(BLOB_SEED + 1);
    repo.set_blob(lib_id, lib_new);
    repo.set_blob(layout_id, layout);
    let id = MemoryBackend::fake_oid(COMMIT_SEED);
    let who = Signature {
        name: "Bruno Verdi".to_owned(),
        email: "bruno@example.com".to_owned(),
        // Dopo l'ultimo commit dell'esempio.
        time: 1_700_000_000 + 12 * 3_600 + 1_800,
        offset_min: -300,
    };
    repo.insert_commit(Commit {
        id,
        parents: vec![parent],
        author: who.clone(),
        committer: who,
        summary: "Pannello: prova di un layout in due colonne".to_owned(),
        body: String::new(),
    });
    let zero = Oid::from_bytes([0; Oid::LEN]);
    repo.set_changes(
        id,
        vec![
            ChangedFile {
                path: RepoPath::from("docs/layout.md"),
                change: Change::Added,
                original: None,
                score: None,
                old_mode: 0,
                new_mode: FILE_MODE,
                old_oid: zero,
                new_oid: layout_id,
            },
            ChangedFile {
                path: RepoPath::from("src/lib.rs"),
                change: Change::Modified,
                original: None,
                score: None,
                old_mode: FILE_MODE,
                new_mode: FILE_MODE,
                old_oid: lib_old,
                new_oid: lib_id,
            },
        ],
    );
    repo.set_branch(BRANCH, id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_engine::{Registry, Session};
    use serde_json::json;
    use std::sync::Arc;

    /// Il percorso della demo: stash delle modifiche locali, merge di `esperimento` in `main`
    /// → merge in corso con `src/lib.rs` in conflitto, che `repo.operation` mostra.
    #[test]
    fn la_demo_porta_a_un_merge_in_conflitto() {
        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(opener()));
        let mut run = |id: &str, params| {
            registry
                .run(id, &mut session, params)
                .unwrap_or_else(|e| panic!("{id}: {e}"))
        };
        run("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        run("stash.push", json!({ "include_untracked": true }));
        let merged = run("merge.start", json!({ "rev": BRANCH }));
        assert_eq!(merged["in_progress"], true, "{merged}");
        assert_eq!(merged["conflicts"], json!(["src/lib.rs"]));
        let operation = run("repo.operation", json!({}));
        assert_eq!(operation["operation"], "Merge");
        assert_eq!(operation["incoming"][0]["label"], BRANCH);
        assert_eq!(operation["continue"], "merge.continue");
        // L'area di stage lo mostra come file in conflitto.
        let diff = run("diff.worktree", json!({ "path": "src/lib.rs" }));
        assert_eq!(diff["files"][0]["change"], "Unmerged", "{diff}");
    }
}
