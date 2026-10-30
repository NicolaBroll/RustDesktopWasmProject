//! Snapshots (`insta`) of the JSON result of every command of [`Registry::builtin`] on the
//! `MemoryBackend`: the sample repo ([`MemoryBackend::sample`]) and, for the operations that stop
//! on a conflict (merge, cherry-pick, revert, rebase), a small repo with a conflict on `a.txt`.
//! They pin the shape of the results while the handlers move to typed results (Q11 of
//! `impl-docs/ROADMAP-quality.md`): a change of the output shows up as a snapshot diff.
//!
//! Every command id has at least one snapshot named `<id>` or `<id>-<variant>`, or is listed in
//! [`EXCLUDED`] with the reason; [`every_command_has_a_snapshot`] checks it. New snapshots:
//! `INSTA_UPDATE=always cargo test -p project-engine snapshot_tests` (or `cargo insta review`).
//! The repos and helpers marked `pub(super)` are shared with the journal tests of `edit.rs`.

use crate::{MemoryBackend, MemoryOpener, PullSettings, Registry, Session};
use project_domain::{Change, ChangedFile, Commit, Oid, RepoPath, Signature};
use serde_json::{Value, json};
use std::sync::Arc;

/// Commands without a snapshot, with the reason. Empty: the `MemoryBackend` runs them all.
const EXCLUDED: &[(&str, &str)] = &[];

/// Prefix of the snapshot files of this module (`<crate>__<module path>__`).
const SNAPSHOT_PREFIX: &str = "project_engine__builtin__snapshot_tests__";

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

pub(super) fn id(n: u64) -> String {
    oid(n).to_string()
}

pub(super) fn opened_with(repo: MemoryBackend) -> (Registry, Session) {
    let mut opener = MemoryOpener::default();
    opener.insert("r", repo);
    let mut s = Session::new(Arc::new(opener));
    let r = Registry::builtin();
    r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
    (r, s)
}

/// The sample repo, opened.
pub(super) fn sample() -> (Registry, Session) {
    opened_with(MemoryBackend::sample())
}

/// The sample repo without local changes.
pub(super) fn clean_sample() -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    repo.set_entries(Vec::new());
    repo
}

pub(super) fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// Runs `id` and snapshots its result as `<id>` (or `<id>-<variant>`).
fn snap(r: &Registry, s: &mut Session, id: &str, variant: &str, params: Value) -> Value {
    let out = run(r, s, id, params);
    let name = if variant.is_empty() {
        id.to_owned()
    } else {
        format!("{id}-{variant}")
    };
    insta::assert_json_snapshot!(name, out);
    out
}

fn sig(n: u64) -> Signature {
    Signature {
        name: "Ada".into(),
        email: "ada@example.com".into(),
        time: 1_700_000_000 + i64::try_from(n).unwrap_or(0) * 60,
        offset_min: 0,
    }
}

fn blob(repo: &mut MemoryBackend, content: &str) -> Oid {
    let hash = content.bytes().fold(1_469_598_103_934_665_603_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1_099_511_628_211)
    });
    let id = oid(hash);
    repo.set_blob(id, content);
    id
}

/// Commit `n` with parent `parent` and the files (path, content before if any, content after).
fn commit(
    repo: &mut MemoryBackend,
    n: u64,
    parent: Option<u64>,
    summary: &str,
    files: &[(&str, Option<&str>, &str)],
) {
    let changes = files
        .iter()
        .map(|(path, old, new)| {
            let old_oid = old.map(|c| blob(repo, c));
            ChangedFile {
                path: RepoPath::from(*path),
                change: if old.is_some() {
                    Change::Modified
                } else {
                    Change::Added
                },
                original: None,
                score: None,
                old_mode: if old.is_some() { 0o100644 } else { 0 },
                new_mode: 0o100644,
                old_oid: old_oid.unwrap_or(Oid::from_bytes([0; Oid::LEN])),
                new_oid: blob(repo, new),
            }
        })
        .collect();
    repo.insert_commit(Commit {
        id: oid(n),
        parents: parent.map(oid).into_iter().collect(),
        author: sig(n),
        committer: sig(n),
        summary: summary.into(),
        body: String::new(),
    });
    repo.set_changes(oid(n), changes);
}

