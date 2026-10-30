# BinariGit

![BinariGit](assets/banner.png)


Graphical git client under construction (see [impl-docs/ROADMAP-git.md](impl-docs/ROADMAP-git.md)),
written in Rust with a layered architecture: an engine independent of the UI,
an egui desktop app, and a CLI and an MCP server that use the same
command catalog. Details in [impl-docs/ARCHITECTURE-rust.md](impl-docs/ARCHITECTURE-rust.md)
and [AGENTS.md](AGENTS.md).

## Getting started

```sh
make setup        # clippy and rustfmt
make run          # desktop app
make desktop-shot # headless screenshot of the desktop app -> target/desktop.png
make demo-repo    # demo repo to try the app -> target/demo-repo
make cli ARGS="commands --search repo"
make appimage     # Linux AppImage -> target/appimage (see packaging/README.md)
make windows      # Windows zip, cross-compiled -> target/windows
```

## CLI

```sh
project-cli commands [--search <text>]
project-cli [--repo <path>] run --cmd <id>[=<json>]...   # e.g. --repo . run --cmd repo.status
project-cli [--repo <path>] eval                         # commands from stdin, one per line
project-cli mcp [--root <dir>]                           # MCP server on stdin/stdout
```

`--repo` opens the repository before the commands (equivalent to `repo.open` as the first command).

Commands: `repo.open {"path"}` (`{"path", "head", "commit_graph"}`: `commit_graph` false = no
commit-graph, so `repo.log` is slow on huge histories), `repo.optimize` (writes the commit-graph,
`git commit-graph write --reachable`: a cache, no `confirm`, nothing to undo; `{"commit_graph"}`),
`repo.clone {"url", "path", "branch"?, "depth"?}` and `repo.init {"path", "branch"?}` (writes; no
open repo needed: they create the repo in an absent or empty folder, never over existing files, and
open it; result as `repo.open`, plus `messages` for the clone; a cancelled or failed clone removes
the folder it created; not available over MCP),
`repo.status`, `repo.refs`, `repo.log {"skip"?, "limit"?, "rev"?, "all"?, "stashes"?}`
(pages of 200 commits if `limit` is missing, with `has_more`; from HEAD, from `rev`, or with
`"all": true` from all local and remote branches, tags and HEAD, without stashes or backups, and with
`"stashes": true` in addition also from the commits the stashes were created on; `rev` and
`all` together, or `stashes` without `all`, are an error), `repo.search {"query", "in"?, "path"?,
"all"?, "stashes"?, "skip"?, "limit"?}` (ids of the commits whose message, author name or email, or id
prefix contain the text, literal and case-insensitive, in the order of `repo.log`; `in` limits to
`"message"`, `"author"`, `"id"`; result `{"ids", "total", "has_more"}`), `commit.show {"id"}` (metadata and files
changed relative to the first parent), `diff.worktree {"path"?}` (unstaged changes),
`diff.index {"path"?}` (staged changes), `diff.commit {"id", "path"?}` (commit relative to its
first parent) and `diff.range {"from", "to", "merge_base"?, "path"?}` (`to` relative to `from`, or
with `merge_base: true` relative to their merge base; the result also has `from`, `to` and `base`
as full ids). Diffs return `{"files": [...]}` with hunks and lines (in `diff.worktree` and
`diff.index` each file also has `digest`, to be passed to `index.*_lines`); `path` limits the
result to that file (renames stay renames) and is a string, or a byte array for non-UTF-8 names.
There is no size limit: for large commits it is better to request one file at a time.
`file.blame {"path", "rev"?}` says who last changed each line of a file (`{"lines", "commits",
"path", "rev"}`; without `rev` the worktree file, uncommitted lines have `commit: null`) and
`file.log {"path", "rev"?, "skip"?, "limit"?, "follow"?}` lists the commits that change it, newest
first, across renames unless `"follow": false` (`{"commits": [{"commit", "path", "old_path",
"status"}], "has_more"}`).

