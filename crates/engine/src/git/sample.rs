//! The sample repo of [`MemoryBackend::sample`].

use super::MemoryBackend;
use project_domain::{
    Change, ChangedFile, Commit, EntryKind, Oid, RepoPath, Signature, StatusEntry, TrackedInfo,
};
use std::collections::BTreeMap;

/// Date of the first commit (14 November 2023); the following ones are one hour apart.
const START: i64 = 1_700_000_000;
const HOUR: i64 = 3_600;
const FILE_MODE: u32 = 0o100644;

/// Seed of the fake ids: commit `n` → `n`, tag objects and files from here on.
const TAG_SEED: u64 = 1_000;
const BLOB_SEED: u64 = 2_000;
/// First seed of the file contents in the commits (a new one for each version of a file).
const HISTORY_SEED: u64 = 3_000;

struct Person {
    name: &'static str,
    email: &'static str,
    offset_min: i32,
}

const ADA: Person = Person {
    name: "Ada Rossi",
    email: "ada@example.com",
    offset_min: 60,
};
const BRUNO: Person = Person {
    name: "Bruno Verdi",
    email: "bruno@example.com",
    offset_min: -300,
};

/// Commits of the sample: (number, parents, author, subject, body). The number gives the seed
/// of the id and the date (`START + (n - 1) hours`).
const COMMITS: &[(u64, &[u64], &Person, &str, &str)] = &[
    (1, &[], &ADA, "First commit", ""),
    (2, &[1], &ADA, "Add the README", ""),
    (
        3,
        &[2],
        &BRUNO,
        "Project structure",
        "crates/ and apps/ folders, Cargo workspace.",
    ),
    (4, &[3], &ADA, "Log parser", ""),
    (5, &[4], &BRUNO, "Graph lanes: first version", ""),
    (6, &[4], &ADA, "Fix a typo in the README", ""),
    (
        7,
        &[5],
        &BRUNO,
        "Lanes: reuse the free columns",
        "A lane that closes becomes available for the next fork.\n\n\
         This keeps the graph narrow even with many branches.",
    ),
    (8, &[6], &ADA, "Status parser", ""),
    (9, &[7], &BRUNO, "Property tests for the lanes", ""),
    (10, &[8, 9], &ADA, "Merge branch 'feature/graph'", ""),
    (11, &[10], &ADA, "Ref parser", ""),
    (12, &[11], &BRUNO, "Commit panel", ""),
];