/// Clean repo: `1` (root: `a.txt` = "base", `b.txt`); `main` at `2` (`a.txt` = "ours");
/// `other` at `3` from `1` (`a.txt` = "theirs": conflict with `main`); `feat` at `4` from `1`
/// (new `n.txt`); `ahead` at `5` from `2` (new `c.txt`: fast-forward of `main`).
pub(super) fn conflict_repo() -> MemoryBackend {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    commit(
        &mut repo,
        1,
        None,
        "base",
        &[("a.txt", None, "base\n"), ("b.txt", None, "b\n")],
    );
    commit(
        &mut repo,
        2,
        Some(1),
        "ours",
        &[("a.txt", Some("base\n"), "ours\n")],
    );
    commit(
        &mut repo,
        3,
        Some(1),
        "theirs",
        &[("a.txt", Some("base\n"), "theirs\n")],
    );
    commit(&mut repo, 4, Some(1), "new", &[("n.txt", None, "new\n")]);
    commit(&mut repo, 5, Some(2), "ahead", &[("c.txt", None, "c\n")]);
    repo.set_branch("main", oid(2));
    repo.set_branch("other", oid(3));
    repo.set_branch("feat", oid(4));
    repo.set_branch("ahead", oid(5));
    repo.set_head_branch("main");
    repo
}

/// The clean sample with `origin/main` at a commit `20` (child of `11`, adds `src/bin/app.rs`)
/// that `main` does not have, and `pull.rebase=true`: `remote.pull` stops on a conflict.
pub(super) fn pull_rebase_conflict() -> (Registry, Session) {
    let mut repo = clean_sample();
    let who = Signature {
        name: "Carla".into(),
        email: "carla@example.com".into(),
        time: 1_700_000_000 + 20 * 3_600,
        offset_min: 0,
    };
    repo.insert_remote_commit(
        "origin",
        Commit {
            id: oid(20),
            parents: vec![oid(11)],
            author: who.clone(),
            committer: who,
            summary: "From the remote".into(),
            body: String::new(),
        },
    );
    repo.set_blob(oid(5_000), "from the remote\n");
    repo.set_changes(
        oid(20),
        vec![ChangedFile {
            path: RepoPath::from("src/bin/app.rs"),
            change: Change::Added,
            original: None,
            score: None,
            old_mode: 0,
            new_mode: 0o100644,
            old_oid: Oid::from_bytes([0; Oid::LEN]),
            new_oid: oid(5_000),
        }],
    );
    repo.set_branch_on_remote("origin", "main", oid(20));
    repo.set_pull_settings(PullSettings {
        rebase: Some("true".into()),
        ff: None,
    });
    opened_with(repo)
}

#[test]
fn every_command_has_a_snapshot() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/builtin/snapshots");
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            let name = name.strip_prefix(SNAPSHOT_PREFIX)?.strip_suffix(".snap")?;
            Some(name.to_owned())
        })
        .collect();
    let r = Registry::builtin();
    let mut missing = Vec::new();
    for spec in r.list() {
        let excluded = EXCLUDED.iter().any(|(id, _)| *id == spec.id);
        let covered = names
            .iter()
            .any(|n| n == spec.id || n.starts_with(&format!("{}-", spec.id)));
        assert!(
            !(excluded && covered),
            "{} is excluded but has a snapshot",
            spec.id
        );
        if !excluded && !covered {
            missing.push(spec.id);
        }
    }
    assert!(
        missing.is_empty(),
        "commands without a snapshot (add one here or to EXCLUDED with the reason): {missing:?}"
    );
    for (id, _) in EXCLUDED {
        assert!(
            r.get(id).is_some(),
            "EXCLUDED names an unknown command {id}"
        );
    }
}

#[test]
fn repo_reads() {
    let mut s = Session::new(Arc::new(MemoryOpener::sample()));
    let r = Registry::builtin();
    snap(
        &r,
        &mut s,
        "repo.open",
        "",
        json!({ "path": MemoryOpener::SAMPLE_PATH }),
    );
    snap(&r, &mut s, "repo.status", "", json!({}));
    snap(&r, &mut s, "repo.refs", "", json!({}));
    snap(&r, &mut s, "repo.operation", "none", json!({}));
    snap(&r, &mut s, "repo.log", "has_more", json!({ "limit": 3 }));
    snap(
        &r,
        &mut s,
        "repo.log",
        "all_last_page",
        json!({ "all": true, "skip": 10 }),
    );
    snap(
        &r,
        &mut s,
        "repo.log",
        "rev",
        json!({ "rev": "feature/graph" }),
    );
    snap(&r, &mut s, "repo.search", "", json!({ "query": "parser" }));
    snap(
        &r,
        &mut s,
        "repo.search",
        "has_more",
        json!({ "query": "lanes", "in": ["message"], "limit": 1 }),
    );
    snap(
        &r,
        &mut s,
        "repo.search",
        "path",
        json!({ "query": "a", "path": "README.md" }),
    );
    snap(&r, &mut s, "repo.optimize", "", json!({}));
}

