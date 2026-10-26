# Interactive rebase

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md) (§1, §5, §7); the model is cherry-pick in
[cherry-pick.md](cherry-pick.md) and the merge in [merge.md](merge.md), of which only what changes
is described here. The pull's rebase is in [sync.md](sync.md).

The plan is pure data in `crates/domain/src/git/rebase.rs`; shared types and checks
(`RebaseRequest`, `RebaseRecord`, `check_rebase_state`, `check_rebase_request`, `check_rebasing`,
`rebase_range`, `rewritten_commits`, `plan_error`) in `crates/engine/src/git/rebase.rs`; commands in `crates/engine/src/builtin/rebase.rs`; `CliBackend` in
`crates/git/src/backend/rebase.rs`; `MemoryBackend` in `crates/engine/src/git/memory/rebase.rs`.

## The plan (domain)

```rust
pub enum RebaseAction { Pick, Reword, Squash, Fixup, Drop }      // serde snake_case
pub struct RebaseStep { pub action: RebaseAction, pub commit: Oid, pub message: Option<String> }
pub struct RebasePlan { pub base: Oid, pub steps: Vec<RebaseStep> }
```

`base` is excluded: the commits rewritten are `base..HEAD` (first parent, linear), placed again on
`base` (the base does not change: that is the pull's job). `steps` is in the order of application
(oldest first, like the git todo).

Rules (`RebasePlan::validate(&self, commits: &[Oid])`, `commits` = `base..HEAD` oldest first):
every commit exactly once (dropping is an explicit `drop` step), no other commits; at least one
step that is not `drop`; the first non-`drop` step is not `squash`/`fixup` (nothing to combine with);
`message` required for `reword`, allowed only on the **last** `squash`/`fixup` of a group (it is the
message of the combined commit), forbidden elsewhere; a `reword` cannot start a group with
`squash`/`fixup` (use `pick` and put the message on the group's last step); message not empty or
blank after trimming and without NUL. A group is a `pick`/`reword` followed by the `squash`/`fixup`
after it; `drop` steps do not interrupt it. Errors: `PlanError { step: Option<usize>, reason:
PlanErrorReason }` (`step` 0-based, `None` for a missing commit or all dropped; English `Display`
with the step 1-based), mapped by the engine to `InvalidParams`. `RebasePlan::pick_all(base,
&commits)` builds the default plan.

`RebasePlan::is_noop(&commits)`: all `pick`, in the original order, without messages.

`RebasePlan::result()`: the commits the plan produces, in order: `RebasedCommit { sources: Vec<Oid>,
message: Option<String> }` (`sources[0]` the picked/reworded commit, then those squashed in; `message`
`None` = the git default: the original for `pick`, the combination of the messages for a group with
`squash`, the first commit's for a group with only `fixup`). Used by the `MemoryBackend` and by the preview
of S33.

`RebasePlan::todo()`: the `git-rebase-todo` text and the list of messages: `pick|squash|fixup|drop
<full oid>` one per line; `reword` is written as `pick` followed by `break`, and the last
`squash`/`fixup` of a group with a message is followed by `break`; the `drop` lines all go at
the end (so none sits inside a group); the messages, in the order of the
`break` lines, are applied at those stops by the backend with `commit --amend` (never the editor:
with `GIT_EDITOR=:` git takes its default message). No `exec`, `edit`, `label`, `merge`.

## Commands

### `rebase.plan` (read)

Parameters: `{"base": rev}`. Result: `{"base", "branch", "steps": [{"action": "pick", "commit",
"summary"}], "published": [oid]}`: the default plan of `base..HEAD` (oldest first), `published` the
commits of the range reachable from a remote-tracking branch (one `missing_commits(HEAD, [remote
branches..., base])`: the range minus what it returns). Refusals (`NotAllowed`): detached HEAD (or
a branch without commits), `base` not an ancestor of HEAD or equal to it, merges in the range, more
than `MAX_REBASE_COMMITS` (2000) commits; `base` empty → `InvalidParams`, unknown → `UnknownRev`.
`rebase_range` reads the range with `missing_commits(HEAD, [base])` and orders it by walking the first
parents from HEAD to `base`.

### `rebase.start` (writes)

Parameters: `{"base": rev, "steps": [{"action", "commit": oid, "message"?}], "confirm"?}`.

- `base` resolved with `log` (`UnknownRev`); plan validated against `base..HEAD` (`InvalidParams`);
  plan equal to the current one (`is_noop`) → `NotAllowed` ("would change nothing").
- **Refusals** (`NotAllowed`) before the backend and again in the backend on the state read just before
  writing: detached HEAD, operation in progress, conflicted files, any change to tracked files
  (no autostash), merges in the range, `base` not an ancestor of HEAD.
- **Confirmation**: if one of the commits from the first changed step onward is published
  (reachable from a remote-tracking branch) → `ConfirmRequired` naming them, as `commit.create`
  with `amend` (`rewritten_commits`: git keeps the leading unchanged `pick`s as they are, so a
  published commit before the first change needs no confirmation).
- The backend checks again (`check_rebase_request`: the range read again must equal the
  command's, the plan valid against it), plus HEAD on the branch with the expected value,
  `--assume-unchanged` files changed between `base` and HEAD, and the files the rebase writes in
  place of an untracked or ignored one (the trees of `base` and of every commit of the range,
  like the pull's rebase: `NotAllowed`, nothing written).
- **Backup** before writing: `refs/project/backup/rebase-...` on the branch as it was (it stays: the
  rewritten and dropped commits remain reachable from it; removed only if the branch has not moved
  and nothing is in progress).
- **Stops** (conflict, failed hook or signing, `break` whose amend failed, a lock): the rebase
  **stays in progress**, result with `in_progress: true`, `conflicts`, `message` (git's text, or the
  amend's for a rejected message); `after` is the branch, which git moves only at the end (HEAD is
  detached on the commit reached); no journal entry until it concludes, the backup remains.
- Concluded: journal entry if HEAD is back on the branch and the branch moved, otherwise a warning
  naming the backup. `warnings` also when the commits between `base` and the branch are not the
  ones the plan produces (`RebasePlan::result().len()`).

Result: `{"base", "branch", "before", "after", "in_progress", "conflicts": [path], "message",
"backup", "warnings", "status" | null, "refs" | null, "can_undo", "can_redo", "next_undo",
"next_redo"}`.

### `rebase.continue` (writes)

Parameters: `{}`. With no conflicted files left: `git rebase --continue` (`GIT_EDITOR=:`: the
commit stopped on keeps its message), then the remaining `break` stops of the plan with their
messages as in `rebase.start`. Stopped at a `break` whose amend failed (a `commit-msg` hook, signing):
the amend is retried first with the same message (the message cannot be changed once started:
abort and start again), then `--continue`. Refusals (`NotAllowed`): no rebase in progress, files
still conflicted; a locked index → `IndexLocked`.

**A step resolved to nothing** (index equal to HEAD after a conflict on a `pick`): git **drops the
commit** and goes on, even with `--empty=keep` (that option only concerns commits that become empty
while being applied, not a resolution). The command says so in `warnings`
(`engine-rebase-dropped-empty`, and when concluded the count of commits differs from the plan's);
the dropped commit stays reachable from the backup. Tested in both backends.

Concluded → journal entry (below): `rebase.continue` for an interactive rebase, `remote.pull` for a
stopped pull; a rebase started from the terminal (no record) concludes without a journal entry, with
a warning. Result: `{"command": "rebase.start" | "remote.pull" | null, "base", "branch", "before",
"after", "in_progress", "conflicts", "message", "backup", "warnings", "status", "refs", "can_undo",
"can_redo", "next_undo", "next_redo"}` (`before`/`backup` `null` without a record).

### `rebase.abort` (writes)

Parameters: `{"confirm": true}`. Like `merge.abort`: always `confirm` (`ConfirmRequired` with the
files), a stash-shaped backup of index and worktree in `refs/project/backup/rebase-abort-...` (its
parent is the commit the rebase had reached, so the commits already rewritten stay reachable), then
`git rebase --abort` (the branch returns to where it was before the rebase, HEAD back on it). Not in
the journal. Refusals: no rebase in progress. `git rebase --abort` is a hard reset to the branch as
it was: unlike `merge --abort` it has no limit on files both staged and changed, but it **refuses
halfway an untracked file** where that commit has a file, or a file in place of one of its folders
(`d` for `d/f`): the abort refuses first, `NotAllowed`, nothing written; it **overwrites an ignored
one** without saying so: it goes in the backup too, and `warnings` names it. Also for a rebase started by `remote.pull` or from the terminal.

`repo.operation` gives `continue`/`abort` = `rebase.continue`/`rebase.abort` for a rebase (also one
started from the terminal: continuing it concludes it without a journal entry, with a warning).

## Pull with rebase

With these commands a pull with rebase that stops on a conflict **stays in progress** like the
merge: result `in_progress: true`, `conflicts`, `message`; `rebase.continue` concludes it with the
pull's journal entry (`Pull of origin/main into main (rebase ...)`), `rebase.abort` returns the branch
where it was. A rebase stopped for another reason (hook, signing) also stays in progress.

## Undo and backup

Concluded (immediately or with `rebase.continue`): `JournalEntry::rebase` with the branch from
`before` to `after`, HEAD equal, the backup and the description `Rebase of `<branch>` (n commits, from
a1b2c3d to e4f5a6b)` (`n` the commits of the range); `rebase.start` and `rebase.continue` are in
`WORKTREE_COMMANDS`: undo and redo with `reset_branch` (`git reset --keep`) and the usual checks. The
data the continue needs is the `RebaseRecord` (`command`, `branch`, `before`, `onto`, `backup`,
`messages` of the `break` stops, `message_commits` = for each the commit whose step the `break`
follows, `applied` = how many were applied or skipped, `commits`, `produces` = the commits the plan
produces, `upstream` for the pull), in
`<git dir>/rebase-merge/project-rebase.json`, removed by git together with the directory when the
rebase ends or is aborted; `OperationState::rebase` exposes it. A failure to write it is a warning
(the continue would then conclude without journal entry). The `CliBackend` ignores a record whose
`before`/`branch` differ from `rebase-merge/orig-head`/`head-name` (left over, or a rebase restarted
from the terminal): the continue then concludes like a terminal rebase, without journal entry and
with the warning.

## UI

`crates/ui/src/rebase.rs` (model, `RebaseBox`), `view/rebase_view.rs`, tests in `rebase_tests.rs`,
texts in `rebase.ftl`. Commit menu "Interactive rebase from here…": base = first parent of the
chosen commit, offered only for a commit on HEAD's first-parent history (loaded commits) with the
merge gate (branch with commits, no operation, no conflicts). The editor reads `rebase.plan` and
lists the steps **oldest first** as in the git todo (squash/fixup join the row above): drag handle
(egui dnd) and up/down arrows, action selector, published badge, dropped rows struck through. The
message is edited for the selected row, only where the plan takes one (`reword`, last
`squash`/`fixup` of a group; blank = git's default, `None`). The "Result" pane is
`RebasePlan::result()`; `validate` errors are shown per `PlanErrorReason` (the row highlighted) and,
like `is_noop`, disable "Run". `rebase.start` → `ConfirmRequired` opens a confirmation with the
engine's reason and resends with `"confirm": true`; engine refusals stay in the editor; done → notice
with the backup and Undo; stopped → the stage area and the operation panel, like the pull's rebase.

## CliBackend

Plan written to `<git dir>/project-rebase-todo` and copied by the sequence editor:
`GIT_SEQUENCE_EDITOR="cp '<path>'"` (the only shell, git's; the path quoted for `sh`), `GIT_EDITOR=:`,
`GIT_REFLOG_ACTION=project: rebase.start`:

`git -c core.fsmonitor=false -c rebase.updateRefs=false -c rebase.abbreviateCommands=false -c
rebase.missingCommitsCheck=error -c rebase.instructionFormat= -c sequence.editor= rebase -i
--no-autostash --no-autosquash --no-rerere-autoupdate --empty=keep --keep-empty
--end-of-options <base oid>`

(`rebase.updateRefs=false` and not `--no-update-refs`, which exists only from git 2.38; `--empty=keep`: a commit that becomes empty is kept, never dropped silently). At each `break` stop:
`git commit --amend --only --cleanup=whitespace -F -` with the message on stdin (`#` lines kept whatever `commit.cleanup` says) (hooks and signing according to
configuration), then `git rebase --continue`. Which message: the `break` lines already executed
are counted in `rebase-merge/done`; the stop is the `n`-th, and `messages[n-1]` is applied if
`applied < n` and the step executed just before the `break` is `message_commits[n-1]` (so a
retried continue never applies a message twice and never loops). A `break` moved from the
terminal after another step: the message is not applied, warning (`engine-rebase-message-skipped`);
a `break` without a message (added from the terminal): the rebase just goes on. Concluded with
messages not applied (e.g. continued from a terminal after a failed amend) → warning
(`engine-rebase-messages-not-applied`).

**Squash groups without a message**: the `CliBackend` makes git's default explicit before writing
the todo (`explicit_squash_messages`: the original messages, `%B`, of the `pick`/`reword` and
`squash` steps joined by a blank line, applied at a `break` like a plan message). Git would build
it from its comment template, cleaned according to `commit.cleanup` (with `verbatim`/`whitespace`
the `# This is a combination...` lines would stay; with `strip` a `#` line of the original
messages would go). `-c commit.cleanup=strip` on the rebase is not an option: it would also strip
the `#` lines of every picked message. Afterwards:
in progress → `in_progress`; not in progress and the branch unmoved → git did not start (`NotAllowed`
for files it would overwrite, otherwise `Git` with its text) and the backup is removed; branch
gone or unmoved after a run → `GitWithReason` naming the backup. The plan file is removed in every
case. The leading `pick`s equal to the original are fast-forwarded by git (the same commits).

The pull (`backend/sync.rs`) no longer aborts a stopped rebase: it writes the record (command
`remote.pull`) and returns `in_progress` ([sync.md](sync.md)).

## MemoryBackend

Applies the plan commit by commit on `base` (`memory/rebase.rs`: the patch of each source, parent →
commit, file by file as cherry-pick) with HEAD detached, as git; a `squash`/`fixup` rebuilds the
group's commit on the group's parent; the group's message is the plan's or git's default (the
messages of `pick`/`squash` joined by a blank line); an unchanged leading `pick` keeps the original
commit. A conflict stops it: `u` entries and markers as cherry-pick, the remaining steps in
`MemoryRebase` (with the record); `rebase.continue` commits the resolution (the index; resolved to
nothing → dropped with the warning, as git) and goes on; `rebase.abort` returns the branch, index and
worktree to before (backup `rebase-abort-<n>`). No hooks: the amend stops are not simulated. The
pull's rebase uses the same code with `--empty=drop` semantics.
