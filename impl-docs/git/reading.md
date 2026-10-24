# Reads: repo, log and diff

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

## Commands

### `repo.open`

Parameters: `{"path"}`.

Uses the injected `RepoOpener`.

### `repo.status`

Parameters: `{}`.

### `repo.refs`.

Parameters: `{}`.

### `repo.log`.

Parameters: `{"skip"?, "limit"?, "rev"?, "all"?, "stashes"?}`.

Paginated (`limit` 200 if missing, result `{"commits", "has_more"}`). `git log --date-order` (children
before parents, then the most recent by committer date; also applies with several starting points,
and the graph lanes §3 handle several tips and several roots). From HEAD, or from `rev` (ref, `HEAD` or
id, after `--end-of-options`); a HEAD without commits gives an empty log. `"all": true`: from all local
branches, remote branches, tags and HEAD (`--branches --remotes --tags` and the HEAD id, so a detached
HEAD appears; a HEAD without commits is skipped): it shows the commits of unmerged branches and those
just downloaded on `origin/*`. `refs/stash`, backups in `refs/project/backup/` and
any other ref outside `refs/heads/`, `refs/remotes/`, `refs/tags/` (e.g. notes) are left out; a tag on a
tree or a blob is ignored. `"stashes": true` (only with `all`, otherwise `InvalidParams`): it also starts
from the stash bases (`StashEntry::base`, the first parent), so the commit the UI
attaches a stash to is always in the graph, even if no branch contains it any more; the stash commits
(and the index and untracked ones that have them as parents) are left out, the list is
`stash.list`. `rev` and `all` together → `InvalidParams`. `skip`/`limit` paginate in the same order,
so the pages are contiguous.

### `commit.show`

Parameters: `{"id"}`.

Metadata and files changed relative to the first parent.

### `diff.worktree` / `diff.index` / `diff.commit`

Parameters: `{"path"?}` / `{"path"?}` / `{"id", "path"?}`.

worktree relative to the index (without untracked files), index relative to HEAD (empty tree if there are
no commits), commit relative to the first parent (empty tree for a root commit, like
`commit.show`). Result: the `Diff` (`{"files": [...]}`). `path` (string or byte array, like
`RepoPath`) limits to the file with that exact path, old or new: a rename stays a rename (the
backend first looks for the other path with `--diff-filter=R`) and paths go to git after `--` with
`--literal-pathspecs`. No size limit

## Blame and file history (S34)

Pure parsers in `crates/domain/src/git/blame.rs` and `crates/domain/src/git/file_log.rs`; commands in
`crates/engine/src/builtin/file.rs`. Both are reads (not in `WRITES`) and slow on large histories
(§6: run in the background by the UI with `Registry::run_with`; the job's `CancelToken` stops git).

### `file.blame`

Parameters: `{"path", "rev"?}`. `path` as `RepoPath` (string or bytes). Without `rev` the file in the
working tree (uncommitted lines have `commit: null`); with `rev` the file at that commit (`UnknownRev`
if not found). Errors → `InvalidParams` naming the path: not a file at `rev` (missing, folder,
submodule; `engine-blame-missing-rev`), not a tracked file of the worktree (missing, deleted from the
worktree, folder, untracked: git wants it in HEAD or in the index; `engine-blame-missing-worktree`),
worktree on a branch without commits (`engine-blame-no-commits`); binary file (NUL in the first 8000
bytes) → `NotAllowed` (`engine-blame-binary`). Cancellable through `Session::job()` (§6):
`Cancelled { command: "file.blame" }`.

`CliBackend`: `git --no-optional-locks -c core.fsmonitor=false -c blame.markIgnoredLines=false -c
blame.markUnblamableLines=false blame --porcelain --no-textconv --no-root --ignore-revs-file= [<oid>]
-- <path>`. `git blame` does **not** accept `--end-of-options` (it reads it as a revision): only the
resolved id reaches it, hexadecimal, so it cannot look like an option. The path after `--` is taken
literally (blame does not use pathspecs). `-w`, `-M`, `-C` are not used (the result matches plain `git
blame`). `--ignore-revs-file=` (empty) clears the files of `blame.ignoreRevsFile`, so no commit is
skipped (`-c blame.ignoreRevsFile=` does **not** work: git sorts the list and the empty value comes
first); a configured file that does not exist still makes git fail, as with plain `git blame`
(`Git` error with its text). `--no-root`: the root commit stays a boundary whatever
`blame.showRoot`; `--no-textconv`: the file's bytes, no configured converter run. The errors
"not a file" are recognized from git's text (`no such path ...`, `Cannot lstat ...`, with
`LC_ALL=C`). The run is cancellable like the fetch (§6: the token is checked every 50 ms, SIGTERM to
the process group).