#[test]
fn commit_and_file_reads() {
    let (r, mut s) = sample();
    snap(&r, &mut s, "commit.show", "rename", json!({ "id": id(12) }));
    snap(&r, &mut s, "commit.show", "merge", json!({ "id": id(10) }));
    snap(&r, &mut s, "commit.show", "root", json!({ "id": id(1) }));
    snap(
        &r,
        &mut s,
        "file.blame",
        "worktree",
        json!({ "path": "README.md" }),
    );
    snap(
        &r,
        &mut s,
        "file.blame",
        "rev",
        json!({ "path": "src/lib.rs", "rev": "main" }),
    );
    snap(
        &r,
        &mut s,
        "file.log",
        "follow",
        json!({ "path": "src/bin/app.rs", "follow": true }),
    );
    snap(
        &r,
        &mut s,
        "file.log",
        "has_more",
        json!({ "path": "src/lib.rs", "limit": 2 }),
    );
}

#[test]
fn diffs() {
    let (r, mut s) = sample();
    snap(&r, &mut s, "diff.worktree", "", json!({}));
    snap(
        &r,
        &mut s,
        "diff.worktree",
        "path",
        json!({ "path": "README.md" }),
    );
    snap(&r, &mut s, "diff.index", "", json!({}));
    snap(&r, &mut s, "diff.commit", "", json!({ "id": id(12) }));
    snap(
        &r,
        &mut s,
        "diff.commit",
        "path",
        json!({ "id": id(4), "path": "src/lib.rs" }),
    );
    snap(
        &r,
        &mut s,
        "diff.range",
        "",
        json!({ "from": id(8), "to": id(9), "merge_base": true, "path": "src/graph.rs" }),
    );
}

