# Reads: repo, log and diff

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

## Commands

### `repo.open`

Parameters: `{"path"}`.

Uses the injected `RepoOpener`. Result `{"path", "head", "commit_graph": bool}`: `path` is the absolute worktree root
(`GitRead::root`, `git rev-parse --show-toplevel`), so the same repo opened from a subfolder or
a relative path gives the same `path` (the given one on the `MemoryBackend`); `commit_graph` is
`true` when git reads a commit-graph for the repo (`GitMaintenance::commit_graph` →
`CommitGraphState::in_use`, see `repo.optimize` below); a failed check gives `false`, the repo
opens anyway.

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

### `repo.search`

Parameters: `{"query", "in"?, "path"?, "all"?, "stashes"?, "skip"?, "limit"?}` (S48). A read
(`Access::Read`). Result `{"ids": [oid], "total": number, "has_more": bool}`: the page `skip..skip + limit`
(`limit` 200 if missing) of the ids of the matching commits, in the order of `repo.log` with the same
`all`/`stashes` (from HEAD, or from all refs; `stashes` without `all` → `InvalidParams`), so the UI
highlights the matches in the graph instead of filtering it; `total` = all the matches, `has_more` =
others after the page. Pages are contiguous.

- `query`: the text, taken **literally** (never a regex) and case-insensitive for ASCII letters
  (`LC_ALL=C`); empty or with a newline/NUL → `InvalidParams` (git would split it into one pattern per
  line, ORed).
- `in`: any of `"message"` (subject and body), `"author"` (name or email, as written in the commit:
  no mailmap), `"id"` (prefix of the full id, any case; only a hexadecimal text can match); default
  all three, empty → `InvalidParams`. A commit matches if **any** field matches (OR).
- `path` (`RepoPath`): only the commits that change that path, or anything under it if a folder,
  with respect to **at least one** parent (`--full-history`: commits of side branches and merges that
  bring a change in count). **No rename following** (`--follow` is not used: git allows it with one
  path only, applies it badly to several starting points and does not combine it with the history of
  all refs): a rename matches both the old and the new name, the commits before it only the old one.
  `file.log` follows renames.

Slow on long histories (several full walks, §6): cancellable through `Session::job()`
(`Cancelled { command: "repo.search" }`). HEAD without commits (without `all`) → empty.

`CliBackend` (`crates/git/src/backend/search.rs`): git ANDs `--author` with `--grep`, so each field is
a separate `git --literal-pathspecs log --format=tformat:%H --no-show-signature --no-color` run over
the range of `repo.log` (`CliBackend::log_range`) and the ids are merged here, in the order of one
unfiltered `--date-order` run (a filtered `--date-order` run with a path could order the commits
differently, since git sorts only the commits it keeps):

- order: `--date-order`; the id prefix is checked on these ids;
- message: `--fixed-strings --regexp-ignore-case --encoding=UTF-8 --grep=<text>`;
- author: the same with `--no-mailmap --author=<text>`;
- path: `--full-history --no-follow ... -- <path>` (`--no-follow`: independent of `log.follow`).

The text reaches git only as the value of `--grep=`/`--author=` (cannot become an option); only `%H`
comes out, so non-UTF-8 messages and names do not affect the parsing. `MemoryBackend`: the same rules
on `repo.log` (`contains_literal`, `SearchQuery::id_matches`, trees compared with each parent).

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

## Huge repos: the commit-graph (S36)

### Measurements

Linux kernel (1.5M commits), git 2.56, release `project-cli`; `target/bench/linux` = worktree
clone without checkout whose objects are in `target/bench/linux.git` (`--filter=tree:0` bare clone,
via `objects/info/alternates`), `GIT_NO_LAZY_FETCH=1`:

| `repo.log` | without commit-graph | with commit-graph |
| --- | --- | --- |
| `{"limit":500}` (first page) | ≈ 15.8 s (14.3 s) | 42 ms (43-46 ms) |
| `{"all":true,"stashes":true,"limit":500}` | ≈ 15.8 s (14.2 s) | 1.36 s (1.33-1.38 s) |
| `{"skip":1000000,"limit":500}` | ≈ 15.8 s (14.7 s) | 0.96 s (0.86-0.91 s) |
| writing the graph, once | | ≈ 18.8 s (`repo.optimize`: 19.5 s) |

First figures: `git` before S36; in brackets: re-measured with `repo.optimize` (graph removed from
`linux.git`, written by the command into `linux/.git/objects/info`; same `repo.log` output before and
after).