Writes (marked `[writes]` by `commands`): `index.stage {"paths"}` stages files
(modifications, new files, deletions; for a rename the two paths) and `index.unstage {"paths"}`
removes them from the index without touching the worktree. `paths` is a non-empty list of exact paths
(strings or byte arrays), taken literally; a directory includes the files it contains. A file
ignored by `.gitignore` is never forced: error, and nothing changes. Staging a conflicted file
(requested by name) resolves it; unstaging it is refused. Both
return `{"status", "backup"}`: the new status, like `repo.status`, and the index backup ref
(or `null`). The backup is saved automatically before writing when a staged version
is neither in HEAD nor in the worktree and would be lost (`git show <ref>:<path>` recovers it); if
it fails, the index is not touched.

`index.stage_lines {"path", "lines", "digest"}` and `index.unstage_lines {"path", "lines",
"digest"}` stage or unstage only some lines of a file (index only,
never the worktree): `lines` is a non-empty list of `{"old": n}` (removed line) or
`{"new": n}` (added line) with the line numbers from `diff.worktree` (stage) or `diff.index`
(unstage) for `path`, and `digest` is that of the file in the same diff. If the file changed since
the diff was read, the command fails without writing anything (the diff must be re-read).
Result and backup as `index.stage`. Binary files, submodules, symbolic links and conflicts can
only be staged whole.

`commit.create {"message", "amend"?, "confirm"?}` creates a commit with what is staged
(`git commit -F -`: message via stdin, hooks and signing according to the user's configuration, never
an editor). Empty message, nothing staged, conflicted files, rebase/cherry-pick/revert in
progress: error and nothing changes; during a merge the commit is the merge commit. With `"amend":
true` it rewrites the last commit (same author and parents) and saves the rewritten commit in a ref
`refs/project/backup/amend-...`; if that commit is already on a remote branch or has a tag,
`"confirm": true` is also required, and the other branches and tags that contain it are listed in `also_on`.
A hook that rejects or a signature that fails gives an error with all of its
output; a problem after a successful commit (e.g. a `post-commit` hook that moves HEAD) gives
an error that says not to repeat the commit and names the backup. Result: `{"commit",
"previous", "branch", "backup", "also_on", "status"}` (the commit as in `repo.log`, HEAD before,
the branch that moved, the backup or `null`, the refs left on the rewritten commit, the new status).

`worktree.discard {"paths", "confirm"}` discards the unstaged changes of the files `paths` (files or
directories, taken literally): the worktree goes back to the index, the index is not touched (an
`MM` file keeps its staged change). Without `"confirm": true` it gives an error that lists the files
that would be lost, and nothing changes. Before writing it saves the worktree with `git stash create`
in a ref `refs/project/backup/discard-...` (`git show <ref>:<path>` recovers it; if the
backup fails nothing is discarded) and records the discard in the journal: `edit.undo` restores the files
identically, only if they have not changed in the meantime. Untracked files are never touched
(requested by name: error; the backup would not cover them); submodules requested by name are also
refused and, because `git stash create` would not save the backup, so are a branch with no commits and a repo with
conflicted or `intent-to-add` files (`git add -N`). Also refused if, in place of a directory in
the path of a file, there is a file or a link (untracked or ignored, which `git restore`
would delete). If the bytes of a file are not what git would recreate (e.g. CRLF with
`core.autocrlf=input`) the backup saves them as they are, and the undo restores them identically. Undo and
redo of a discard are refused even if HEAD has changed. Result: `{"files": [{"path", "change",
"mode", "oid"}], "backup", "stash", "status", "can_undo", "can_redo", "next_undo",
"next_redo"}`.

