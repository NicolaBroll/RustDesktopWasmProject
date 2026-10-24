//! The web demo repo: the `MemoryBackend` sample repo plus the branch
//! `experiment`, which changes `src/lib.rs` like `main` (conflict) and adds `docs/layout.md`.
//! This way the demo can try a merge stopped on a conflict: "Stash…" the local changes, then
//! on the `experiment` branch "Merge into main" → the in-progress merge panel appears.

use project_domain::{Change, ChangedFile, Commit, Oid, RepoPath, Signature};
use project_engine::{GitBackend, MemoryBackend, MemoryOpener};

/// The branch that conflicts with `main`.
pub const BRANCH: &str = "experiment";

/// Seed of the fake commit ids and added contents, far from those of the sample.
const COMMIT_SEED: u64 = 50;
const BLOB_SEED: u64 = 5_000;
const FILE_MODE: u32 = 0o100644;

/// The demo opener: only the repo from [`repo`], at the sample repo's path.
pub fn opener() -> MemoryOpener {
    let mut opener = MemoryOpener::new();
    opener.insert(MemoryOpener::SAMPLE_PATH, repo());
    opener
}

/// The sample repo with the [`BRANCH`] branch: a commit on the parent of `main` that changes
/// `src/lib.rs` (also changed by the last commit of `main`) and adds `docs/layout.md`. If
/// the sample changed and this could not be built, the plain sample repo remains.
pub fn repo() -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    if let Some((parent, lib_old)) = lib_before_main(&repo) {
        add_branch(&mut repo, parent, lib_old);
    }
    repo
}

/// The parent of `main` and the id of `src/lib.rs` there (i.e. before the last commit of `main`).
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
    let lib_new = format!("{old}// experiment: the commit panel in two columns\n");
    let layout = "# Layout\n\nThe commit panel in two columns: graph on the left, \
                  details on the right.\n";
    let lib_id = MemoryBackend::fake_oid(BLOB_SEED);
    let layout_id = MemoryBackend::fake_oid(BLOB_SEED + 1);
    repo.set_blob(lib_id, lib_new);
    repo.set_blob(layout_id, layout);
    let id = MemoryBackend::fake_oid(COMMIT_SEED);
    let who = Signature {
        name: "Bruno Verdi".to_owned(),
        email: "bruno@example.com".to_owned(),
        // After the sample's last commit.
        time: 1_700_000_000 + 12 * 3_600 + 1_800,
        offset_min: -300,
    };
    repo.insert_commit(Commit {
        id,
        parents: vec![parent],
        author: who.clone(),
        committer: who,
        summary: "Panel: try a two-column layout".to_owned(),
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

    /// The demo path: stash the local changes, merge `experiment` into `main`
    /// → in-progress merge with `src/lib.rs` in conflict, which `repo.operation` shows.
    #[test]
    fn demo_leads_to_a_conflicted_merge() {
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
        // The stage area shows it as a conflicted file.
        let diff = run("diff.worktree", json!({ "path": "src/lib.rs" }));
        assert_eq!(diff["files"][0]["change"], "Unmerged", "{diff}");
    }
}
