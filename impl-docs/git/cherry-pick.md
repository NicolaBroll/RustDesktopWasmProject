# Cherry-pick and revert

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md) (§1, §5, §7); the model is the merge in
[merge.md](merge.md), of which only what changes is described here.

Types and checks shared by the backends are in `crates/engine/src/git/pick.rs` (`PickKind`, `PickRequest`,
`pick_parent`, `pick_message`, `check_pick_state`, `check_picking`, `pick_empty_error`); commands
in `crates/engine/src/builtin/pick.rs`; `CliBackend` in `crates/git/src/backend/pick.rs` (the abort
in `backend/merge.rs`, shared with the merge); `MemoryBackend` in `crates/engine/src/git/memory/pick.rs`.

## Commands

### `commit.cherry_pick`, `commit.revert`

Parameters: `{"rev", "mainline"?: n}`.

They write (`writes`), and go into the journal when the operation concludes. One commit at a time:
cherry-pick applies the changes of `rev` on top of the HEAD branch with a new commit
(**original author and message**, without `-x`); revert applies the inverse changes
(git's message: `Revert "<subject>"` + blank line + `This reverts commit <id>.`; for a merge
commit `This reverts commit <id>, reversing\nchanges made to <parent>.`; the revert of a
revert is `Reapply "..."`, like git since 2.43).

- `rev` is resolved with `GitBackend::log` (not found → `UnknownRev`); only the id reaches git after
  `--end-of-options`. Empty or with NUL → `InvalidParams`.
- `mainline`: a merge commit requires it (from 1 to the number of parents; without it → `InvalidParams`
  asking for it); on a non-merge commit (even if git would accept it) or out of range →
  `InvalidParams`.
- **Refusals** (`NotAllowed`, nothing changes) before the backend and again in the backend on the state
  read just before writing (`check_pick_state`): detached HEAD, branch without commits,
  operation in progress, conflicted files, **any** change to tracked files (staged or
  not: commit them or `stash.push`, never autostash, like the merge commit). Always refused: files
  that the patch writes in place of an untracked or ignored one and `--assume-unchanged` files that the
  patch changes.
- **Already applied / empty result** → `NotAllowed` ("would change nothing"): no commit,
  nothing in progress (no `CHERRY_PICK_HEAD`), backup removed.
- **Backup**: before writing `refs/project/backup/cherry-pick-...` / `revert-...` on the branch
  as it was (removed if nothing changes).