`branch.create {"name", "start"?}` creates the branch `name` (short name) at `start` (a rev; HEAD if
missing) without switching branch; an existing branch is never overwritten, and a new name that git
does not accept (even one starting with `-`) is an error. Result: `{"branch", "target", "refs"}`
plus the journal state. `branch.checkout {"name"}` switches to the branch with `git switch`, never forced:
if a local change, an untracked file or an ignored one would be overwritten it gives an error
that lists the files, and nothing changes; other changes are kept. Refused during merge, rebase,
etc. and with conflicted files. Result: `{"branch", "previous", "changed", "warnings",
"status"}`. `branch.delete {"name", "confirm"?, "expected"?}` deletes a branch (with `expected`, the full id of the
commit seen, it refuses without touching anything if the branch points elsewhere in the meantime): the active branch or one open in
another worktree is refused; if it has commits that are neither in HEAD nor in its upstream,
`"confirm": true` is required. It always saves the commit first in `refs/project/backup/delete-...`; like `git
branch -d` it also removes the `branch.<name>.*` configuration (reported in `removed_config` and
saved for the undo). Result: `{"branch", "target", "unmerged", "backup", "removed_config",
"warnings", "refs"}`. `branch.rename {"name", "new_name"}` renames with `git
branch -m` (never over an existing branch): reflog, upstream and HEAD follow. Result:
`{"branch", "previous", "target", "head", "warnings", "refs"}`. All four go into the
journal: the undo deletes the created branch, recreates the deleted one at the same commit with its
configuration (if a new one has not appeared in the meantime; the reflog of the deleted branch is not
restored), renames back, and returns to the previous branch (or commit) with another safe
checkout; always only if nobody has touched those refs or HEAD in the meantime.

`branch.reset {"target", "mode": "soft"|"mixed"|"hard", "confirm"?, "expected"?}` moves the current
branch (or a detached HEAD) to `target` like `git reset`: `soft` only the ref, `mixed` also the index
(staged versions that exist only there saved first in `refs/project/backup/index-...`), `hard` also the
worktree of tracked files. A `hard` with local changes needs `"confirm": true` and saves them first in
`refs/project/backup/reset-...` (`git stash create` plus a byte-for-byte copy); untracked and ignored files
are never touched (one in the way refuses the reset). Refused during merge/rebase/cherry-pick/revert/bisect
and on a branch without commits. `branch.reset_preview {"target", "mode"}` (read-only) gives the same plan
as data: `files` a `hard` overwrites, `needs_confirm`, `expected`. Result: `{"mode", "branch", "before",
"after", "files", "backup", "stash", "index_backup", "backups", "published", "warnings", "status",
"refs"}`; `warnings` says when commits leaving the branch are already on its upstream. It goes into the
journal: undo moves the ref back (with index and worktree for `mixed`/`hard`), the changes overwritten by a
`hard` stay in the backup.

`remote.fetch {"remote"?, "branch"?}` downloads from the remote (`git fetch --progress`): new commits,
remote branches and tags as the remote's configuration says (also `fetch.prune`), never the local
branches, the index or the worktree. `remote` is the name of a configured remote (never a URL or a
path; a name that does not exist gives an error with the list of remotes); if missing, that of the
upstream of the active branch, then `origin`, then the only remote. With `branch` it downloads only that
branch of the remote. It never asks anything from the keyboard (no terminal, stdin closed): rejected or missing
credentials give an authentication error, a remote that cannot be reached (e.g. a URL
pointing to a directory that does not exist) a separate error. Result: `{"remote", "branch", "updated":
[{"name", "before", "after"}], "messages", "warnings", "refs"}` (the refs that changed, git's lines,
the new list of refs). It does not go into the journal. In the CLI, on a terminal, progress goes to
stderr; the UI shows it while the fetch runs and can cancel it.

