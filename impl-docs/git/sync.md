# Fetch, pull and push

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

## Commands

### `remote.fetch`

Parameters: `{"remote"?, "branch"?, "prune"?}`.

Slow (§6), writes (`writes`: remote branches, tags, `FETCH_HEAD`). `prune: true` (not with `branch`:
`InvalidParams`) also removes the remote-tracking branches whose branch the remote no longer has
(`--prune`); the UI's "Fetch" always asks for it, so a branch deleted on the remote disappears from
the column. A removed remote-tracking branch with commits no other ref reaches is saved first in
`refs/project/backup/prune-...` (result `backups`): the remote no longer has them, the repo keeps
them. `remote`: the **name** of a configured
remote (`GitRemotes::remotes`, i.e. `git remote`), checked with `check_remote_name` (like
git: `refs/remotes/<name>/x` must be a valid ref name; a leading `-` is allowed) and looked up
among the configured ones before calling the backend: a name that does not exist → `NotAllowed` with the list of
remotes (never passed to git, which would use it as a URL or path). Without `remote`: that of the
upstream of the active branch, otherwise `origin`, otherwise the only remote (`default_remote`;
none or ambiguous → `NotAllowed`). `branch`: short name of a branch **of the remote**
(`check_branch_name`), downloaded alone as `refs/heads/<branch>`; if the remote does not have it →
`NotAllowed`. Errors: rejected credentials or credentials to be requested → `Auth { remote, message }`; remote not
reachable (directory that does not exist, host, network, repository nonexistent on the server) →
`RemoteUnreachable { remote, message }`; cancelled → `Cancelled`; other → `Git`. Result:
`{"remote", "branch": string | null, "prune": bool, "updated": [{"name", "before": oid | null,
"after": oid | null}] | null, "messages": [string], "warnings": [Message], "backups": [string],
"refs": [RefInfo] | null, "refs_error"?}`: `updated` are the changed refs (comparison of `refs()` before and after, an annotated tag
with the commit pointed to; `null` with `refs_error` if the refs cannot be re-read: the fetch is done),
`warnings` are `Message` objects `{"text", "key", "args"?}` (English text plus Fluent key and
arguments, see `impl-docs/i18n.md`), like the journal `description` and `next_undo`/`next_redo` in every result;
`messages` the git lines that are not progress (`From <url>`, ` * [new branch] ...`),
`warnings` the refs outside `refs/remotes/` and `refs/tags/` moved by a configured refspec (e.g. a
mirror toward `refs/heads/`). It does not go into the journal (§7). `CliBackend` details below.

**Automatic fetch** (S50, UI only: `crates/ui/src/auto_fetch.rs`): every tab sends the same
`remote.fetch` as the bar's "Fetch" (`{"prune": true}`, the remote the engine chooses) on a timer
(Settings → Remotes: off / 1 / 5 / 15 minutes, default 5), only for the tab shown and with no dialog,
command or operation in progress; `Auth` suspends it until "Retry", `RemoteUnreachable` doubles the
wait up to 30 minutes. No engine change.

### `remote.pull`

Parameters: `{"remote"?, "branch"?, "strategy"?}`.