The cost is all `git log --date-order` (plain `git log` = 22 ms): without generation numbers git
must parse every reachable commit before printing the first one (children before parents). The same
would hold for any other reader (`gix` was considered and dropped: the order needs the whole walk
anyway); the fix is the commit-graph, which git itself writes in `git gc`/`git maintenance`.

### `repo.optimize`

Parameters: `{}`. A write (`Access::Write`): `git commit-graph write --reachable --progress` (no
`--changed-paths`, no `--split`) through `GitMaintenance::write_commit_graph`. Result `{"commit_graph":
bool}`, the state afterwards (as in `repo.open`; `false` for a repo without commits, where git writes
nothing).

**Why no `confirm`, backup or journal**: it adds or replaces only `objects/info/commit-graph` in the
repo's own object directory, a cache that git derives from the objects (and that `git gc` rewrites
on its own); no ref, index or worktree file changes, so it cannot lose data and there is nothing for
`edit.undo` to restore (no `JournalEntry`).

Refused with `core.commitGraph=false` (`NotAllowed`, `engine-commit-graph-disabled`): git would
neither read nor (it turns out) write the graph. Slow (~19 s on the kernel): progress through
`Session::job()` (phases `Expanding reachable commits in commit graph: <n>` and `Writing out commit
graph in <k> passes: <p>% (<n>/<total>)`), cancellable (`Cancelled { command: "repo.optimize" }`).
Git writes `commit-graph.lock` and renames it at the end; on SIGTERM (our cancel) it removes the
lock and the previous graph stays. Only a SIGKILL (after `TERM_GRACE`, or from outside) leaves a
stale `commit-graph.lock`: the next `repo.optimize` fails with git's text (`Unable to create
'.../commit-graph.lock': File exists`) and the repo keeps working with the previous graph or none.
The lock is never removed by us (it may belong to a running git).

`CliBackend` state (`crates/git/src/backend/maintenance.rs`): `git rev-parse --git-path objects`
(handles linked worktrees; `GIT_OBJECT_DIRECTORY` is cleared from git's environment), then `info/commit-graph` or
`info/commit-graphs/commit-graph-chain` there or in the alternates (`info/alternates`, relative
entries from the listing directory, up to 5 levels like git; quoted entries and
`GIT_ALTERNATE_OBJECT_DIRECTORIES` are not followed); git uses the first graph it finds, so a graph
in an alternate counts (the bench clone above reads the one in `linux.git`), while `repo.optimize`
always writes into the repo's own object directory, which git reads first. Then `git config
--type=bool --get core.commitGraph` (unset = enabled) and `git rev-parse --is-shallow-repository`:
in a shallow repo git ignores the graph and `commit-graph write` exits 0 writing nothing, so
`repo.optimize` refuses (`NotAllowed`, `engine-commit-graph-shallow`). Replace refs
(`refs/replace/`, unless `core.useReplaceRefs=false` or `GIT_NO_REPLACE_OBJECTS`) and a grafts file
(`info/grafts`) also make git ignore the graph and write nothing: `replaced` in the state,
`repo.optimize` refuses (`NotAllowed`, `engine-commit-graph-replaced`). A write over a split chain
left by `git maintenance` replaces it with a single `commit-graph` file (still only a cache). `MemoryBackend`: a flag set by the write,
`core.commitGraph` from `set_config`.

"Big repo": nothing is exposed (an object or commit count is either expensive or a poor proxy:
the slowness depends on the reachable commits without a graph). The UI offers `repo.optimize` when
`commit_graph` is `false` and the first `repo.log` page took noticeably long (e.g. > 1 s).
The notice is described in [ARCHITECTURE-git.md](../ARCHITECTURE-git.md) §8 ("Commit-graph notice").

## Blame and file history (S34)

Pure parsers in `crates/domain/src/git/blame.rs` and `crates/domain/src/git/file_log.rs`; commands in
`crates/engine/src/builtin/file.rs`. Both are reads (`Access::Read`) and slow on large histories
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

`CliBackend`: `git --literal-pathspecs -c log.diffMerges=separate log <FILE_LOG_ARGS> (--follow --find-renames | --no-follow
--no-renames) [--skip=<n>] --max-count=<n> --end-of-options <oid> -- <path>` (`LOG_ARGS` plus
`--name-status -m --root --no-relative`: without `-m` a merge has no entry; with `-m` it has one
record per parent, first parent first, and `parse_file_log` keeps the first (`--diff-merges=first-parent`
exists only from git 2.31, `log.diffMerges=separate` pins `-m` from git 2.33 on); without `--root` and with `log.showRoot=false` the root commit is hidden); the explicit
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
