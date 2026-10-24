# Graphical git client: architecture

Goal: a desktop git client for Linux (then Windows and macOS), open source, in the style of
GitKraken: commit graph, diff with staging per hunk and per line, branches, remotes, conflicts,
interactive rebase. It is built **inside this workspace**, transforming the "list of
items" example step by step. The steps are in [ROADMAP-git.md](ROADMAP-git.md).

**In short**: invariants and safety checklist in [git/rules.md](git/rules.md), to be read
always; this document and the per-function files are opened for the part being touched. CLI examples for
every command in [git/cli.md](git/cli.md).

The general rules of [ARCHITECTURE-rust.md](ARCHITECTURE-rust.md) and of
[AGENTS.md](../AGENTS.md) still apply: layers with dependencies only downward, every modification is a
`Registry` command, no logic in graphical callbacks, `cargo xtask ci` green before delivering.

The dependency versions cited (`gix` 0.89, `git2` 0.21, `tempfile` 3, `insta` 1, git ≥ 2.30)
are those available when the document was written: **verify them when you add the
dependency**.

## 1. Principles

1. **Never lose user data.** Every operation that can delete work (discarding
   changes, reset, deleting unmerged branches, dropping a stash) requires `"confirm": true` in the
   parameters and first saves a recoverable backup (§7). This holds for UI, CLI and MCP alike.
   Operations that git's UX performs without confirmation but that can rewrite the only copy of
   something (stage and unstage of a staged version that is neither in HEAD nor in the worktree)
   do not require `confirm`: they save a backup on their own before writing and, if the backup
   fails, they do not write (§7).
2. **System git to write, pure parsers to read.** Operations go through the installed `git`
   command: this way SSH keys, `ssh-agent`, credential helpers, hooks, commit
   signing and the user's configuration work right away. git's output (porcelain formats, `-z`) is
   read with pure parsers in the domain, testable without processes. `gix` comes in later only where
   speed is needed (reading the log on huge repos, step S36).
3. **The engine does not know where the data comes from.** The git backend is a trait (§4). The desktop uses
   the real backend, tests use an in-memory backend with a sample repo.
4. **Never block the UI.** Slow commands (fetch, push, large logs) run off the UI
   thread (§6).
5. **Same catalog for people and agents.** UI, CLI and MCP use the same command ids.
   An agent can prepare a commit and the user sees the result in the graph.

## 2. Layers

| Layer | Crate | Contents |
| --- | --- | --- |
| L0 | `project-core` | errors, parsing `id=<json>` (unchanged) |
| L2 | `project-domain` | git types (`Oid`, `Commit`, `RefInfo`, `Status`, `Diff`), git output parsers, graph lane algorithm, patch builder |
| L5 | `project-engine` | repo `Session`, command `Registry`, `GitBackend` and `RepoOpener` traits, `MemoryBackend`, `Platform` trait |
| L6 | `project-ui` | egui interface (no `cfg(target_arch)`) |
| L6 | `project-mcp` | MCP server on stdio |
| L6 | `project-git` | **new**: `CliBackend`, runs `git` and uses the domain parsers |
| L7 | `project`, `project-cli` | startup and wiring |

`project-git` sits at L6 because it implements an engine trait (L5). No other L6 crate imports it:
the L7 apps wire it up, as already happens for `Platform`. It must be declared in
`xtask/src/layers.rs` (step S06).

Who uses what:

- **`project`** (desktop): `CliBackend` + `NativePlatform`.
- **`project-cli`**: `CliBackend`; `--repo <path>` opens the repo, `mcp --root <dir>` confines
  paths.

The `project-*` names remain provisional: renaming them is a separate decision, to be made in a single
dedicated step if needed.

## 3. Domain (L2)

Only data and pure functions, with `serde` where command parameters and results need it.

```rust
pub struct Oid([u8; 20]);          // SHA-1; SHA-256 later, if needed
pub struct Signature { pub name: String, pub email: String, pub time: i64, pub offset_min: i32 }
pub struct Commit {
    pub id: Oid,
    pub parents: Vec<Oid>,
    pub author: Signature,
    pub committer: Signature,
    pub summary: String,           // %s: first paragraph on one line
    pub body: String,              // %b: the rest, without trailing newlines
}
pub enum RefKind { LocalBranch, RemoteBranch, Tag, Stash }
pub struct RefInfo { pub name: String, pub kind: RefKind, pub target: Oid, pub tag_object: Option<Oid>, pub upstream: Option<Upstream> }
// target: the commit (for an annotated tag the one the tag points to); tag_object: the id of the annotated tag
pub enum Head { Branch { name: String, target: Option<Oid> }, Detached(Oid) }
pub struct Status { pub head: Head, pub upstream: Option<Upstream>, pub entries: Vec<StatusEntry>, pub operation: Option<Operation> }
pub enum Operation { Merge, Rebase, CherryPick, Revert, Bisect }   // "in progress" state
pub struct Diff { pub files: Vec<FileDiff> }                        // FileDiff → Hunk → Line
// FileDiff: paths, Change, modes, id, binary, hunks; Line: type, line numbers, content as bytes, no_newline
```