Slow (§6), writes (`writes`), goes into the journal (§7). Fetch of only the upstream branch with
`GitRemotes::fetch` (progress and cancellation like `remote.fetch`; cancelled during the fetch →
`Cancelled { command: "remote.pull" }` even if the fetch finished, before integrating), then
the local integration with `GitRemotes::pull`, which is not interrupted. **Choice: not `git pull`**,
but fetch + merge/rebase driven by the engine: this way the command knows what arrived before writing,
chooses the strategy, checks the state and saves the backup. `branch`: short name of the local branch
(HEAD if missing; detached HEAD without `branch` → `NotAllowed`); a branch that is not HEAD's can
only be fast-forwarded (ref fast-forward with `update-ref` and the old value, without touching the
worktree; diverged → `NotAllowed` asking to switch to it; open in another worktree →
`NotAllowed`). `remote`: if present it must be the upstream's (`NotAllowed` otherwise): the pull
always integrates the upstream. Upstream from `branch.<name>.remote`/`.merge` (`GitRemotes::upstream`);
without → `NotAllowed` saying how to set it (`git branch --set-upstream-to=<remote>/<branch>`) or
to publish the branch with `remote.push`; local upstream (`.`), `merge` that is not a branch,
no refspec mapping it into `refs/remotes/` → `NotAllowed`, nothing downloaded. **Strategy**
(`PullSettings::policy`, read before the fetch): `strategy` (`merge`, `rebase`, `ff_only`)
overrides the configuration; otherwise `branch.<name>.rebase`, then `pull.rebase` (`true` →
rebase, `merges` → `--rebase-merges`, `false` → merge, `interactive` → `NotAllowed`) and `pull.ff`
(`only` → fast-forward only, `false` → always a merge commit); **without configuration, merge**
(choice: git ≥ 2.33 would refuse diverged branches; a merge rewrites nothing and can be aborted).
Then (`pull_mode`, from `ahead`/`behind` with `unmerged_count`): nothing to do if the branch contains
the upstream (`up_to_date`, nothing in the journal), fast-forward if possible, otherwise merge or rebase
(`ff_only` diverged → `NotAllowed`). **Checks** (`check_pull_state`, before the fetch for HEAD and
again in the backend): operation in progress or conflicted files → `NotAllowed`; merge and rebase with
any change to tracked files → `NotAllowed` (never `--autostash`; untracked files are fine);
the fast-forward accepts changes that do not touch the changed files, and if git refuses because it would
overwrite changes or untracked files → `NotAllowed` with the files. **Backup**: before writing
`refs/project/backup/pull-...` on the branch as it was (removed if afterwards nothing changes).

**Stopped merge** (choice, after S29): a pull in merge that stops (conflicts, even if `rerere`
already resolved them; a `pre-merge-commit`/`commit-msg` hook or signing that fail after a merge
without conflicts) **stays in progress** as with `merge.start` ([merge.md](merge.md)): it is not an
error, the result has `"in_progress": true`, the files in `conflicts` (empty for hook and signing) and
git's text in `message`; the branch has not moved (`after == before`), the backup
`refs/project/backup/pull-...` stays and nothing goes into the journal. The fetch is done. You exit as from
a stopped `merge.start`: `merge.continue` (after resolving and staging the files with
`index.stage`; the message is the pull's, `Merge remote-tracking branch 'origin/main'`, which
git puts in `MERGE_MSG` with the `# Conflicts:` block, removed by `merge.continue`) goes into the
journal as a concluded merge (`Merge of origin/main into `main``); `merge.abort` (with `confirm`)
returns index and worktree to HEAD, i.e. as they were before the pull, because the pull in merge starts
only from a worktree without changes to tracked files (untracked files stay: `merge --abort` does not
touch them). `repo.operation` gives the banner (`incoming` is the upstream commit, `label` the name
of the remote branch). The name of the other side in the `CliBackend` markers is the upstream id
(`>>>>>>> <oid>`: only the id reaches git, as for `merge.start`).