Domain (`parse_blame`, on the bytes of `--porcelain`):

```rust
pub struct Blame { pub lines: Vec<BlameLine>, pub commits: Vec<BlameCommit> }
pub struct BlameLine { pub text: ByteText, pub commit: Option<usize>, pub orig_line: u32 }
// commit: index in `commits`, None for "Not Committed Yet" (the all-zero id)
pub struct BlameCommit { pub id: Oid, pub author: Signature, pub committer: Signature,
    pub summary: String, pub path: RepoPath /* `filename` */, pub previous: Option<(Oid, RepoPath)>,
    pub boundary: bool }
```

Line numbers are the index in `lines` + 1. Each commit appears once in `commits` (porcelain prints its
headers only the first time), unless git blamed lines from it under more than one name (porcelain
then repeats `filename`): one entry per name. `filename`/`previous` are C-quoted by git and decoded
to bytes. Porcelain always ends the last line with `\n`, so whether the file has a final newline is
not known from the blame. Result of the command: the `Blame` plus `{"path", "rev": oid | null}`.

### `file.log`

Parameters: `{"path", "rev"?, "skip"?, "limit"?, "follow"?}` (`limit` 200 if missing, `follow`
`true` if missing). The commits that change the file, newest first, from HEAD or `rev`; with
`follow` across renames (`--follow`: git allows it with a single path only). Result `{"commits":
[{"commit": Commit, "path", "old_path": path | null, "status": Change}],
"has_more"}` (`FileLogEntry`; `status` is `Added`, `Modified`, `Deleted`, `Renamed`, `Copied` or
`TypeChanged`, merges compared with the first parent): `path` is the name of the file in that commit, `old_path` the previous one for a rename
or copy, so the UI can show the diff of that commit for the right path (`diff.commit` with `path`).

`CliBackend`: `git --literal-pathspecs log <FILE_LOG_ARGS> (--follow --find-renames | --no-follow
--no-renames) [--skip=<n>] --max-count=<n> --end-of-options <oid> -- <path>` (`LOG_ARGS` plus
`--name-status --diff-merges=first-parent --root --no-relative`: without `--diff-merges` a merge has
no entry, without `--root` and with `log.showRoot=false` the root commit is hidden); the explicit
follow/renames options make the result independent of `log.follow` and `diff.renames=copies`.
Merges are kept (git shows a merge only if it changed the file with respect to every parent, like
`git log -- <path>`). **With `--follow` git applies `--skip` before seeing the renames** (the commits
before a rename vanish from the later pages): the backend asks for `skip + limit` commits and drops
the first `skip` itself; without follow `--skip` goes to git. A `rev` that does not resolve →
`UnknownRev`; HEAD without commits or a path never present → empty. Cancellable like `file.blame`.
Domain: `parse_file_log` on that output (records of `LOG_FORMAT` followed by the `--name-status -z`
entries).

`MemoryBackend` (`crates/engine/src/git/memory/file.rs`): blame by walking the **first-parent**
history version by version (longest common subsequence of lines; renames from the changes of each
commit): unlike git, a line brought in by a merge is blamed on the merge. File log by comparing the
file with the first parent (the recorded changes), renames followed when `follow`; a merge only if
the file differs from every parent. Both check the cancellation token only at the start.

### UI

State in `crates/ui/src/file.rs` (`FileBox`, tested on the `MemoryBackend` in `file_tests.rs`),
drawing in `crates/ui/src/view/file_view.rs`. "Blame" and "File history" are in the context menu of
a file of the commit details (blame at that commit, history from it; blame disabled for a deleted
file or a submodule) and of the stage area (blame of the worktree, history from HEAD; disabled for
untracked and deleted files). The view replaces the graph (like the conflict editor) with the tabs
Blame and History; Esc or "Close" goes back. Only the tab in view is read, once; every request has a
number, a replaced one (other file, "Follow renames" toggled, view closed) is cancelled with
`Executor::cancel` and its answer discarded. Blame: rows of the same height in
`ScrollArea::show_rows`, a fixed gutter with the commit (short id, date, author, summary) on the first
line of each run of consecutive lines from the same commit (and on the first visible line),
alternating background per run, "Not committed yet" for `commit: null`; hover gives the full commit,
click selects it in the commit list (`Action::SelectCommit`). History: two-line rows with the status
letter, the path when it differs from the file shown and "renamed from X"/"copied from X"; "Load
more" asks for the next page with `skip`; a click opens `diff.commit` with the entry's `id` and
`path` in the diff view beside the list. Errors of the engine (`InvalidParams`, `NotAllowed` for a
binary file) are shown in the view with "Retry".