/// A change to a file in a commit, relative to the first parent.
enum Edit {
    Add(&'static str),
    Modify(&'static str),
    Delete(&'static str),
    /// From, to, similarity: with 100 the content does not change.
    Rename(&'static str, &'static str, u8),
}

use Edit::{Add, Delete, Modify, Rename};

/// Files changed by each commit of [`COMMITS`], relative to the first parent. For the merge (10)
/// they are the files brought in by `feature/graph`, with the content of the second parent.
const CHANGES: &[(u64, &[Edit])] = &[
    (1, &[Add(".gitignore"), Add(".gitkeep")]),
    (2, &[Add("README.md")]),
    (
        3,
        &[
            Add("Cargo.toml"),
            Add("src/lib.rs"),
            Add("src/main.rs"),
            Delete(".gitkeep"),
        ],
    ),
    (4, &[Add("src/log.rs"), Modify("src/lib.rs")]),
    (5, &[Add("src/graph.rs")]),
    (6, &[Modify("README.md")]),
    (7, &[Modify("src/graph.rs")]),
    (8, &[Add("src/status.rs"), Modify("src/lib.rs")]),
    (9, &[Add("tests/graph.rs"), Modify("Cargo.toml")]),
    (
        10,
        &[
            Add("src/graph.rs"),
            Add("tests/graph.rs"),
            Modify("Cargo.toml"),
        ],
    ),
    (11, &[Add("src/refs.rs"), Modify("src/lib.rs")]),
    (
        12,
        &[
            Add("src/panel.rs"),
            Modify("src/lib.rs"),
            Rename("src/main.rs", "src/bin/app.rs", 92),
        ],
    ),
];

type Tree = BTreeMap<&'static str, Oid>;

/// The history of the sample: files changed by each commit, content of every blob and tree of
/// HEAD.
struct History {
    changes: Vec<(Oid, Vec<ChangedFile>)>,
    blobs: BTreeMap<Oid, String>,
    head: Tree,
}

/// The comment prefix in the file type of `path` (empty for Markdown).
fn comment(path: &str) -> &'static str {
    if path.ends_with(".rs") {
        "// "
    } else if path.ends_with(".md") {
        ""
    } else {
        "# "
    }
}

/// Content of `path` created by `label` (e.g. `commit 4`): a title, a line saying who last
/// changed it and a body that depends on the file type. `.gitkeep` is empty.
fn initial_content(path: &str, label: &str) -> String {
    let c = comment(path);
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .split('.')
        .next()
        .unwrap_or_default();
    let body = if path.ends_with(".gitkeep") {
        return String::new();
    } else if path.ends_with(".rs") {
        format!("pub fn {stem}() -> &'static str {{\n    \"{path}\"\n}}\n")
    } else if path.ends_with(".md") {
        format!("# {stem}\n\nProject documentation.\n")
    } else if path.ends_with(".toml") {
        "[package]\nname = \"project\"\nedition = \"2024\"\n".to_owned()
    } else {
        "/target\n".to_owned()
    };
    format!("{c}{path}\n{c}last change: {label}\n\n{body}")
}

/// New version of a content, changed by `label`: the second line says who last changed it
/// and a line is added at the end. In the diff: one changed line and one added line.
fn evolve(old: &str, path: &str, label: &str) -> String {
    let c = comment(path);
    let mut lines: Vec<String> = old.lines().map(str::to_owned).collect();
    if let Some(second) = lines.get_mut(1) {
        *second = format!("{c}last change: {label}");
    }
    lines.push(format!("{c}line added by: {label}"));
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// The files changed by each commit, with content ids consistent along the history: the "old"
/// content of a file is the one left by the previous commit that touched it.
fn history() -> History {
    let zero = Oid::from_bytes([0; Oid::LEN]);
    let mut next_seed = HISTORY_SEED;
    let mut blobs: BTreeMap<Oid, String> = BTreeMap::new();
    let mut trees: BTreeMap<u64, Tree> = BTreeMap::new();
    let mut changes = Vec::new();
    for &(n, edits) in CHANGES {
        let label = format!("commit {n}");
        let parents = COMMITS
            .iter()
            .find(|c| c.0 == n)
            .map(|c| c.1)
            .unwrap_or_default();
        let mut tree = parents
            .first()
            .map(|p| trees[p].clone())
            .unwrap_or_default();
        let theirs = parents.get(1).map(|p| &trees[p]);
        // New content of `path`, which was `old` before. In a merge it comes from the second
        // parent, if it has it.
        let mut new_content = |path: &str, old: Option<Oid>| match theirs.and_then(|t| t.get(path))
        {
            Some(id) => *id,
            None => {
                let text = match old {
                    Some(old) => evolve(&blobs[&old], path, &label),
                    None => initial_content(path, &label),
                };
                next_seed += 1;
                let id = oid(next_seed);
                blobs.insert(id, text);
                id
            }
        };
        let file =
            |change, path: &str, original: Option<&str>, old: Option<Oid>, new: Option<Oid>| {
                ChangedFile {
                    path: RepoPath::from(path),
                    change,
                    original: original.map(RepoPath::from),
                    score: None,
                    old_mode: if old.is_some() { FILE_MODE } else { 0 },
                    new_mode: if new.is_some() { FILE_MODE } else { 0 },
                    old_oid: old.unwrap_or(zero),
                    new_oid: new.unwrap_or(zero),
                }
            };
        let mut files = Vec::new();
        for edit in edits {
            files.push(match *edit {
                Add(path) => {
                    let new = new_content(path, None);
                    tree.insert(path, new);
                    file(Change::Added, path, None, None, Some(new))
                }
                Modify(path) => {
                    let old = tree[path];
                    let new = new_content(path, Some(old));
                    tree.insert(path, new);
                    file(Change::Modified, path, None, Some(old), Some(new))
                }
                Delete(path) => {
                    let old = tree.remove(path);
                    file(Change::Deleted, path, None, old, None)
                }
                Rename(from, to, score) => {
                    let old = tree.remove(from);
                    let new = if score == 100 {
                        old
                    } else {
                        Some(new_content(to, old))
                    };
                    if let Some(new) = new {
                        tree.insert(to, new);
                    }
                    ChangedFile {
                        score: Some(score),
                        ..file(Change::Renamed, to, Some(from), old, new)
                    }
                }
            });
        }
        // Like git: in path order.
        files.sort_by(|a, b| a.path.cmp(&b.path));
        trees.insert(n, tree);
        changes.push((oid(n), files));
    }
    let head = COMMITS
        .last()
        .map(|c| trees[&c.0].clone())
        .unwrap_or_default();
    History {
        changes,
        blobs,
        head,
    }
}

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn signature(who: &Person, time: i64) -> Signature {
    Signature {
        name: who.name.to_owned(),
        email: who.email.to_owned(),
        time,
        offset_min: who.offset_min,
    }
}

/// Status entry of a tracked file: `head` is the id in HEAD (`None` if the file is not there),
/// `index` the one in the index.
fn tracked(
    path: &str,
    index_change: Change,
    worktree_change: Change,
    head: Option<Oid>,
    index: Oid,
) -> StatusEntry {
    StatusEntry {
        path: RepoPath::from(path),
        index: index_change,
        worktree: worktree_change,
        kind: EntryKind::Ordinary(TrackedInfo {
            submodule: None,
            mode_head: if head.is_some() { FILE_MODE } else { 0 },
            mode_index: FILE_MODE,
            mode_worktree: FILE_MODE,
            oid_head: head.unwrap_or(Oid::from_bytes([0; Oid::LEN])),
            oid_index: index,
        }),
    }
}

impl MemoryBackend {
    /// Sample repo, always the same (fixed ids and dates):
    ///
    /// ```text
    /// * 12 Commit panel            (main, HEAD)
    /// * 11 Ref parser                 (origin/main)
    /// *   10 Merge branch 'feature/graph' (tag v0.1.0, annotated)
    /// |\
    /// | * 9 Property tests for the lanes   (feature/graph, origin/feature/graph)
    /// * | 8 Status parser
    /// | * 7 Lanes: reuse the free columns
    /// * | 6 Fix a typo in the README
    /// | * 5 Graph lanes: first version
    /// |/
    /// * 4 Log parser
    /// * 3 Project structure
    /// * 2 Add the README
    /// * 1 First commit
    /// ```
    ///
    /// `main` follows `origin/main` and is ahead by 1; `feature/graph` follows
    /// `origin/feature/graph`, aligned. The remote `origin` (in memory) has `main` on commit
    /// 11 and `feature/graph` on a commit 13 "Lanes: one color per branch" (child of 9) that the
    /// repo does not have yet: `remote.fetch` downloads it. The worktree is not clean: `README.md` modified
    /// staged and unstaged, `docs/guide.md` added in the stage, `src/lib.rs` modified unstaged,
    /// `note.txt` untracked (with content in the worktree, so it can be staged).
    /// The committer is Ada Rossi (`ada@example.com`).
    ///
    /// Each commit has its changed files (for `commit.show`): additions, modifications, a
    /// deletion (`.gitkeep` in commit 3, empty file) and a rename with changes
    /// (`src/main.rs` → `src/bin/app.rs` in commit 12). The files have content (a few lines
    /// of text, with ids consistent between commits and status), so the diffs of the commits, the index and the
    /// worktree have real hunks.
    pub fn sample() -> Self {
        let mut repo = Self::new();
        for &(n, parents, who, summary, body) in COMMITS {
            let time = START + (i64::try_from(n).unwrap_or(0) - 1) * HOUR;
            repo.insert_commit(Commit {
                id: oid(n),
                parents: parents.iter().map(|&p| oid(p)).collect(),
                author: signature(who, time),
                committer: signature(who, time),
                summary: summary.to_owned(),
                body: body.to_owned(),
            });
        }
        let History {
            changes,
            blobs,
            head,
        } = history();
        for (id, files) in changes {
            repo.set_changes(id, files);
        }
        for (id, content) in &blobs {
            repo.set_blob(*id, content.as_str());
        }
        repo.set_branch("main", oid(12));
        repo.set_branch("feature/graph", oid(9));
        repo.set_remote_branch("origin/main", oid(11));
        repo.set_remote_branch("origin/feature/graph", oid(9));
        repo.set_upstream("main", "origin/main");
        repo.set_upstream("feature/graph", "origin/feature/graph");
        repo.add_remote("origin", "https://example.com/project.git");
        let fetched = START + 12 * HOUR;
        repo.insert_remote_commit(
            "origin",
            Commit {
                id: oid(13),
                parents: vec![oid(9)],
                author: signature(&BRUNO, fetched),
                committer: signature(&BRUNO, fetched),
                summary: "Lanes: one color per branch".to_owned(),
                body: String::new(),
            },
        );
        repo.set_branch_on_remote("origin", "main", oid(11));
        repo.set_branch_on_remote("origin", "feature/graph", oid(13));
        repo.set_tag("v0.1.0", oid(10), Some(oid(TAG_SEED)));
        repo.set_head_branch("main");
        repo.set_identity(ADA.name, ADA.email);

        // Worktree: README.md changed in the stage and then again outside it, docs/guide.md new in
        // the stage, src/lib.rs changed outside the stage, note.txt untracked.
        let staged = "staged change";
        let unstaged = "unstaged change";
        let readme = head["README.md"];
        let readme_index = evolve(&blobs[&readme], "README.md", staged);
        let readme_worktree = evolve(&readme_index, "README.md", unstaged);
        let guide = initial_content("docs/guide.md", staged);
        let lib = head["src/lib.rs"];
        let lib_worktree = evolve(&blobs[&lib], "src/lib.rs", unstaged);
        repo.set_blob(oid(BLOB_SEED), readme_index);
        repo.set_blob(oid(BLOB_SEED + 1), guide);
        repo.set_worktree_file("README.md", readme_worktree);
        repo.set_worktree_file("src/lib.rs", lib_worktree);
        repo.set_worktree_file("note.txt", "Notes not to lose.\n");
        repo.set_entries(vec![
            tracked(
                "README.md",
                Change::Modified,
                Change::Modified,
                Some(readme),
                oid(BLOB_SEED),
            ),
            tracked(
                "docs/guide.md",
                Change::Added,
                Change::Unmodified,
                None,
                oid(BLOB_SEED + 1),
            ),
            tracked(
                "src/lib.rs",
                Change::Unmodified,
                Change::Modified,
                Some(lib),
                lib,
            ),
            StatusEntry {
                path: RepoPath::from("note.txt"),
                index: Change::Untracked,
                worktree: Change::Untracked,
                kind: EntryKind::Untracked,
            },
        ]);
        repo
    }
}
