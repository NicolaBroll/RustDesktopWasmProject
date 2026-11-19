//! Helpers shared by the UI's unit tests.

use project_domain::{Change, ChangedFile, Commit, EntryKind, Oid, RepoPath, Signature};
use project_engine::{GitRead, GitStash, MemoryBackend, StashPush};

/// The fake id of seed `n` ([`MemoryBackend::fake_oid`]).
pub(crate) fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

/// The sample repo with its changes in a stash, then a commit that changes `README.md` on
/// `main`: applying the stash conflicts on that file.
pub(crate) fn stash_in_conflict() -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    let old = match &repo.status().unwrap().entries[0].kind {
        EntryKind::Ordinary(info) => info.oid_head,
        other => panic!("{other:?}"),
    };
    repo.stash_push(&StashPush::default()).unwrap();
    let head = repo.head().unwrap().target().unwrap();
    let new = oid(4242);
    repo.set_blob(new, "README changed on main\n");
    let id = oid(4243);
    let who = Signature {
        name: "Ada".to_owned(),
        email: "ada@example.com".to_owned(),
        time: 1_800_000_000,
        offset_min: 0,
    };
    repo.insert_commit(Commit {
        id,
        parents: vec![head],
        author: who.clone(),
        committer: who,
        summary: "README".to_owned(),
        body: String::new(),
    });
    repo.set_changes(
        id,
        vec![ChangedFile {
            path: RepoPath::from("README.md"),
            change: Change::Modified,
            original: None,
            score: None,
            old_mode: 0o100644,
            new_mode: 0o100644,
            old_oid: old,
            new_oid: new,
        }],
    );
    repo.set_branch("main", id);
    repo
}