`remote.pull {"remote"?, "branch"?, "strategy"?}` downloads the upstream branch (like
`remote.fetch`, cancellable) and integrates it into the branch `branch` (HEAD if missing): fast-forward, merge
or rebase according to `branch.<name>.rebase`/`pull.rebase` and `pull.ff` (`strategy` = `"merge"`,
`"rebase"` or `"ff_only"` overrides them; with no configuration, merge). A branch that is not the
active one can only be fast-forwarded, without touching the worktree. Never `--autostash`: merge and rebase
require a clean worktree (tracked files), the fast-forward refuses only if it would overwrite
local changes or untracked files. Refused (and nothing downloaded) with a detached HEAD, a branch
without upstream (the error says how to set it, or to publish it with `remote.push`), a merge or
rebase in progress, conflicted files, `pull.rebase=interactive`; also refused if the pull
would bring a file that already exists in the worktree as untracked or ignored (git would
overwrite it). Before writing it saves the branch in
`refs/project/backup/pull-...`. If the **merge** stops (conflicts, a hook, signing) it remains
in progress as with `merge.start`: it is not an error, the result has `"in_progress": true` and the files
in `conflicts`, the branch has not moved and the backup remains; it is concluded with `merge.continue` (after
resolving and staging the files) or cancelled with `merge.abort`. A stopped **rebase** also stays
in progress: it is concluded with `rebase.continue` (the pull's journal entry) or cancelled with
`rebase.abort`. It goes into the journal when it moves the branch:
`edit.undo` returns the branch to the previous commit with index and worktree (like `git reset --keep`:
refused if a file to restore has local changes or if a file to recreate already exists); the
stopped merge or rebase does not (it goes in with `merge.continue`/`rebase.continue`). Result: `{"branch", "remote", "upstream",
"strategy": "fast_forward"|"merge"|"rebase"|"up_to_date", "strategy_source", "before", "after",
"ahead", "behind", "in_progress", "conflicts", "message", "backup", "fetch": {"updated",
"messages"}, "warnings", "status", "refs"}` plus the journal state.

`remote.push {"remote"?, "branch"?, "force"?, "confirm"?, "expected"?}` publishes the branch (HEAD if missing)
with an explicit refspec `refs/heads/<b>:refs/heads/<b>`: never tags or other branches, whatever
`push.default`, `remote.<name>.push` or `push.followTags` say. The remote is chosen like `git push`:
`remote`, then `branch.<name>.pushRemote`, `remote.pushDefault`, the upstream's remote, `origin`
or the only remote (`remote_source` in the result); a mirror remote is refused. On the upstream's
remote it goes to the upstream's branch, elsewhere to the same name; on the first push (no
upstream) it sets the upstream. An
upstream with a different name is refused (like `push.default=simple`). A non-fast-forward push is
refused (error that suggests the pull) and nothing changes. `"force": true` uses only
`--force-with-lease` with the id of the local remote branch read beforehand (never `--force`); if the remote
would lose commits, `"confirm": true` is also required, with `"expected"`, the id of the local remote branch
that the confirmation error shows (without it, the error lists them; if it changed in the meantime confirmation is asked
again and nothing is published), and they are first saved
in `refs/project/backup/push-...`; if the remote moved in the meantime the push is refused.
A `pre-push` hook that rejects gives an error with its output. It does not go into the journal (a push cannot
be undone). Result: `{"remote", "remote_source", "branch", "remote_branch", "kind":
"new"|"fast_forward"|"forced"|"up_to_date", "summary", "before", "after", "set_upstream",
"forced", "lost", "backup", "updated", "messages", "warnings", "status", "refs"}`.

`remote.list {}` (read-only) lists the remotes in name order: `{"remotes": [{"name", "url",
"push_urls", "fetch", "mirror", "branches", "default", "push_default", "credentials"}],
"default", "push_default"}` (`push_urls` empty = the push goes to `url`; `default` is the remote that
`remote.fetch` uses without parameters). **Credentials in URLs are never shown**:
`https://user:token@host/r` becomes `https://user:***@host/r`, `https://token@github.com/r`
becomes `https://***@github.com/r` (in `http(s)`/`ftp(s)` a lone user is often a token),
`credentials: true` says there were some; an ssh user (`git@host:r`) stays. This also applies to the
results of `remote.add`/`remote.remove` and to the journal (`edit.history`).