/// The `digest` and the first added line of `path` in the diff `diff_id`.
fn added_line(r: &Registry, s: &mut Session, diff_id: &str, path: &str) -> (Value, Value) {
    let diff = run(r, s, diff_id, json!({ "path": path }));
    let file = &diff["files"][0];
    let line = file["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|h| h["lines"].as_array().unwrap())
        .find(|l| l["kind"] == "Added")
        .unwrap_or_else(|| panic!("no added line in {diff}"));
    (file["digest"].clone(), json!({ "new": line["new_line"] }))
}

#[test]
fn index_writes() {
    let (r, mut s) = sample();
    snap(
        &r,
        &mut s,
        "index.stage",
        "",
        json!({ "paths": ["note.txt"] }),
    );
    snap(
        &r,
        &mut s,
        "index.unstage",
        "",
        json!({ "paths": ["docs/guide.md"] }),
    );
    let (digest, line) = added_line(&r, &mut s, "diff.worktree", "src/lib.rs");
    snap(
        &r,
        &mut s,
        "index.stage_lines",
        "",
        json!({ "path": "src/lib.rs", "lines": [line], "digest": digest }),
    );
    let (digest, line) = added_line(&r, &mut s, "diff.index", "README.md");
    snap(
        &r,
        &mut s,
        "index.unstage_lines",
        "",
        json!({ "path": "README.md", "lines": [line], "digest": digest }),
    );
}

#[test]
fn commit_discard_and_journal() {
    let (r, mut s) = sample();
    snap(&r, &mut s, "edit.history", "empty", json!({}));
    snap(
        &r,
        &mut s,
        "commit.create",
        "",
        json!({ "message": "Docs guide\n\nWith a body." }),
    );
    snap(
        &r,
        &mut s,
        "commit.create",
        "amend",
        json!({ "message": "Docs guide, amended", "amend": true }),
    );
    snap(
        &r,
        &mut s,
        "worktree.discard",
        "",
        json!({ "paths": ["src/lib.rs"], "confirm": true }),
    );
    snap(&r, &mut s, "edit.history", "", json!({}));
    snap(&r, &mut s, "edit.undo", "", json!({}));
    snap(&r, &mut s, "edit.redo", "", json!({}));
}

#[test]
fn branch_and_tag_writes() {
    let (r, mut s) = sample();
    snap(&r, &mut s, "branch.create", "", json!({ "name": "topic" }));
    snap(
        &r,
        &mut s,
        "branch.create",
        "start",
        json!({ "name": "old", "start": "v0.1.0" }),
    );
    snap(
        &r,
        &mut s,
        "branch.rename",
        "",
        json!({ "name": "old", "new_name": "older" }),
    );
    snap(&r, &mut s, "branch.delete", "", json!({ "name": "older" }));
    snap(
        &r,
        &mut s,
        "branch.checkout",
        "",
        json!({ "name": "topic" }),
    );
    snap(&r, &mut s, "tag.create", "", json!({ "name": "v0.2.0" }));
    snap(
        &r,
        &mut s,
        "tag.create",
        "annotated",
        json!({ "name": "v0.2.1", "target": id(11), "message": "Release 0.2.1" }),
    );
    snap(
        &r,
        &mut s,
        "tag.delete",
        "",
        json!({ "name": "v0.1.0", "confirm": true }),
    );
}

#[test]
fn branch_delete_unmerged() {
    let (r, mut s) = opened_with(conflict_repo());
    snap(
        &r,
        &mut s,
        "branch.delete",
        "unmerged",
        json!({ "name": "other", "confirm": true, "expected": id(3) }),
    );
}

#[test]
fn resets() {
    let (r, mut s) = sample();
    snap(
        &r,
        &mut s,
        "branch.reset_preview",
        "hard",
        json!({ "target": id(11), "mode": "hard" }),
    );
    snap(
        &r,
        &mut s,
        "branch.reset_preview",
        "soft",
        json!({ "target": id(11), "mode": "soft" }),
    );
    snap(
        &r,
        &mut s,
        "branch.reset",
        "soft",
        json!({ "target": id(11), "mode": "soft" }),
    );
    let (r, mut s) = sample();
    snap(
        &r,
        &mut s,
        "branch.reset",
        "hard",
        json!({ "target": id(11), "mode": "hard", "confirm": true, "expected": id(12) }),
    );
}

#[test]
fn remotes() {
    let (r, mut s) = sample();
    snap(&r, &mut s, "remote.list", "", json!({}));
    snap(
        &r,
        &mut s,
        "remote.add",
        "",
        json!({ "name": "fork", "url": "https://example.com/fork.git" }),
    );
    snap(
        &r,
        &mut s,
        "remote.remove",
        "",
        json!({ "name": "fork", "confirm": true }),
    );
    snap(
        &r,
        &mut s,
        "remote.add",
        "fetch",
        json!({ "name": "mirror", "url": "https://example.com/project.git", "fetch": true }),
    );
    snap(&r, &mut s, "remote.fetch", "", json!({}));
    snap(
        &r,
        &mut s,
        "remote.fetch",
        "branch",
        json!({ "remote": "origin", "branch": "main" }),
    );
}

#[test]
fn sync() {
    let (r, mut s) = sample();
    snap(
        &r,
        &mut s,
        "remote.pull",
        "fast_forward",
        json!({ "branch": "feature/graph" }),
    );
    snap(&r, &mut s, "remote.pull", "up_to_date", json!({}));
    snap(&r, &mut s, "remote.push", "", json!({}));
    snap(
        &r,
        &mut s,
        "remote.delete_branch",
        "",
        json!({
            "remote": "origin",
            "branch": "feature/graph",
            "confirm": true,
            "expected": id(13),
        }),
    );
    snap(
        &r,
        &mut s,
        "remote.push_tag",
        "",
        json!({ "remote": "origin", "name": "v0.1.0" }),
    );
    snap(
        &r,
        &mut s,
        "remote.delete_tag",
        "",
        json!({
            "remote": "origin",
            "name": "v0.1.0",
            "confirm": true,
            "expected": id(1_000),
        }),
    );
}

#[test]
fn pull_rebase_continue() {
    let (r, mut s) = pull_rebase_conflict();
    snap(&r, &mut s, "remote.pull", "rebase_conflict", json!({}));
    snap(&r, &mut s, "repo.operation", "rebase", json!({}));
    run(
        &r,
        &mut s,
        "index.stage",
        json!({ "paths": ["src/bin/app.rs"] }),
    );
    snap(&r, &mut s, "rebase.continue", "", json!({}));
}

#[test]
fn pull_rebase_abort() {
    let (r, mut s) = pull_rebase_conflict();
    run(&r, &mut s, "remote.pull", json!({}));
    snap(&r, &mut s, "rebase.abort", "", json!({ "confirm": true }));
}

#[test]
fn stashes() {
    let (r, mut s) = sample();
    snap(&r, &mut s, "stash.list", "empty", json!({}));
    snap(&r, &mut s, "stash.push", "", json!({}));
    snap(
        &r,
        &mut s,
        "stash.push",
        "untracked",
        json!({ "message": "Notes", "include_untracked": true }),
    );
    let list = snap(&r, &mut s, "stash.list", "", json!({}));
    snap(&r, &mut s, "stash.apply", "", json!({ "index": 1 }));
    run(
        &r,
        &mut s,
        "worktree.discard",
        json!({ "paths": ["README.md", "src/lib.rs"], "confirm": true }),
    );
    snap(&r, &mut s, "stash.pop", "", json!({ "index": 0 }));
    let expected = list["stashes"][1]["id"].clone();
    snap(
        &r,
        &mut s,
        "stash.drop",
        "",
        json!({ "index": 0, "expected": expected, "confirm": true }),
    );
}

#[test]
fn merges() {
    let (r, mut s) = opened_with(conflict_repo());
    snap(
        &r,
        &mut s,
        "merge.start",
        "fast_forward",
        json!({ "rev": "ahead" }),
    );
    let (r, mut s) = opened_with(conflict_repo());
    snap(
        &r,
        &mut s,
        "merge.start",
        "merge_commit",
        json!({ "rev": "feat" }),
    );
    let (r, mut s) = opened_with(conflict_repo());
    snap(
        &r,
        &mut s,
        "merge.start",
        "conflict",
        json!({ "rev": "other" }),
    );
    snap(&r, &mut s, "repo.operation", "merge", json!({}));
    let shown = snap(&r, &mut s, "conflict.show", "", json!({ "path": "a.txt" }));
    snap(
        &r,
        &mut s,
        "conflict.resolve",
        "",
        json!({ "path": "a.txt", "digest": shown["digest"], "content": "ours and theirs\n" }),
    );
    snap(&r, &mut s, "merge.continue", "", json!({}));
    let (r, mut s) = opened_with(conflict_repo());
    run(&r, &mut s, "merge.start", json!({ "rev": "other" }));
    snap(&r, &mut s, "merge.abort", "", json!({ "confirm": true }));
}

#[test]
fn cherry_picks_and_reverts() {
    let (r, mut s) = opened_with(conflict_repo());
    snap(
        &r,
        &mut s,
        "commit.cherry_pick",
        "",
        json!({ "rev": "feat" }),
    );
    snap(&r, &mut s, "commit.revert", "", json!({ "rev": "main" }));
    let (r, mut s) = opened_with(conflict_repo());
    snap(
        &r,
        &mut s,
        "commit.cherry_pick",
        "conflict",
        json!({ "rev": "other" }),
    );
    snap(&r, &mut s, "repo.operation", "cherry_pick", json!({}));
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    snap(&r, &mut s, "cherry_pick.continue", "", json!({}));
    let (r, mut s) = opened_with(conflict_repo());
    run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "other" }));
    snap(
        &r,
        &mut s,
        "cherry_pick.abort",
        "",
        json!({ "confirm": true }),
    );
    let (r, mut s) = opened_with(conflict_repo());
    snap(
        &r,
        &mut s,
        "commit.revert",
        "conflict",
        json!({ "rev": "other" }),
    );
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    snap(&r, &mut s, "revert.continue", "", json!({}));
    let (r, mut s) = opened_with(conflict_repo());
    run(&r, &mut s, "commit.revert", json!({ "rev": "other" }));
    snap(&r, &mut s, "revert.abort", "", json!({ "confirm": true }));
}

