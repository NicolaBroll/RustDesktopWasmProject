# Reset of the current branch

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

Code: `crates/engine/src/git/reset.rs` (types and shared checks), `crates/engine/src/builtin/reset.rs`
(commands), `crates/engine/src/git/memory/reset.rs`, `crates/git/src/backend/reset.rs`.

## Commands

### `branch.reset`

Parameters: `{"target", "mode": "soft"|"mixed"|"hard", "confirm"?, "expected"?}`. Writes.

Moves the branch of HEAD (or a detached HEAD) to `target` (a rev resolved to a commit), like `git
reset --<mode>`:

- **`soft`**: only the ref. Index and worktree stay.
- **`mixed`**: the ref and the index (as the target's tree); the worktree stays. The staged versions
  that exist only in the index (different from HEAD and from the worktree) are saved first in
  `refs/project/backup/index-...`, on its own, as stage/unstage do (`index_backup`).
- **`hard`**: the ref, the index and the worktree of the tracked files. The local changes it
  overwrites (`files`: every tracked file with staged or unstaged changes, the original of a
  rename, every conflicted file; submodules skipped) need `"confirm": true` and are saved first as
  in `worktree.discard`: `git stash create` (worktree, with the index as second parent) and the
  byte-for-byte copy of the files git would not recreate identically (CRLF with
  `core.autocrlf=input`, filters) in a child commit; ref `refs/project/backup/reset-...` on the raw
  commit if there is one, otherwise on the stash. `git stash apply --index <stash>` puts the changes
  back. If the backup fails, nothing is written. A `hard` without local changes needs no
  confirmation.

`expected`: **full** id of the commit HEAD was on when the reset was confirmed (the `expected` of
`ConfirmRequired`, or of `branch.reset_preview`); if HEAD is elsewhere → `NotAllowed`, nothing
changes. Without `expected` the check covers only the instant between the command's read and the
write (below).

Refused before writing (`NotAllowed`, nothing changes): merge, rebase, cherry-pick, revert, bisect
or `git am` in progress; branch without commits; `hard` with `intent-to-add` files (the reset would
delete them, `git stash create` cannot save them); `hard` with an untracked or ignored file, link or
folder where the target puts a file or a folder (`read-tree -u` would overwrite or delete it, the
backup does not contain it; a folder of tracked files that the target replaces with a file is
refused only if it also holds untracked or ignored files); `mixed` and `hard` with a
`--assume-unchanged` (or, without sparse checkout, `--skip-worktree`) file whose index entry differs
from the target. `target` that does not exist → `UnknownRev`; one that is not a commit (a tree,
a blob) → git error (`rev-parse --verify <rev>^{commit}`: "expected commit type"), nothing changes. Empty `target` or unknown fields →
`InvalidParams`.

Result: `{"mode", "branch": string | null (null = detached), "before", "after", "files": [path],
"backup": string | null, "stash": oid | null, "index_backup": string | null, "backups": [string],
"published": n, "warnings": [Message], "status", "refs", "can_undo", "can_redo", "next_undo",
"next_redo"}`. `backups`: refs saved for commits that would no longer be reachable
(`refs/project/backup/reset-...`, e.g. a target that is not an ancestor). `published` and the
warning `engine-reset-published`: how many commits leaving the branch are already on its upstream
(the next push will have to overwrite the remote branch, with lease).

### `branch.reset_preview`

Parameters: `{"target", "mode"}`. Read-only.

The plan of `branch.reset` with the same refusals, for the confirmation dialog: `{"mode", "branch",
"before", "after", "files": [path], "needs_confirm", "expected", "published", "upstream": string |
null, "warnings"}`. `files` are those a `hard` overwrites (empty for `soft`/`mixed`); the UI
repeats `expected` in `branch.reset`.

## Steps and guarantees

The command reads the status, resolves the target and computes the plan (`reset_files`); then it
passes the backend a `ResetRequest`: the ref move as a `RefRestore` (the same as undo: HEAD and the
branch with their expected values), the mode and the confirmed files. Shared by the backends:
`reset_files`, `check_same_reset_files`, `strict_conflicts`, `reset_moved`.

The `CliBackend`, in this order (nothing is written if a step before 5 fails):

1. `RefRestore::check` (operation in progress), HEAD and the branch equal to the expected values
   (otherwise `NotAllowed` `engine-reset-moved`; `UndoConflict` for undo/redo), target a commit
   (`rev-parse --verify <oid>^{commit}`).
2. Status without `core.fsmonitor`; for undo/redo, `strict_conflicts`.
3. `mixed` and `hard`: hidden files (`diff-index --cached <target>` against the
   `--assume-unchanged`/`--skip-worktree` entries). `hard`: the plan equal to the confirmed one; untracked in the way:
   `diff-index --cached --diff-filter=D <target>` gives the paths of the target not in the index,
   each must not exist on disk (unless it is in the plan, i.e. in the backup) and every folder of
   its path must be a real folder or a tracked file; a file of the plan must not be a folder.
   `index.lock` → `IndexLocked` (also for `mixed`).
4. Backups. With conflicted files `git stash create` runs on a temporary copy of the index
   (`GIT_INDEX_FILE`) where each conflict is collapsed to "ours" (stage 2, otherwise 3): git can
   write the tree and saves the worktree of the conflicted file (with its markers). Last
   verification that the backup has exactly the current worktree (as in `worktree.discard`).
5. The ref move with the transaction of `edit.undo` (`update-ref --stdin` with the old value, under
   the ref lock; detached: `option no-deref` + `update HEAD <new> <old>`), backups of the commits no
   longer reachable, reflog `project: reset (<mode>) to <oid> (<target>)`.
6. HEAD re-read: if it is not the requested one (another process at that instant), index and
   worktree are not touched and the error says the branch has moved.
7. `mixed`: `git read-tree --reset <oid>` and `update-index -q --refresh`; `hard`: `git read-tree
   --reset -u --no-recurse-submodules <oid>` (no hooks; `submodule.recurse` does not apply). An
   error here says the branch has already moved (from which commit) and names the backups.

Limits: an editor saving a file between the last verification of the backup and `read-tree` loses
that change (the same instant as `worktree.discard`). Nothing enters the journal when step 7 fails.

The `MemoryBackend` does the same checks; the worktree backup is a `WorktreeStash` in
`worktree_backups()` (`refs/project/backup/reset-<n>`), the index backup that of stage.

## Undo and backup

A reset that changes the commit enters the journal (`JournalEntry::reset`, `reset: {"mode", "stash"}`,
`backup` the worktree backup); a `hard` on HEAD itself does not (its recovery is the backup).
`edit.undo`/`edit.redo` call `GitRefs::reset` with the same mode and `strict`: HEAD and the branch
must be as the reset (or the undo) left them (`UndoConflict`), and nothing may need a new backup:
`mixed` refuses with staged changes or conflicts (the index is rewritten as the other commit),
`hard` with any local change to tracked files (the worktree is rewritten). The undo of a `hard`
does **not** put the overwritten local changes back: the warning `engine-reset-undo-backup` names
the backup and `git stash apply <stash>`. Putting them back automatically would be a second
`stash apply` on another commit, with possible conflicts: it stays a manual step.