`remote.add {"name", "url", "fetch"?}` adds a remote (`git remote add --end-of-options`,
default refspec; `"fetch": true` then downloads like `remote.fetch`, with progress and
cancellation: a failed fetch does not undo the addition and ends up in `fetch_error`). Refused
before calling git: names that start with `-` or are invalid, empty URLs, URLs with control
characters, that start with `-` (`--upload-pack=...`), with the `ext::`/`fd::` transports (they run
commands) or with a host that starts with `-` (`ssh://-oProxyCommand=...`); a name already in use or with
remote branches left over from before. A URL with credentials is accepted with a warning (git stores it in
clear text). Result: `{"remote", "url", "credentials", "fetched", "fetch", "fetch_error",
"warnings", "remotes", "refs"}`. It goes into the journal: the undo removes the configuration and the downloaded remote branches
only if they are still as they were (and no branch has taken the remote as upstream).

`remote.remove {"name", "confirm": true}` removes a remote configured in the repo's file
(refused if it is defined, even partly, elsewhere, globally or via `include`, or if it is the promisor
remote of a partial clone): the
`remote.<name>` section, the remote branches `refs/remotes/<name>/...` (with `HEAD`), the upstream of the
branches that use it, their `pushRemote` and `remote.pushDefault` if they name it. Without `confirm`
the error lists the branches that would lose their upstream and the number of remote branches, and nothing
changes. Before writing, everything is saved in the journal and the commits that were left only in remote
branches go into `refs/project/backup/remote-...`. Result: `{"remote", "removed",
"upstreams_removed", "push_remotes_removed", "push_default_removed", "backups", "warnings",
"remotes", "refs"}`. The undo restores configuration, remote branches and upstream, only if nobody has
recreated a remote with that name; an upstream reset in the meantime stays, with a warning.
Ahead/behind of the branches relative to the upstream are in `repo.refs` (`upstream.ahead`/`behind`) and
in the `refs` of the fetch, pull and push results.

`stash.list {}` (read-only) lists the stashes from the most recent: `{"stashes": [{"index",
"name": "stash@{n}", "id", "base", "message", "branch", "time", "untracked"}]}` (`base` is the
commit the stash was created on, `untracked` says whether it also has untracked files; the
content is read with `commit.show`/`diff.commit` on the `id`). `stash.push {"message"?,
"include_untracked"?, "keep_index"?, "paths"?}` sets changes aside (`git stash push`;
never ignored files; a one-line message; refused with a merge or rebase in progress, conflicted
or `intent-to-add` files, `--assume-unchanged` files among those chosen, whose changes git would lose,
or nothing to set aside) and returns `{"stash", "stashes",
"status"}`; it goes into the journal: the undo reapplies the stash with the split between staged and not and removes it
from the list (only if HEAD is still at the same commit), the redo redoes it. `stash.apply
{"index"?, "expected"?, "restore_index"?}` applies the stash `stash@{index}` (0 if missing;
`expected`, the full id seen, protects against a list that changed in the meantime) without removing it,
`stash.pop` with the same parameters also removes it, after saving its commit in
`refs/project/backup/stash-...`. Both are refused, without changing anything, if the stash
would touch files with local changes, untracked or ignored ones (`git stash apply` in these cases
would write halfway; a staged file that already has the stash's version, as after a push with
`keep_index`, does not count); if the merge with HEAD conflicts the files stay conflicted as git
leaves them (`conflicts`) and `pop` keeps the stash. Result: `{"stash", "conflicts", "dropped",
"backup", "warnings", "stashes", "status"}`; they do not go into the journal. If after the apply the drop of
`pop` fails, the pop is not an error: `dropped: false` and a warning (the stash must be deleted
with `stash.drop`, the pop must not be repeated). `stash.drop {"index"?, "expected", "confirm": true}`
deletes a stash (without `confirm` the error says what would be lost and gives the id to repeat in
`expected`; `confirm` without `expected` is an error), with the backup in `refs/project/backup/stash-...`: it goes into the journal and the undo
puts it back at the top of the list. Result: `{"stash", "backup", "warnings", "stashes"}`.