#[test]
fn rebases() {
    let (r, mut s) = opened_with(clean_sample());
    snap(&r, &mut s, "rebase.plan", "", json!({ "base": id(10) }));
    snap(
        &r,
        &mut s,
        "rebase.start",
        "reword",
        json!({
            "base": id(11),
            "steps": [{ "action": "reword", "commit": id(12), "message": "Commit panel, v2" }],
        }),
    );
    let (r, mut s) = opened_with(clean_sample());
    snap(
        &r,
        &mut s,
        "rebase.start",
        "published",
        json!({
            "base": id(10),
            "steps": [
                { "action": "pick", "commit": id(11) },
                { "action": "squash", "commit": id(12) },
            ],
            "confirm": true,
        }),
    );
}

#[test]
fn clone_and_init() {
    let mut opener = MemoryOpener::new();
    opener.insert("remote", MemoryBackend::sample());
    let mut s = Session::new(Arc::new(opener));
    let r = Registry::builtin();
    snap(
        &r,
        &mut s,
        "repo.clone",
        "",
        json!({ "url": "remote", "path": "dest" }),
    );
    snap(
        &r,
        &mut s,
        "repo.clone",
        "branch",
        json!({ "url": "remote", "path": "dest2", "branch": "feature/graph", "depth": 1 }),
    );
    snap(&r, &mut s, "repo.init", "", json!({ "path": "new" }));
    snap(
        &r,
        &mut s,
        "repo.init",
        "branch",
        json!({ "path": "new2", "branch": "trunk" }),
    );
}