**Stopped rebase** (S32): like the merge it **stays in progress** (conflicts, a lock, signing): result
`in_progress: true`, `conflicts`, `message`, the branch not moved (`after == before`, HEAD detached
by git on the commit reached), the backup `refs/project/backup/pull-...` kept, nothing in the journal.
The backend saves the [`RebaseRecord`](rebase.md#undo-and-backup) of the pull (command `remote.pull`,
branch, before, upstream, backup) in `rebase-merge/project-rebase.json`. `rebase.continue` (after
resolving and staging) concludes it with the **pull's** journal entry (`Pull of `main` (rebase onto
origin/main: ...)`, undo like a concluded pull, with a warning if the branch does not contain the
upstream); `rebase.abort` (with `confirm`) returns the branch where it was. `repo.operation` offers
`rebase.continue`/`rebase.abort`. `PullConflict` (and `PullOperation`) stay in the types for
compatibility but no backend produces them any more.

Fetch errors: `Auth`, `RemoteUnreachable`, `Cancelled`. Result: `{"branch", "remote",
"upstream", "strategy": "fast_forward" | "merge" | "rebase" | "up_to_date", "strategy_source":
"param" | "config" | "default", "before", "after", "ahead", "behind", "in_progress": bool,
"conflicts": [path], "message": string, "backup": string | null, "fetch": {"updated",
"messages"}, "warnings", "status" | null, "refs" | null, "can_undo", "can_redo", "next_undo",
"next_redo"}` (`in_progress`/`conflicts`/`message` as in `merge.start`: `false`, `[]`, `""`
if the pull is not stopped). `CliBackend` and undo details below.

### `remote.push`

Parameters: `{"remote"?, "branch"?, "force"?, "confirm"?, "expected"?}`.

Slow (§6), writes (`writes`), does **not** go into the journal (a push cannot be undone: the remote belongs to
others; the result says what changed). Publishes only
`refs/heads/<branch>:refs/heads/<remote_branch>` (explicit refspec: never tags, `--all`, `--mirror`,
`--tags`, whatever `push.default`, `remote.<name>.push`, `push.followTags` say). `branch`: HEAD if
missing (detached → `NotAllowed`). **Remote** as `git push` chooses it (result `remote_source`):
`remote` (`param`), otherwise `branch.<name>.pushRemote` (`push_remote`), `remote.pushDefault`
(`push_default`), the upstream's remote (`upstream`), `origin` (`origin`) or the only remote
(`only`); several remotes without `origin` → `NotAllowed`; a remote from the configuration that is not
configured → `NotAllowed` naming the key; a remote with `remote.<r>.mirror=true` →
`NotAllowed` (a mirror publishes all refs together). **Destination**: if the remote is the
upstream's, the upstream's branch (with a name different from the branch → `NotAllowed`, like
`push.default=simple`) and the upstream's local remote branch; otherwise the same name, and the local
remote branch is derived from the remote's fetch refspecs (`tracking_ref` on
`remote.<r>.fetch`, even with non-standard and negative refspecs; none → no local
remote branch). **First push** (no upstream): `--set-upstream` toward the chosen remote (the upstream is
set only if the push succeeds); with an upstream the upstream never changes (e.g. when publishing to
`fork` because of `remote.pushDefault`). **Force**: only with `"force": true`, always and only
`--force-with-lease=refs/heads/<b>:<oid>` with the oid of the local remote branch read beforehand (empty if
it does not exist: the branch must not exist), never `--force` or `+refspec`; if the remote would lose commits
that the branch lacks (`unmerged_count` of the local remote branch) `"confirm": true` is required **with
`"expected"`**, the full id (40 digits) of the local remote branch shown in the confirmation: without it,
`ConfirmRequired` with the commits (`missing_commits`, at most 10), the id read now and the fixed phrase
`"expected": "<id>"` to repeat. The confirmation is valid only for that commit: if `expected` is not the
local remote branch read now (a fetch in the meantime) → `ConfirmRequired` again with the new id,
nothing published; the lease uses `expected`. Before the push the local remote branch is saved in
`refs/project/backup/push-...` (removed if the push is rejected). `confirm` without `expected`,
`confirm`/`expected` without `force`, `expected` not full → `InvalidParams`. `--force-if-includes`
is not needed: with an explicit oid git ignores it. Errors: `PushRejected { remote, branch, reason:
non_fast_forward | stale_lease | remote_rejected | hook | other, message }` (non-fast-forward
suggests `remote.pull`; lease: the remote moved, do a fetch), nothing changes locally;
`Auth`, `RemoteUnreachable`, `Cancelled` (cancelled midway: the remote may already have updated
the branch, re-read with a fetch). Result: `{"remote", "remote_source": "param" | "push_remote"
| "push_default" | "upstream" | "origin" | "only", "branch", "remote_branch", "kind": "new" |
"fast_forward" | "forced" | "up_to_date", "summary", "before": oid | null, "after", "set_upstream",
"forced", "lost": [Commit], "backup": string | null, "updated" | null, "messages", "warnings",
"status" | null, "refs" | null}`.

### `remote.delete_branch`

Parameters: `{"remote", "branch", "confirm"?, "expected"?}`.

Slow (§6), writes (`writes`), does **not** go into the journal (like the push: the remote belongs to
others). Deletes **only** `refs/heads/<branch>` from the configured remote `remote` (`branch` is the
name on the remote, `check_branch_name`; `remote` with `check_remote_name`, not configured →
`NotAllowed` with the list, `remote.<r>.mirror=true` → `NotAllowed`). The local remote-tracking branch
(`tracking_ref` on `remote.<r>.fetch`) must **exist**: it is the commit the user saw, the one confirmed
and saved; no refspec mapping it, or not fetched yet → `NotAllowed` (do a `remote.fetch`), nothing
reaches git. It **always** requires `"confirm": true` **with `"expected"`**, the full id (40 digits) of
the local remote-tracking branch: without them `ConfirmRequired` with the id, the commits of the branch
that no other ref here reaches (`unmerged_count` against all the other refs, at most 10 listed with
`missing_commits`), the local branches that have it as upstream and the fixed phrase `"expected":
"<id>"`; nothing changes. An `expected` that is not the value read now (a fetch in the meantime) →
`ConfirmRequired` again with the new id. `confirm` without `expected`, `expected` not full →
`InvalidParams`. Before the push the commit goes into `refs/project/backup/push-delete-...` (removed
if git rejects or fails before starting; kept if cancelled midway). Then always
`--force-with-lease=refs/heads/<branch>:<expected>`: if the branch moved on the remote or someone
already deleted it, git answers `stale info` → `PushRejected { reason: stale_lease }` and nothing
changes. git removes the local remote-tracking branch; the upstream of the local branches stays
configured (shown as gone), like `git push --delete`. A branch the server refuses to delete (its
default branch, a protected branch) → `PushRejected { reason: remote_rejected }`. Errors as the push:
`Auth`, `RemoteUnreachable`, `Cancelled`. **Several push URLs** (`remote.<r>.pushurl`): git deletes on
each in turn; deleted on at least one → success with `"incomplete": true` if another refused (or git
failed after) and a warning naming the backup. The backup is removed **only** when nothing was
certainly deleted (`!` on every URL, a `pre-push` hook before sending, credentials refused); after an
unknown failure or a lost connection it stays. To put a branch back: a branch on the backup and
`remote.push`. Result: `{"remote", "branch", "tracking", "deleted": oid, "only_here": n, "lost":
[Commit], "upstream_of": [branch], "backup", "summary", "messages", "incomplete", "updated" | null, "warnings",
"status" | null, "refs" | null}`.

### `remote.push_tag`

Parameters: `{"remote", "name"}`.

Slow (§6), writes, does **not** go into the journal. Publishes **only**
`refs/tags/<name>:refs/tags/<name>` to the configured remote `remote` (`check_remote_name`; not
configured → `NotAllowed` with the list, `remote.<r>.mirror=true` → `NotAllowed`). The local tag
must exist (`check_tag_name`: any valid name, even `-x`; missing → `NotAllowed`). **Never forced**:
the same value already on the remote → success with `"kind": "up_to_date"`; a tag already there
with another value → `PushRejected { reason: tag_exists }` (`branch` = `refs/tags/<name>`),
nothing changes (to replace it: `remote.delete_tag` first, with its confirmation). Other errors as
`remote.push` (`hook`, `remote_rejected`, `Auth`, `RemoteUnreachable`, `Cancelled`). Result:
`{"remote", "tag", "name", "target", "annotated", "value" (as stored: the tag object of an
annotated tag), "kind": "new" | "up_to_date", "summary", "messages", "refs" | null}`.

### `remote.delete_tag`

Parameters: `{"remote", "name", "confirm"?, "expected"?}`.

Like `remote.delete_branch`: slow, writes, not in the journal, same remote checks. There is no
remote-tracking ref for tags, so the value is **read from the remote** (`GitRemotes::remote_tag`,
`git ls-remote`; it contacts the remote even for the confirmation): the value as the remote stores
it (the tag object of an annotated tag, not the peeled commit). Not on the remote → `NotAllowed`.
**Always** `"confirm": true` with `"expected"`, the full id read: without them `ConfirmRequired` with
the value, how it relates to the local tag (same value / another value / no local tag) and the fixed
phrase `"expected": "<id>"`; an `expected` that is not the value read now → `ConfirmRequired`
again with the new value. `confirm` without `expected`, `expected` not full → `InvalidParams`. The
local tag is never touched. Before the push the value goes into `refs/project/backup/push-delete-...`
(if the object is not here it is fetched into it first, see below); if the backup cannot be made
nothing is deleted. Then `--force-with-lease=refs/tags/<name>:<expected>`: moved or already gone
→ `PushRejected { reason: stale_lease }`. Backup removal, several push URLs (`incomplete`) and
errors as `remote.delete_branch`. Result: `{"remote", "tag", "name", "deleted": oid, "backup",
"summary", "messages", "incomplete", "warnings", "refs" | null}`.

## CliBackend: fetch

**Fetch** (`remote.fetch`, S25): `git fetch --progress --end-of-options <remote>
[refs/heads/<branch>]`. The remote (always a configured remote, never a URL) and the branch
go after `--end-of-options`: a remote called `-x` or `--upload-pack=false` does not
become an option (tests with these names). The branch is a refspec without destination, without
`:` or `+`: it writes no local refs, and the remote branch is updated with the configured refspec.
No `--force`, `--tags`, `--update-head-ok`, `--recurse-submodules`: tags and submodules as the
user's configuration says, like `git fetch` from the terminal. `--prune` only with `prune: true`,
together with `--no-prune-tags` (a configured `fetch.pruneTags` would delete local tags) and
`--no-auto-gc` (no gc between the fetch and the backups): all refs (symbolic ones and
`refs/project/` excluded) are read before and after (`for-each-ref`); a removed remote-tracking branch
whose commits `rev-list --count <id> --not --all` finds unreachable gets a backup ref, and any other
removed ref (a refspec toward `refs/heads/`, a mirror) always gets one; a backup
that fails becomes a line in `messages`, the fetch is done. Without it pruning follows `fetch.prune` /
`remote.<name>.prune`. **No input
requests**, ever: stdin closed, `GIT_TERMINAL_PROMPT=0` and, on Unix, the process starts in a new
session (`setsid`, `libc` dependency) without a controlling terminal, so neither git, nor
ssh (passphrase, host key confirmation), nor a credential helper can open
`/dev/tty` and wait, even if the CLI or MCP server are started from a terminal. Graphical
requests remain (`SSH_ASKPASS` with a display, `GIT_ASKPASS`, `core.askPass`) and credential
helpers that ask nothing (cache, keychain, libsecret, GCM); without them, a missing
credential fails immediately with `Auth`. stderr is read while git runs
(`Git::run_streaming`): split at every `\r` and `\n`, the progress lines go to the
`JobContext` (§6), the others, only those closed by `\n`, are the result's `messages` or the
error text. Errors recognized from the text (`LC_ALL=C`, `progress::remote_error`):
`Auth` for "Authentication failed", "could not read Username/Password", "terminal prompts
disabled", "Permission denied (publickey...)", "Host key verification failed", HTTP 401/403;
`NotAllowed` for "couldn't find remote ref"; `RemoteUnreachable` for "does not appear to be a
git repository", "Could not resolve host", "Connection refused/timed out", "repository not
found", HTTP 404, "unable to access", "Could not read from remote repository" (checked after
the `Auth` ones, which precede them in the same stderr). After git exits the backend
waits at most one second for stderr to close: a process left in the background (an ssh
master with `ControlPersist`) does not block the command. Cancellation: see §6. Limits: on
Windows no `setsid` (there is no `/dev/tty`; cancellation uses `kill`); the repo's
configuration can give remotes with URLs outside the MCP server's root (the fetch reads from there, as
from the terminal).

## CliBackend: pull

**Pull** (`remote.pull`, S26): after the fetch of `refs/heads/<merge>` (as above), the backup
and then, with `GIT_REFLOG_ACTION=project: remote.pull`, `GIT_EDITOR=:` and `GIT_SEQUENCE_EDITOR=:`:
fast-forward of the active branch `git -c core.fsmonitor=false merge --ff-only --no-autostash
--no-overwrite-ignore --no-edit --end-of-options <oid>` (git refuses before writing if it would overwrite changes
or untracked files); of another branch `update-ref -m ... refs/heads/<b> <new> <old>`;
merge `git -c core.fsmonitor=false merge --no-autostash --no-overwrite-ignore --no-edit --commit --no-squash
--ff|--no-ff -F <file> --end-of-options <oid>` (the message, `Merge remote-tracking branch 'origin/main'` plus ` into
<b>` outside `main`/`master`, from a temporary file in the git directory: `git merge` does not read
`-F -` from stdin, and its path goes to git as `OsStr`; explicit `--ff`/`--no-ff`, `--commit`, `--no-squash` against `merge.ff` and
`branch.<name>.mergeOptions`); rebase `git -c
core.fsmonitor=false -c rebase.updateRefs=false rebase --no-autostash --no-autosquash [--rebase-merges]
--end-of-options <oid>` (`rebase.updateRefs` would move other branches outside the journal; without the
fork-point of `git pull --rebase`: with a rewritten upstream the rebase usually stops on
conflict, and stays in progress). **Ignored files**: git overwrites without saying so an ignored file that
the upstream adds (and an aborted merge would delete it); the rebase has no
`--no-overwrite-ignore`. For this reason, for the active branch and before the backup, the files the
upstream brings and HEAD does not have (`diff-tree -r -z --name-only --no-renames --diff-filter=A HEAD
<upstream>`) must not exist in the worktree, nor in place of one of their directories
(`symlink_metadata`): otherwise `NotAllowed` with the files and nothing changes (for fast-forward and
merge it is a second defense after `--no-overwrite-ignore`). For the rebase (also
`--rebase-merges`) the files touched by each of the commits to reapply and that
HEAD does not have also count (`git log -m --no-renames --format= -z --name-only <upstream>..HEAD`): the rebase
goes through the intermediate trees, so a file that a local commit adds and a later one
removes (or renames, or removes and recreates), or that a local commit modifies while the upstream
removes it (modify/delete conflict, then `rebase.abort`), would be written and then deleted
together with the ignored file that was in its place. The merge goes only from HEAD to the
result and does not need it. The `MemoryBackend` does the same check on the trees of the
commits to reapply. Hooks (`pre-merge-commit`, `commit-msg`, `post-merge`,
`post-rewrite`) and signing follow the configuration. Stopped (conflict or other): conflicted files
from the status and those git reports as conflicted (`Merge conflict in <file>`, and with
`rerere.autoUpdate` `Resolved '<file>' using previous resolution`: even a conflict resolved
by rerere leaves the merge/rebase stopped and counts as a conflict; `stopped_conflicts`, the same
function as `merge.start`; a name from the output is added only if no path in the status has
the same text, so a non-UTF-8 name does not appear twice). Merge (`MERGE_HEAD` after `git
merge`): no `--abort`, result `in_progress`, with the same checks as a successful merge
as `warnings` (HEAD still on the branch, the branch still at its previous value, `MERGE_HEAD`
equal to the expected upstream). Rebase (`rebase-merge/` after `git rebase`): no `--abort` either,
result `in_progress` with the same checks as `warnings` (`head-name` the branch, the branch at its
previous value, `onto` the expected upstream) and the record written (a failure to write it is a
warning: `rebase.continue` would then conclude without the journal entry). The backup is never
removed with an operation in progress. Non-zero exit without an operation in progress: if the branch is as it was
nothing started, error (`NotAllowed` if git refuses so as not to overwrite files) and the backup is
removed; if the branch moved anyway, the error says so and names the backup, which stays, and
nothing goes into the journal. Afterwards: the branch must contain the upstream, HEAD on the branch, otherwise
`warnings`. `--assume-unchanged` files that differ between the two commits → `NotAllowed` (git would
overwrite them without checking).

## CliBackend: push

**Push** (`remote.push`, S26): `git push --porcelain --progress --no-follow-tags
--recurse-submodules=check [--set-upstream] [--force-with-lease=refs/heads/<dst>:<oid>]
--end-of-options <remote> refs/heads/<b>:refs/heads/<dst>`, with the fetch environment (no
terminal, stdin closed, `setsid`). `--recurse-submodules=check` in place of any
`push.recurseSubmodules=on-demand`, which would also publish the submodules (and refuses a push
that references unpublished submodule commits). Never `--no-verify`: `pre-push` hooks
run. stderr like the fetch (progress to the `JobContext`, the rest in `messages` or
in the error); the outcome from the stdout of `--porcelain` (`Git::run_streaming` with
`capture_stdout`): the ref's line `<flag>\t<from>:<to>\t<summary>` (` ` fast-forward, `+`
forced, `*` new, `=` already up to date, `!` rejected: `non-fast-forward`/`fetch first`,
`stale info`, `remote rejected`). Non-zero exit without that line: `Auth`/`RemoteUnreachable`
from the text like the fetch, otherwise with "failed to push some refs" a `pre-push` hook (or the
submodule check): `PushRejected` with the hook's output.

**Deletion on the remote** (`remote.delete_branch`): `git push --porcelain --progress
--no-follow-tags --force-with-lease=refs/heads/<b>:<oid> --end-of-options <remote> :refs/heads/<b>`,
same environment as the push; never `--delete` with a list, `--prune`, `--force`, `--mirror`. The
outcome is the `--porcelain` line of `refs/heads/<b>` (`-` deleted; `!` rejected, `stale info` also
for a branch already gone). Before the push `create_backup_ref("push-delete", <oid>)`.

**Tags** (`remote.push_tag`, `remote.delete_tag`, S47). The remote value: `git ls-remote --tags
--refs --end-of-options <remote> refs/tags/<name>`, same environment as the fetch; `--refs` leaves
out the peeled `^{}` lines and, since the pattern matches the tail of the names, only the line with
exactly `refs/tags/<name>` counts (ls-remote reads the fetch URL; the push may use `pushurl`: the
lease covers a difference). The push: `git push --porcelain --progress --no-follow-tags
--recurse-submodules=check --end-of-options <remote> refs/tags/<n>:refs/tags/<n>`, never `--force`
or `+`; `=` → up to date, `!` with `(already exists)` → `tag_exists`. The deletion: as for branches
with `refs/tags/<n>` (shared code). Backup of the remote value: `create_backup_ref("push-delete",
<oid>)` if `cat-file -e <oid>` finds the object here; otherwise `git fetch --progress --no-tags
--no-write-fetch-head --no-recurse-submodules --no-prune --no-auto-gc --refmap= --end-of-options
<remote> refs/tags/<n>:refs/project/backup/push-delete-<s>-<ns>` (a free name; without `+` an existing
ref is never overwritten): only that ref is written (`--refmap=` keeps a configured refspec such as
`+refs/tags/*:refs/tags/*` from updating the local tag too, `--no-tags` from following others). The
backup must then equal `expected`; otherwise it is removed and `stale_lease` (the tag moved since it
was read), nothing deleted.

## Undo of the pull

`remote.pull` goes into the journal when it moves the branch: `remote.pull` entry with the branch from
`before` to `after`, HEAD equal before and after, the backup `refs/project/backup/pull-...` (the
branch as it was, saved before writing) and the description `Pull of `main` (fast-forward: from
a1b2c3d to e4f5a6b)` / `(merge of origin/main: ...)` / `(rebase onto origin/main: ...)`. The fetch
done by the pull does not, like `remote.fetch`.

- **Active branch: undo and redo with index and worktree** (`GitRefs::reset_branch`): moving
  only the ref (as for a commit) would leave the pull's files in the worktree, which would look like
  local changes to undo by hand. First the same checks as `restore_refs`
  (operation in progress, HEAD and branch as the pull left them, otherwise `UndoConflict`) and
  the backups of the abandoned commits (the merge commit, the commits rewritten by the rebase:
  `refs/project/backup/undo-...`), then `git -c core.fsmonitor=false reset --keep <oid>`: the files
  that differ between the two commits return as they were, **only if they have no local changes** (otherwise
  `NotAllowed` and the entry stays), the other changes stay, and an untracked file is not
  overwritten. Even before that it is checked that no file to recreate exists in the worktree
  (`reset --keep` would overwrite an ignored one) and `--assume-unchanged` files. Limit: between the
  ref check and `reset` there remains an instant in which another process could move the
  branch (`reset` updates with the value it reads itself); the commit moved from outside would remain in the
  reflog. The redo is the same move in reverse (the later commit is kept by the undo's
  backup).
- **Non-active branch**: only the ref (fast-forward), undo and redo with `restore_refs`.
- A push cannot be undone from here; after a push the undo of a previous pull remains possible
  (it checks only the local branch), but the remote already has the commits.

The `MemoryBackend` does the same with its checkout checks; it records pull backups in
`commit_backups` (`refs/project/backup/pull-<n>`) and those of force pushes (`push-<n>`). The pull's
in-memory merge is that of `merge.start` (`merge_three_way`): with conflicts it leaves the merge in
progress (`u` entries with `>>>>>>> origin/main` markers, `MERGE_HEAD`, `MERGE_MSG`); the rebase
replays the local commits with the in-memory rebase of [rebase.md](rebase.md) (those whose changes are
already upstream dropped, like git's `--empty=drop`) and a conflict leaves it in progress.

**Stopped merge or rebase and journal**: the stopped pull does not go into the journal (the branch has
not moved) and while the operation is in progress `edit.undo`/`edit.redo` are refused. The entry
arrives with `merge.continue` (undo like a concluded merge, [merge.md](merge.md)) or with
`rebase.continue` (the pull's own entry); `merge.abort`/`rebase.abort` do not go into the journal, and
`can_undo` after the abort refers to the previous entry.

## The fetch does not go into the journal

**The fetch does not go into the journal** (choice, S25): it moves only remote branches and tags, which
mirror the remote. Moving them back would show a remote that no longer exists (and the next
fetch would put them back where they were), and nothing local is lost: objects are only
added, local branches, index and worktree are not touched, and a remote branch removed
by `fetch.prune` has disappeared from the remote. Entries already in the journal are not affected: they check only
HEAD and the refs they moved. The exception is a refspec configured toward `refs/heads/` (a
mirror): the previous value stays in the ref's reflog and the result reports it in `warnings`.