`merge.start {"rev", "no_ff"?, "ff_only"?, "message"?}` merges `rev` (local or remote branch,
tag, id) into the active branch: fast-forward if possible (`no_ff`: always a merge commit;
`ff_only`: fast-forward only), otherwise a merge commit with `message` or git's message
(`Merge branch 'x'`). Refused with a detached HEAD, an operation in progress, conflicted files and,
for a merge commit, any change to tracked files (never `--autostash`); it first saves the
branch in `refs/project/backup/merge-...`. **With conflicts the merge stays in progress**: the result
has `in_progress: true` and the files in `conflicts`. Result: `{"rev", "source", "target", "branch",
"strategy": "fast_forward"|"merge"|"up_to_date", "before", "after", "ahead", "behind",
"in_progress", "conflicts", "message", "backup", "warnings", "status", "refs"}`. `merge.continue
{"message"?}` concludes the merge when no conflicted files remain (resolved and staged with
`index.stage`; otherwise the error lists them), with the message prepared by git without the
`# Conflicts:` block. `merge.abort {"confirm": true}` cancels it (`git merge --abort`): without `confirm`
the error says which resolutions and changes would be lost; it first saves index and worktree in
`refs/project/backup/merge-abort-...` (restored with `git stash apply <ref>`). The concluded merge
(immediately or with `merge.continue`) goes into the journal and the undo returns the branch as it was, with index and
worktree; a stopped merge and the abort do not. `repo.operation {}` (read-only) is the banner
of the operation in progress: `{"operation", "head", "conflicts", "incoming": [{"id", "label",
"summary", "refs"}], "message", "onto", "rebase_branch", "ready", "continue", "abort"}`
(`continue`/`abort` are the commands to use: merge, rebase, cherry-pick and revert).

`commit.cherry_pick {"rev", "mainline"?}` applies a commit on the active branch with a new commit
(original author and message); `commit.revert {"rev", "mainline"?}` undoes its changes with
a new commit (`Revert "<subject>"`, `This reverts commit <id>.`). A merge commit needs
`mainline` (the parent, from 1). Refused like the merge (detached HEAD, operation in progress,
conflicts, any change to tracked files); a commit already applied (empty result) gives
`NotAllowed` and nothing stays in progress. They first save the branch in
`refs/project/backup/cherry-pick-...` / `revert-...`. **With conflicts the operation stays in
progress**: it is concluded with `cherry_pick.continue` / `revert.continue {"message"?}` or cancelled with
`cherry_pick.abort` / `revert.abort {"confirm": true}` (`...-abort-...` backup as for the merge).
Result: `{"rev", "commit", "summary", "mainline", "branch", "before", "after", "in_progress",
"conflicts", "message", "backup", "warnings", "status", "refs"}`. Once concluded it goes into the journal
(the undo returns the branch as it was); a sequence of several commits started from the terminal stays
with the terminal. Details in [impl-docs/git/cherry-pick.md](impl-docs/git/cherry-pick.md).