### git output parsers

Every git command has a format chosen on purpose to be read without ambiguity:

| Data | Command | Notes |
| --- | --- | --- |
| Log | `git log -z --date=raw --encoding=UTF-8 --no-show-signature --no-color --format=tformat:%H%x00%P%x00%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%s%x00%b` (constant `LOG_ARGS`) | 10 fields per record, **each terminated by NUL** (the tenth NUL is added by `-z`, even after the last record): a git message cannot contain NUL, so counting fields is unambiguous. `%ad`/`%cd` with `--date=raw` give `<seconds> <+hhmm>`, i.e. also the timezone that `%at`/`%ct` lack. `%s` is the first paragraph joined on one line, `%b` the rest (trailing newlines removed by the parser) |
| Refs | `git for-each-ref --format=%(refname)%00%(objectname)%00%(*objectname)%00%(upstream)%00%(upstream:track,nobracket)%00%(symref)%00 refs/heads/ refs/remotes/ refs/tags/ refs/stash` (constant `REFS_ARGS`) | 6 fields per ref, **each terminated by NUL**; git adds a newline after every record (there is no `-z`), so the parser splits only on NULs and strips the newline at the start of the following record: ref names cannot contain NUL or newline, but `%(upstream)` comes from the config and may contain a newline. Symbolic refs (`origin/HEAD`), with a non-UTF-8 name or outside the four kinds: ignored. Annotated tags: `target` from `%(*objectname)` (climbing tags of tags present in the list), `tag_object` from `%(objectname)`. Ahead/behind: empty ⇒ 0/0, `gone` ⇒ `None` |
| Status | `git status --porcelain=v2 -z --branch --untracked-files=all` | stable format by definition |
| Files of a commit | `git diff-tree -z -r --raw --no-abbrev -M --no-commit-id <first parent> <commit>` (constant `DIFF_TREE_ARGS`; `--root <commit>` for a root commit) | one record per file: `:<modes> <id> <status>` and one or two paths (source and destination for `R`/`C`), **each terminated by NUL**. Merges are compared with the first parent (the files brought into the branch), as `commit.show` does |
| Diff | `git -c core.quotePath=false diff --no-color --no-ext-diff --no-textconv --find-renames --full-index --src-prefix=a/ --dst-prefix=b/ --submodule=short --no-relative --unified=3` (constant `DIFF_ARGS`, plus `--cached` or the revisions) | unified diff format, read by `parse_diff` into `Diff` → `FileDiff` → `Hunk` → `Line` (lines as bytes, with old/new line numbers and the "no newline" marker per line). The arguments neutralize configuration that would change the format: colors, external programs and `textconv`, `diff.noprefix`/`diff.mnemonicPrefix` (fixed prefixes `a/` `b/`), `diff.renames=false`, `diff.submodule=log` (a submodule stays a `Subproject commit <id>` line), `diff.relative`, `diff.context` (always three lines of context: the diff shown, its digest and the per-line patches have the same context, and a hunk without leading or trailing context necessarily touches the start or end of the file); `--full-index` gives full ids. Names in quotes with C escapes (git quotes them for `"`, `\`, newline, tab... even with `core.quotePath=false`) decoded into bytes; the number of lines of each hunk comes from the header, so lines like `--- x` inside a hunk are not mistaken for headers. Conflicted files (`diff --cc`, `* Unmerged path`) are reported without hunks. `Diff::to_patch` rewrites the output identically, basis of the patch builder |

Parsers receive `&[u8]`, not `&str`: file names and messages may not be UTF-8.
They are converted with `String::from_utf8_lossy` only for display, never for paths
passed back to git (keep them as bytes or `OsString`).

### Graph lanes

Pure function: commits in topological order (most recent first), result one row per commit with
the commit's column and the segments toward the next row.

1. A vector of active lanes is kept; each lane "waits" for an `Oid`.
2. For each commit: its column is the first lane waiting for it, or a free lane.
   The other lanes that were waiting for the same commit close (they converge here).
3. The commit's lane now waits for the first parent. Every other parent goes on a lane already
   waiting for it, or on a free lane (a branch opens).

Invariants to verify with property tests: every parent–child edge is drawn exactly
once; no commit occupies two columns; free lanes are reused (limited width).


The patch builder for line staging is in [git/stage.md](git/stage.md).

## 4. Engine (L5)

### Backend

```rust
pub trait GitBackend: Send {
    fn head(&self) -> Result<Head, EngineError>;
    fn refs(&self) -> Result<Vec<RefInfo>, EngineError>;
    fn log(&self, query: &LogQuery) -> Result<Vec<Commit>, EngineError>;
    fn status(&self) -> Result<Status, EngineError>;
    fn diff(&self, spec: &DiffSpec) -> Result<Diff, EngineError>;   // DiffSpec: Worktree | Index | Commit { rev }, each with optional `path`
    // Writes (`&mut self`: the MemoryBackend keeps its state inside), one per step:
    // They return the index backup ref, if they saved one (§7).
    fn stage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError>;
    fn unstage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError>;
    // Stage/unstage of lines (`LineSelection`: path, lines, digest, direction).
    fn apply_lines(&mut self, selection: &LineSelection) -> Result<Option<String>, EngineError>;
    // Commit (`CommitRequest`: message, amend, confirm; `CommitOutcome`: commit, previous,
    // branch, backup).
    fn commit(&mut self, request: &CommitRequest) -> Result<CommitOutcome, EngineError>;
    // Undo/redo (§7): moves HEAD and refs from `expected` to `target`, all or none, only if they currently
    // equal `expected`; returns the backup refs created.
    fn restore_refs(&mut self, request: &RefRestore) -> Result<Vec<String>, EngineError>;
    // Branches (S24): short names, already checked by the command.
    fn unmerged_count(&self, tip: &Oid, into: &[Oid]) -> Result<usize, EngineError>;
    fn create_branch(&mut self, name: &str, target: &Oid) -> Result<(), EngineError>;
    fn checkout(&mut self, target: &HeadState) -> Result<RestoreOutcome, EngineError>;
    fn delete_branch(&mut self, name: &str, expected: &Oid) -> Result<BranchDeleted, EngineError>;
    fn rename_branch(&mut self, from: &str, to: &str, expected: &Oid)
        -> Result<RestoreOutcome, EngineError>;
    // Remotes (S25): names of configured remotes and fetch (slow: progress and cancellation from
    // `job`, §6). `FetchRequest`: remote (already checked), branch; `FetchOutcome`: messages.
    fn remotes(&self) -> Result<Vec<String>, EngineError>;
    fn fetch(&mut self, request: &FetchRequest, job: &JobContext)
        -> Result<FetchOutcome, EngineError>;
    // Pull and push (S26). Reads: configured upstream (`UpstreamConfig`: remote, merge,
    // tracking), pull configuration (`PullSettings`: rebase, ff, raw values), commits of
    // `tip` outside `exclude` (for the confirmation of a force).
    fn upstream(&self, branch: &str) -> Result<Option<UpstreamConfig>, EngineError>;
    fn pull_settings(&self, branch: &str) -> Result<PullSettings, EngineError>;
    // All values of a key (`git config --get-all`): `branch.<b>.pushRemote`,
    // `remote.pushDefault`, `remote.<r>.mirror`, `remote.<r>.fetch`.
    fn config_values(&self, key: &str) -> Result<Vec<String>, EngineError>;
    fn missing_commits(&self, tip: &Oid, exclude: &[Oid], limit: usize)
        -> Result<Vec<Commit>, EngineError>;
    // The integration after the fetch (`PullRequest`: branch, current, expected, upstream,
    // mode = FastForward | Merge { no_ff } | Rebase { merges }, message), the undo/redo of a
    // pull of the active branch (like `git reset --keep`) and the push (`PushRequest`: remote,
    // branch, remote_branch, set_upstream, lease, backup).
    fn pull(&mut self, request: &PullRequest) -> Result<PullOutcome, EngineError>;
    fn reset_branch(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError>;
    fn push(&mut self, request: &PushRequest, job: &JobContext)
        -> Result<PushOutcome, EngineError>;
    // Remote management (S27): what a remote has (`RemoteSnapshot`: `remote.<r>.*` of the
    // repo file, remote branches with commits, `refs/remotes/<r>/HEAD`, upstream/pushRemote
    // that use it, `remote.pushDefault`), `git remote add`, and the configuration removed or
    // restored by the engine's `remove_remote`/`restore_remote` (remote branches go through
    // `restore_refs`).
    fn remote_snapshot(&self, name: &str) -> Result<RemoteSnapshot, EngineError>;
    fn create_remote(&mut self, name: &str, url: &str) -> Result<(), EngineError>;
    fn remove_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<String>;
    fn write_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<String>;
    // Stash (S28): the list from the reflog of `refs/stash` (`StashEntry`: index, name, id, base,
    // message, branch, time, untracked); push (`StashPush`: message, include_untracked,
    // keep_index, paths), apply (`StashApply`: id, restore_index; `StashApplied`: conflicts),
    // drop (`StashDrop`: id, backup type) and store (the undo of a drop). A stash always arrives
    // as an id: the position is recomputed by the backend.
    fn stashes(&self) -> Result<Vec<StashEntry>, EngineError>;
    fn stash_push(&mut self, request: &StashPush) -> Result<StashEntry, EngineError>;
    fn stash_apply(&mut self, request: &StashApply) -> Result<StashApplied, EngineError>;
    fn stash_drop(&mut self, request: &StashDrop) -> Result<StashDropped, EngineError>;
    fn stash_store(&mut self, id: &Oid, message: &str) -> Result<(), EngineError>;
    // Merge (S29): `MergeRequest` (branch, expected, target, target_name, mode = FastForward |
    // Merge { no_ff }, message); `MergeOutcome` (after, backup, in_progress, conflicts, message,
    // warnings): a merge stopped on conflict stays in progress. The abort first saves index and
    // worktree (`MergeAborted`: backup, files). `OperationState` (operation, incoming =
    // MERGE_HEAD / CHERRY_PICK_HEAD / REVERT_HEAD, message = MERGE_MSG, onto, rebase_branch)
    // for `repo.operation`. `merge.continue` uses `commit`.
    fn merge(&mut self, request: &MergeRequest) -> Result<MergeOutcome, EngineError>;
    fn merge_abort(&mut self) -> Result<MergeAborted, EngineError>;
    fn operation_state(&self) -> Result<OperationState, EngineError>;
    // Cherry-pick and revert of one commit (S31, git/cherry-pick.md): `PickRequest` (kind, branch,
    // expected, commit, parent, mainline, message) → `MergeOutcome`; the abort like the merge.
    // `*.continue` uses `commit` with `CommitRequest::concluding`.
    fn pick(&mut self, request: &PickRequest) -> Result<MergeOutcome, EngineError>;
    fn pick_abort(&mut self, kind: PickKind) -> Result<MergeAborted, EngineError>;
    // then rebase, ...
}

pub trait RepoOpener: Send + Sync {
    fn open(&self, path: &str) -> Result<Box<dyn GitBackend>, EngineError>;
}
```

- `MemoryBackend` (in the engine): a fake repo built in code. It serves the command and UI tests. Writes modify the in-memory state. It also has in-memory remotes
  (`add_remote`, `set_branch_on_remote`, `insert_remote_commit`, `fail_fetch` to simulate
  `Auth`/`RemoteUnreachable`): the fetch copies reachable commits and updates
  `refs/remotes/<remote>/<branch>` (without prune). In the sample repo `origin` has one more
  commit on `feature/graph` (a pull of `feature/graph` fast-forwards it) and `main` is ahead by one
  commit relative to `origin/main` (a push publishes it). Pull and push (S26): file-by-file merge
  (a file changed on both sides is a conflict), rebase of commits without merges, push that copies
  commits into the in-memory remote; `set_pull_settings` gives `pull.rebase`/`pull.ff`,
  `fail_pre_push` simulates a `pre-push` hook that rejects, `branch_on_remote` reads the remote. Remotes (S27):
  `remote.<r>.url`/`.fetch` come from the in-memory remote, other keys from `set_config`,
  upstreams from `set_upstream`; no symbolic refs. When a remote is removed its "server"
  (remote branches and commits) is kept aside by URL, and a remote added or restored with the
  same URL finds it again. Stash (S28): each stash keeps the status entries and the content of the
  files set aside, plus a commit (with the files changed relative to the base, so `commit.show`
  on the id works); `set_stash` adds a stash without files. Stricter than git: a file
  changed by the stash and by HEAD relative to the base is always a conflict (without markers), and
  with `paths` the stash contains only the chosen files (git puts the whole index in, see
  `stash.push`). Deleted stashes are kept aside by id, for the undo of the drop;
  `fail_stash_drop` simulates a drop that fails (for a pop applied and not removed). Merge (S29):
  the file-by-file merge (`merge_three_way`, also used by the pull in merge): a conflict leaves the merge in progress (`u` entries
  with markers in the worktree, `MERGE_HEAD` and `MERGE_MSG` in memory) and a commit with a merge in
  progress makes the merge commit; see [git/merge.md](git/merge.md).
- `CliBackend` (in `project-git`): runs `git` (§5).

### Session

`Session` is the session of one repo: it holds the open `Box<dyn GitBackend>` (or none) and the
**operation journal** (`Journal`) for undo (§7), which belongs to the open repo: opening a
repo, even the same one, starts an empty journal (entry ids continue). `repo_and_journal()`
gives backend and journal together to the commands that write and record the operation. The cache of
what the UI shows (status, refs, first page of the log) for now lives in the UI. The `Session`
with the document snapshots of the original example was removed (S08): undo and redo
exist only on the journal.

### Commands

Every operation is a command with a stable id and JSON parameters. Reads return data
as JSON, so CLI and MCP use it directly. Contract, `CliBackend`, undo and backup of
every command are in the file of its function; a new command is added to the file of
its function (or to a new file in `impl-docs/git/`, listed here).

| Commands | Where |
| --- | --- |
| `repo.open`, `repo.status`, `repo.refs`, `repo.log`, `commit.show`, `diff.worktree` / `diff.index` / `diff.commit` | [git/reading.md](git/reading.md) |
| `index.stage` / `index.unstage`, `index.stage_lines` / `index.unstage_lines` | [git/stage.md](git/stage.md) |
| `worktree.discard` | [git/discard.md](git/discard.md) |
| `commit.create` | [git/commit.md](git/commit.md) |
| `branch.create`, `branch.checkout`, `branch.delete`, `branch.rename` | [git/branch.md](git/branch.md) |
| `remote.fetch`, `remote.pull`, `remote.push` | [git/sync.md](git/sync.md) |
| `remote.list`, `remote.add`, `remote.remove` | [git/remotes.md](git/remotes.md) |
| `stash.list`, `stash.push`, `stash.apply` / `stash.pop`, `stash.drop` | [git/stash.md](git/stash.md) |
| `merge.start`, `merge.continue`, `merge.abort`, `repo.operation` | [git/merge.md](git/merge.md) |
| `commit.cherry_pick`, `commit.revert`, `cherry_pick.continue` / `abort`, `revert.continue` / `abort` | [git/cherry-pick.md](git/cherry-pick.md) |
| `conflict.show`, `conflict.resolve` | [git/conflicts.md](git/conflicts.md) |
| `edit.undo` / `edit.redo`, `edit.history` | below and §7 |

| Id | Parameters | Notes |
| --- | --- | --- |
| `edit.undo` / `edit.redo` | `{}` | on the operation journal (§7). Writes. `undo` undoes the last entry made, `redo` redoes the last undone; without entries `NotAllowed`. Refs must still equal what the operation (or the undo) left, otherwise `UndoConflict { command, refs }` and nothing changes (the entry stays). Undo and redo of `branch.checkout` are safe checkouts: with conflicting local changes `CheckoutConflict` (the entry stays). Merge, rebase, cherry-pick, revert, bisect or `git am` in progress (even a stopped sequencer, `.git/sequencer` without `CHERRY_PICK_HEAD`/`REVERT_HEAD`): `NotAllowed`. Result: `{"undone" \| "redone": JournalEntry, "backups": [string], "warnings": [string], "status": Status \| null, "can_undo": bool, "can_redo": bool, "next_undo": string \| null, "next_redo": string \| null}` (`warnings`: warnings discovered after a successful move, e.g. HEAD changed by another process at that instant; usually empty. `status` `null`, with `status_error`, if it cannot be re-read: the operation is done, it must not be repeated) |
| `edit.history` | `{}` | read-only: `{"done": [JournalEntry], "undone": [JournalEntry], "can_undo", "can_redo", "next_undo", "next_redo"}`; in both lists the last entry is the next one for `undo` / `redo` |

The catalog grows with the steps. Ids, once published, are not renamed.

## 5. `git` backend (L6, `project-git`)

Rules for every call to git:

- **Never a shell.** `std::process::Command` with separate arguments.
- Always `git -C <repo> -c core.quotePath=false -c color.ui=false ...`.
- Before user-supplied values (branch names, revs): `--end-of-options`, so a name
  starting with `-` does not become an option. Before paths: `--`. Paths are passed
  literally (`--literal-pathspecs`: no globs or magic pathspecs like `:(top)`), and inherited
  `GIT_*_PATHSPECS` variables are removed.
- Environment: `GIT_TERMINAL_PROMPT=0` (git must never wait for input from a terminal that is
  not there), `LC_ALL=C` for the error messages to be recognized. The rest of the environment is inherited
  (SSH agent, credential helper).
- Commit messages via stdin (`git commit -F -`), never as an argument (details in [git/commit.md](git/commit.md)).
- Path lists for writes go via stdin (`--pathspec-from-file=-
  --pathspec-file-nul`, with `--literal-pathspecs`): no command-line limit and no
  path read as an option. An empty list never reaches git (for many commands it would
  mean "the whole repo").
- Reads do not write the index: `--no-optional-locks`, and for `git diff` also
  `-c diff.autoRefreshIndex=false` (otherwise it rewrites the index to update `stat` data).
- Environment variables always removed, besides those of the repo (`GIT_DIR`, `GIT_WORK_TREE`,
  `GIT_INDEX_FILE`, ...) and of pathspecs: the inherited configuration `GIT_CONFIG_PARAMETERS`,
  `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_*`/`GIT_CONFIG_VALUE_*` (an injected `core.worktree` would
  redirect writes).
- Known, intended limits: the repo's and user's configuration can make git run commands
  (`clean`/`smudge` filters, `core.fsmonitor`, hooks such as `reference-transaction`): it is the
  user's configuration and is respected, as in every client that uses system git; opening
  an untrusted repo is like running git in that repo. `index.stage` of a directory that is a
  nested repo adds a gitlink (like `git add`, which flags it only with a warning).
- Fetch, pull and push: [git/sync.md](git/sync.md); remotes: [git/remotes.md](git/remotes.md); stash: [git/stash.md](git/stash.md); merge: [git/merge.md](git/merge.md).
- A command that modifies the repo is marked in `CommandSpec::writes` (CLI `commands`, MCP
  `list_commands`), so the UI disables it during a job and an agent knows what it writes.
- When the repo is opened the git version is checked (minimum 2.30) and a clear error is given if it
  is older.
- git errors become variants of `EngineError` with the stderr text, so UI and agents
  see the real reason.

## 6. Slow commands and non-blocking UI

`Registry::run` stays synchronous: it is simple to test and to use from CLI and MCP.

The UI does not call it directly on its thread for slow commands. `Platform` gets a
method to run background work:

```rust
fn spawn(&self, job: Box<dyn FnOnce() + Send>);
```

- Desktop: a thread (or a small pool).
- Tests: a platform may run the job immediately, inside `spawn`.

The session sits behind an `Arc<Mutex<...>>` shared with the job; the result returns to the UI with
a channel and the UI requests a new frame (`ctx.request_repaint()`). During a job the UI shows
progress and disables the commands that modify the repo. Fetch and push progress is
read from stderr with `--progress`. `remote.pull` and `remote.push` (S26) use the same
`JobContext` as `remote.fetch`: the pull passes it to the fetch (the only cancellable part; a
cancellation seen after the fetch stops the command before integrating, never in the middle of a merge or
a rebase), the push uses it for the whole `git push` (cancelled midway, the remote may already
have updated the branch).

### Progress and cancellation (S25)

`Registry::run` stays synchronous and without callbacks. For slow commands there is
`Registry::run_with(id, session, params, job: JobContext)`: for the duration of the command the
session holds `job` (`Session::job()`), the command passes it to the backend, and afterwards an empty
context returns (`Registry::run` is `run_with` with `JobContext::new()`: no progress, never
cancelled). This way the extension does not change the signature of commands or `Handler`s.

- `JobContext::new().with_progress(sink).with_cancel(token)`: `sink` is a
  `Fn(&Progress) + Send + Sync`, called from the job thread; `token` is a `CancelToken`
  (shared `Arc<AtomicBool>`: `cancel()` from any thread, `is_cancelled()`).
- `Progress` (serializable): `{"phase", "remote", "percent", "current", "total", "done",
  "text"}` from a git line (`Receiving objects:  45% (9/20), 1.2 MiB | 2 MiB/s` → phase
  `Receiving objects`, 45, 9, 20; `remote: Enumerating objects: 5, done.` → `remote: true`,
  `current` 5, `done`). Phases follow one another (`Enumerating`, `Counting`, `Compressing` on the
  remote, then `Receiving`/`Unpacking objects` and `Resolving deltas` locally); the UI shows
  the last one received. Git writes at most a few dozen per second: the sink should only store it
  (e.g. in a `Mutex`) and request a frame, not draw.
- **The UI** (`Executor`): for `remote.fetch` it calls `run_with` in the `Platform::spawn` job with
  a sink that stores the ticket's last `Progress` and calls `wake`, and keeps the ticket's `CancelToken`
  for a "Cancel" button. The session stays locked by the job for the whole fetch
  (§6): reads sent in the meantime wait for the end. The `MemoryBackend` finishes
  immediately (fake events with the same shape).
- **Cancellation**: the `CliBackend` checks the token every 50 ms while git runs; if it is
  cancelled it sends SIGTERM to the fetch's process group (git, ssh, `git-remote-http`,
  `index-pack`: git removes its own `.lock` files), after 3 seconds SIGKILL, and the command gives
  `EngineError::Cancelled`. Refs already updated by git stay (git updates them at the end, each
  ref under its lock), objects already received stay in the database (a `gc` removes them):
  no local ref, index or worktree is touched. A token cancelled before the start gives
  `Cancelled` without starting git.
- CLI: with stderr on a terminal it prints progress to stderr (a line rewritten with `\r`);
  stdout stays JSON. MCP server: uses `Registry::run`, no progress.

## 7. Undo and backup

The operation journal records, for every command that moves refs, the before and after values:
`HEAD`, the branch involved, the oid. `edit.undo` returns the refs to the previous values, only if in the
meantime they have not changed (otherwise a clear error, no overwriting).

### Operation journal (S22)

Each entry (`JournalEntry`, JSON `{"id", "command", "description", "head": {"before",
"after"}, "refs": [{"name", "before", "after"}], "backup", "undo_backups", "worktree",
"branch_config", "remote", "stash"}`) has the
command id, a description for people (`Commit a0744b4 «subject»`, `Amend b12c3d4 «subject»
(was a0744b4)`), HEAD before and after (`HeadState`: `{"branch": "refs/heads/main"}` or
`{"detached": oid}`), the moved refs with full name and oid before and after (`null` = the ref
did not exist / was deleted: several refs per entry, for branches, renames, merges, rebases and resets
in the future), the backup saved by the operation (e.g. that of `amend`) and the backups created by undo and
redo. Today it is recorded by `commit.create` (normal and `amend`; with detached HEAD no ref,
HEAD moves; an `amend` that produces the same commit and a `PostCommit` do not go in), the
`branch.*` commands (see [git/branch.md](git/branch.md)), `stash.push` and `stash.drop` (see [git/stash.md](git/stash.md)), the concluded merge (`merge.start`, `merge.continue`: see [git/merge.md](git/merge.md); a merge stopped on conflict and `merge.abort` do not), concluded cherry-pick and revert (see [git/cherry-pick.md](git/cherry-pick.md)) and `worktree.discard` (no ref, HEAD equal before and after, the worktree backup in the
`"worktree"` field, see [git/discard.md](git/discard.md)). Stage and unstage move no refs and do not go into the journal: their recovery is the
index backup. A new entry clears the undone entries (the redo); the journal keeps at
most 100 entries.

Choices:

- **In memory, per open repo.** It is not saved to disk: the CLIs run one session per
  invocation, so several `--cmd` of the same invocation share the journal and different
  invocations do not; the MCP server and the app keep it as long as the repo stays open. Persisting it (e.g. in
  `.git/project/journal`) would mean handling several instances on the same repo and formats of
  different versions; to avoid losing data it is not needed, because undone commits stay in backup
  refs and every move is in the reflog (`project: undo ...`). It can be added later without
  changing the commands: the check on expected values makes even an old entry safe.
- **Undo of a commit = `git reset --soft`.** Only refs move: index and worktree are not
  touched, so the undone commit's changes reappear staged relative to the new HEAD
  and `edit.redo` returns the branch to the same commit. With `amend`, the undo returns the branch to the
  rewritten commit (which exists: there is the `amend` backup) and what `amend` had added
  stays staged; no check on the index is needed, because nothing writes it. The undo of the first
  commit deletes the branch (`delete` with the old value): HEAD stays on the branch, which returns
  without commits, and the files stay staged as added. Deleting the branch also deletes its
  reflog (like `git update-ref -d`); the entry in HEAD's reflog remains.
- **Atomic check.** `GitBackend::restore_refs` receives a `RefRestore` (expected and new HEAD and
  refs). First: `RefRestore::check` (operations in progress, valid ref names, HEAD branch changes
  refused: undo and redo of checkout and rename go through `GitBackend::checkout` and
  `GitBackend::rename_branch`, see [git/branch.md](git/branch.md)), then current HEAD and refs compared with the expected ones
  (`RefRestore::conflicts`: if anything changed, `UndoConflict` with one ref per line and nothing
  changes). The `CliBackend` writes with a transaction `git update-ref -m "project: undo <description>"
  -z --stdin`: `start`, then `update <ref> <new> <old>` / `create <ref> <new>` /
  `delete <ref> <old>` and, with detached HEAD, `option no-deref` + `update HEAD <new>
  <old>`, finally `commit`. Git checks the old values under the refs' lock: a ref
  moved between the check and the transaction makes it all fail, and the error returns
  `UndoConflict`. With HEAD on a branch the branch is updated (HEAD does not detach) and git writes
  the entry in HEAD's reflog too. A `reference-transaction` hook that rejects gives git's error and
  nothing changes.
- **Limits on HEAD.** HEAD is not verified inside the transaction. With HEAD on a branch, that
  it still points to that branch is checked beforehand: the instant between the check and the
  transaction remains. With detached HEAD `option no-deref` + `update HEAD` checks only the value: a
  `git checkout main` made at that instant by another process, with `main` on the expected
  commit, would be detached again (`main` does not move, no commit is lost).
  `symref-verify` (git ≥ 2.46) is not used: it would serve only the branch case, but verifying
  HEAD in the same transaction that updates the branch it points to collides with git's check
  on multiple updates of HEAD (git adds a reflog entry for HEAD by itself), and CI
  does not have that git to test it. Instead, after the transaction the `CliBackend` re-reads
  HEAD and counts the new entries of HEAD's reflog (at most one, ours): if HEAD is not
  the one requested or changed in the meantime, the result says so in `warnings`
  (`RestoreOutcome::warnings`). Without HEAD's reflog only the comparison with HEAD remains.
- **Backup of undone commits.** Before moving the refs, every commit that would be abandoned
  (`RefRestore::abandoned`: expected values that are not among the new ones) and that does not remain
  reachable from a new value (`merge-base --is-ancestor`) nor from another non-symbolic ref,
  backups included (`for-each-ref --format=%(refname)%00%(symref) --contains`, a single
  command for all commits; symbolic refs are discarded as in `amend_impact`: a
  `refs/heads/master -> refs/heads/main` would appear with `main`'s value before the
  transaction and afterwards would hold nothing), is saved in `refs/project/backup/undo-<seconds>-<nanoseconds>`
  (`redo-...` for the redo; created only if it does not exist, like the other backups). This way gc does not
  delete it and the redo remains possible even after a restart (by hand, from the ref). If the backup
  fails nothing is moved. The result lists them in `backups` and the entry in `undo_backups`.
  Reachability checks use `--no-replace-objects`: the real history counts, the one
  gc sees. The protections given by `refs/stash`, remote branches and index backups
  (`refs/project/backup/index-*`, which have HEAD as parent) are **volatile**: a stash
  is dropped, a remote updates with a fetch, an index backup can be deleted. Today no extra backup is
  created for them; a future cleanup of `refs/project/backup/*` must not
  treat these refs as independent (before removing a backup it must be checked that the commit
  is not held only by volatile refs or by other backups being removed).
  The `MemoryBackend` makes the same checks, records backups in `commit_backups`
  (`refs/project/backup/undo-<n>`) and recomputes the status entries relative to the new HEAD with the
  same index and worktree (staged renames become deletion and addition again).


Undo and backup of individual commands are in the function's file: discarding changes
([git/discard.md](git/discard.md)), index backup of stage and unstage ([git/stage.md](git/stage.md)),
`amend` backup ([git/commit.md](git/commit.md)), branches ([git/branch.md](git/branch.md)), stash
([git/stash.md](git/stash.md)), pull and fetch ([git/sync.md](git/sync.md)), remotes ([git/remotes.md](git/remotes.md)), merge
and the `merge.abort` backup ([git/merge.md](git/merge.md)).

Undo does not cover remote operations: a push cannot be undone from here (the pull can, for its local part: see [git/sync.md](git/sync.md); `remote.add` and `remote.remove` change only the configuration and local remote branches: see [git/remotes.md](git/remotes.md)).

## 8. Interface (L6)

GitKraken-like layout:

- **Top bar**: open repo, fetch/pull/push buttons, undo/redo, state of the job in progress.
- **Left column**: local branches, remotes, tags, stashes ([git/stash.md](git/stash.md)).
- **Center**: commit graph and list, with an "uncommitted changes" row at the top. Virtualized
  list (`ScrollArea::show_rows`): only visible rows are drawn, the log loads
  in pages while scrolling.
- **Right**: details of the selected commit, or stage area (unstaged and staged files,
  commit message) when the changes row is selected.
- **Diff**: below or in place of the graph, with hunk and line selection.

Every interaction produces an `Action` that goes through `Registry::run`, as today.

The Stash section and its panel: [git/stash.md](git/stash.md).

**Operation in progress** (S29, [git/merge.md](git/merge.md)): when `repo.status` has `operation`
(merge, rebase, cherry-pick, revert, bisect, `git am`) a full-width panel appears under the bar,
with the details of `repo.operation` (re-read at every new status, one at a time): what
is coming in, the conflicted files (the first eight; a click opens the stage area with their diff) and
the `continue`/`abort` command buttons when they are not `null`. "Continue the merge…" (active only
with `ready`) opens a dialog with the merge commit message prefilled from `message`;
"Abort the merge…" goes in two steps (without `confirm` the engine responds `ConfirmRequired`, the red
dialog shows the reason, then `"confirm": true`). For operations the UI does not yet conclude
the panel says how to do it from the terminal. During an operation the commit box is
disabled (the merge is concluded with `merge.continue`, which goes into the journal as a merge; during a
bisect you commit normally). The merge is started from the context menu of a branch (non-active local,
remote), of a tag or of a commit: "Merge into «branch»" and "… with merge commit"
(`no_ff`), disabled with the reason if HEAD is detached or an operation is already in progress. The outcome goes
in the status bar (fast-forward, merge commit, already up to date); a merge stopped on conflict
opens the stage area and the panel; a refusal (local changes, unknown rev) opens a
window. The demo repo (`make demo-repo`, `scripts/demo-repo.sh`) has the branch `experiment`,
conflicting with `main` on `src/lib.rs`: «Stash…», then «Merge into «main»» on `experiment`.

## 9. Tests

- **Domain**: unit tests of the parsers with real git output saved as sample files
  (also with non-UTF-8 names, spaces, renames), property tests for the graph lanes,
  snapshots (`insta`) for generated patches.
- **Engine**: commands run on the `MemoryBackend`.
- **`project-git`**: integration tests on real repos created in temporary folders
  (`tempfile`) with a scenario helper. Deterministic environment: `GIT_CONFIG_GLOBAL=/dev/null`,
  `GIT_CONFIG_NOSYSTEM=1`, fixed author and dates (`GIT_AUTHOR_DATE`, `GIT_COMMITTER_DATE`).
  Scenarios to cover over time: empty repo, detached HEAD, conflicted merge, interrupted rebase,
  submodule, worktree, binary files, CRLF line endings, non-UTF-8 names.
- **CLI**: end-to-end tests such as `apps/project-cli/tests/cli.rs`.
- **UI**: unit tests of the views and of the `Executor` on the `MemoryBackend`; the layout is
  checked on the desktop app on Xvfb with the demo repo (`make desktop-start`, `xdotool`,
  `import`).

## 10. Agents

Defined in `.claude/agents/`. Each works on its own layer, starts from
[git/rules.md](git/rules.md) and finishes with `cargo xtask ci --quiet`. The re-checks by
`safety-reviewer` after fixes look only at the diff of the fixes and may use a
lighter model (the agent's `model` parameter).

| Agent | Works on | When to use it |
| --- | --- | --- |
| `git-domain` | `crates/core`, `crates/domain` | types, parsers, graph, patches: pure code |
| `git-engine` | `crates/engine`, `crates/git`, `crates/mcp`, `apps/project-cli` | backend, commands, session, CLI and MCP |
| `egui-ui` | `crates/ui`, `apps/project` | interface and wiring in the apps |
| `git-scenarios` | tests in `crates/git/tests` | scenarios on real repos and edge cases |
| `safety-reviewer` | read-only | review of every step that writes to the user's repo |