- **Conflicts**: the operation **stays in progress** (`CHERRY_PICK_HEAD` / `REVERT_HEAD`), result
  with `in_progress: true`, `conflicts`, `message` (git's text); the branch has not moved,
  no journal entry, the backup remains. A commit that failed because of signing or a hook also stays in progress (without
  conflicts, the changes staged).

Result: `{"rev", "commit", "summary", "mainline", "branch", "before", "after", "in_progress",
"conflicts": [path], "message", "backup", "warnings", "status" | null, "refs" | null,
"can_undo", "can_redo", "next_undo", "next_redo"}`.

### `cherry_pick.continue`, `revert.continue`

Parameters: `{"message"?}`. Like `merge.continue`: `GitBackend::commit` with
`CommitRequest::concluding(op)` (the only way to pass `check_commit_state` with a cherry-pick
or revert in progress; `commit.create` stays refused). With `CHERRY_PICK_HEAD` git keeps the original
author. Message: `message`, otherwise `MERGE_MSG` without the `# Conflicts:` block,
otherwise `pick_message` of the incoming commit. Refusals (`NotAllowed`): no operation of
that type in progress (even during a merge or the other type), a sequence from the terminal (below), files
still conflicted, result equal to HEAD (resolved by keeping our version: nothing to
commit, the operation stays in progress and the message points to `*.abort`). Result as
`merge.continue`.

### `cherry_pick.abort`, `revert.abort`

Parameters: `{"confirm": true}`. Like `merge.abort`: always `confirm` (`ConfirmRequired` with the
files), then a stash-shaped backup of index and worktree in
`refs/project/backup/cherry-pick-abort-...` / `revert-abort-...` (same code as the merge), then
`git cherry-pick --abort` / `git revert --abort`; if it fails the error names the backup, which
remains. It does not go into the journal. Same refusals as `*.continue`.

### Sequence of several commits started from the terminal (choice)

`git cherry-pick a b c` creates `.git/sequencer/`; a cherry-pick of a single commit does not. With
`.git/sequencer` present `*.continue` and `*.abort` are `NotAllowed` (`check_picking`) and
`repo.operation` gives `continue`/`abort` `null`: `git commit` would leave the sequence half done and
`git cherry-pick --abort` would also roll back the commits already applied, outside the journal.
It is concluded from the terminal. `OperationState::sequencer` reports it.

## CliBackend

Reads the state without fsmonitor (`check_pick_state`), HEAD on the branch with the expected value, the
`--assume-unchanged` files changed by the patch (`diff-tree from to`, with `from`/`to` = parent →
commit for cherry-pick and the reverse for revert; empty tree for a root commit) and the
files the patch writes (`--diff-filter=AMT`) that HEAD does not have and that exist on disk; then the
backup and, with `GIT_REFLOG_ACTION=project: <id>` and `GIT_EDITOR=:`:

`git -c core.fsmonitor=false -c revert.reference=false <cherry-pick|revert> --no-edit
--no-rerere-autoupdate [-m <n>] --end-of-options <oid>`

`--no-edit` (no editor), `revert.reference=false` (the message stays the one above),
`--no-rerere-autoupdate` (against `rerere.autoUpdate`: a file resolved by rerere stays
conflicted, to be confirmed). Hooks and signing according to configuration, never `--no-verify`, never
`--no-commit`, `--allow-empty` or `--keep-redundant-commits`.

Afterwards: succeeded without an operation in progress but with the branch unmoved → `Git` naming the backup;
succeeded → the branch afterwards (`warnings` if HEAD is no longer on the branch); in progress: with conflicts (from the
status and the output) or without (signing, hook) → `in_progress`; in progress and empty (`is now empty`) →
`git <cmd> --skip`, backup removed, `NotAllowed`; nothing in progress: empty (the revert of a commit already
undone ends this way) → `NotAllowed`, otherwise `NotAllowed` with the files (`would be overwritten`) or
`Git` with git's text; the backup is removed only if the branch still has its previous value.
A `MERGE_MSG` left by git without an operation in progress (and that was not there before) is removed.

## Undo and backup

- **Concluded** (immediately or with `*.continue`): `JournalEntry::pick` with the branch from `before` to
  `after`, HEAD equal, the backup (only the start command) and the description `Cherry-pick of
  <name> onto `<branch>` (from a1b2c3d to e4f5a6b)` / `Revert of ...`. Undo and redo like the merge:
  `reset_branch` (`git reset --keep`) with the same checks. In the journal only if the commit afterwards
  has the previous branch as its only parent, otherwise a warning in `warnings` naming the backup.
  `*.continue` with a detached HEAD (started from the terminal) goes in as a commit.
- **Stopped on conflict**: no journal entry; undo and redo refused while it is in progress.
- **Abort**: does not go into the journal; recovery is the `...-abort-...` backup.

The `MemoryBackend` applies the patch file by file (base = parent, theirs = commit; the reverse
for revert): a file changed differently from HEAD is a conflict (`u` entries, markers with
label `<id> (<subject>)` / `parent of ...`), the in-progress state in `MemoryMerge` (the commit and
`MERGE_MSG` with `# Conflicts:`); the concluding commit takes the original commit's author for
cherry-pick. Result equal to HEAD → `NotAllowed`. Backup in `commit_backups`
(`refs/project/backup/cherry-pick-<n>`, `revert-<n>`, `...-abort-<n>`); no sequencer.