`rebase.plan {"base"}` (read-only) gives the default plan of the commits `base..HEAD` of the active
branch (`steps`: `pick` of each, oldest first, with `summary`; `published`: those already on a remote
branch). `rebase.start {"base", "steps": [{"action", "commit", "message"?}], "confirm"?}` rewrites them:
`pick`, `reword` (with `message`), `squash`/`fixup` into the commit before (`message` allowed on the
last of the group), `drop`, in any order; every commit of the range exactly once. Refused with a
detached HEAD, an operation in progress, conflicts, any change to tracked files, merges in the range,
a plan that changes nothing; rewriting published commits needs `"confirm": true`. It first saves
the branch in `refs/project/backup/rebase-...`. **A stop (conflict, hook, signing) leaves the rebase
in progress**: `rebase.continue {}` after resolving and staging, `rebase.abort {"confirm": true}`
(`rebase-abort-...` backup) to go back. Result: `{"base", "branch", "before", "after",
"in_progress", "conflicts", "message", "backup", "warnings", "status", "refs"}`. Once concluded it
goes into the journal (the undo returns the branch as it was). Details in
[impl-docs/git/rebase.md](impl-docs/git/rebase.md).

`conflict.show {"path"}` (read-only) gives the versions of a conflicted file: `{"path", "kind",
"base", "ours", "theirs", "worktree", "marker_size", "digest"}`, each version `{"text", "binary",
"size"}` or `null` if missing (`text` `null` and `binary` for non-UTF-8 bytes or with NUL); a file that is not
conflicted, a submodule, a link or a directory are refused. `conflict.resolve {"path",
"digest", "content"? | "delete"?: true}` writes `content` to the worktree (or deletes the file) and
stages it like `index.stage`; refused with `StaleDiff` if the file is no longer the one of the
`digest` read. It first saves the worktree bytes in `refs/project/backup/conflict-...` (`git show
<ref>:<path>`). Result: `{"status", "backup", "index_backup"}`. It does not go into the journal.

`edit.undo {}` undoes the last operation in the journal (currently `commit.create`, also with `amend`,
`branch.*`, `worktree.discard`, `remote.pull`, `remote.add`, `remote.remove`, `stash.push`,
`stash.drop` and the concluded merge, `merge.start` and `merge.continue`) and `edit.redo {}` redoes it; `edit.history {}` (read-only) lists the journal. The undo
returns the branch (or detached HEAD) to the previous value only if nobody has moved it in the
meantime (a `git commit` or `git reset` from the terminal): otherwise error and nothing changes.
For a commit, index and worktree are not touched, like `git reset --soft`: after the undo of a
commit its changes are staged again. The undo of a discard restores the files from the backup
(`git restore --source=<stash> --worktree`) only if they are still as the discard left them and
with the same staged version; the redo discards them again only if they are still as the undo
left them. The undone commit, if no longer reachable, stays in a
ref `refs/project/backup/undo-...`. Stage and unstage do not go into the journal. The journal is in
memory and applies to the open repo: in the CLI several `--cmd` of the same invocation
share it, different invocations do not (`edit.undo` alone says there is nothing to undo).
Result of undo and redo: `{"undone"|"redone": entry, "backups", "warnings", "status",
"can_undo", "can_redo", "next_undo", "next_redo"}` (`warnings`, usually empty, reports for
example HEAD changed by another process during the undo itself). During a merge, rebase,
cherry-pick, revert, bisect or `git am` undo and redo are refused.

The MCP server exposes `list_commands`, `execute_command` and `open_repo`; `list_commands` gives
for every command the JSON Schema of its parameters (`params`, also in `project-cli commands
--json`) and reports `writes: true` for the commands that modify the repository. At startup it opens the
repository of `--root` (default: current directory), if there is one. All paths are confined to
`--root`, including the repository search (git does not climb above the root); a repository is
opened only with `open_repo`: the commands with `outside_repo: true` (`repo.open`, `repo.clone`,
`repo.init`) are refused by `execute_command`.

## Quality

`cargo xtask ci` runs formatting, Clippy with warnings as errors, tests, and the layering check. GitHub CI uses the same command.

## Name and graphics

The visible name is **BinariGit**; crates, binaries and technical refs stay `project-*`. Icon, logo and
banner are in [assets/](assets/): edit the SVGs and regenerate the PNGs with
`assets/render.sh` (headless Chrome).
