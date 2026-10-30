# Discarding changes

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

## Commands

### `worktree.discard`

Parameters: `{"paths", "confirm"}`.

With backup (below).

## Steps and guarantees

`worktree.discard {"paths", "confirm"}` returns the worktree of the files `paths` (files or directories,
taken literally) to the staged version, like `git restore --worktree`. Choices:

- **Worktree only.** The index is not touched: an `MM` file becomes `M.`. To remove the stage too
  there is `index.unstage` (which has its own backup).
- **`"confirm": true` is mandatory.** Without it, `ConfirmRequired` with the list of files that would
  be lost and nothing changes; the refusals below come before the confirmation.
- **Only tracked files with unstaged changes** (Y ∉ {`.`, `?`, `!`}: modified, deleted,
  type-changed, also mode). **Untracked files are never touched**: requested by name
  they are an error (discarding them would mean deleting them, and `git stash create` does not save them, so
  there would be no backup); inside a directory they stay where they are. No option to
  delete them: if needed it will be a separate command (`git stash create` has no `-u`, another
  backup would be needed). Submodules requested by name are refused, inside a directory skipped; the
  `--assume-unchanged`/`--skip-worktree` files have Y = `.` and are not touched.
- **When the backup cannot be made, nothing is discarded**: branch without commits ("You do not have
  the initial commit yet"), conflicted files in the repo ("Cannot save the current index state"),
  `intent-to-add` files in the repo ("not uptodate. Cannot merge", even with git 2.56). The checks
  are in `discard_plan`, shared by the backends.

- **Nothing in place of directories.** An untracked or ignored file in place of a directory in the
  path of a file to discard (`top/d/x` tracked, the directory deleted and a file
  `top/d` created) or in place of the file itself (a directory `x/`) would be deleted by `git
  restore` to recreate the file, and the backup does not contain it: `discard_plan` refuses if the status
  shows it, and the `CliBackend` also checks the file system (`symlink_metadata` of every
  directory in the path: it must be a real directory, not a file or a link; the file must not
  be a directory), so ignored files and links are covered too. The check is repeated before
  the undo and the redo.
- **The confirmed plan.** `GitIndex::discard` also receives the plan computed by the command
  (what the confirmation lists): if the plan read by the backend before writing is different
  (one file more or less, another staged version), `NotAllowed` and nothing changes. The
  plan is computed inside the same command: between what the UI showed and the command
  the guarantee holds only if the UI passes the exact files, not the directories.

The `CliBackend`, in this order: status without fsmonitor, `discard_plan` and comparison with the
confirmed plan; index (`ls-files -s`) equal to the status for those files; directory check;
`index.lock` → `IndexLocked` immediately; `git -c core.fsmonitor=false stash create
<fixed message>` with the fixed backup identity (a commit with the worktree of the tracked files;
it touches neither index, worktree nor `refs/stash`; if it fails because the index is locked, `IndexLocked`);
**byte-for-byte backup**: for each file the worktree bytes are compared with those `git
restore` would recreate from the commit (`cat-file --filters --path=<file>`; for a link the
target); if they differ (e.g. CRLF with `core.autocrlf=input`, which the `clean` filter removes
and `smudge` does not put back, or a non-reversible filter) the bytes are saved without filters
(`hash-object -w --no-filters`) in a commit with only those files, child of the `stash
create` one (the entry's `raw` field); `update-ref refs/project/backup/discard-<seconds>-<nanoseconds>
<raw or stash> ""`; re-check of the index and directories; **final verification** that the backup
has exactly the current worktree (`git diff --name-only -z <stash> -- <file>` empty, for the
mode, without `diff.autoRefreshIndex=false`, which would make files changed only in the `stat` data
appear, with `--no-optional-locks`; and the bytes as above, or those of `raw`); finally `git
restore --worktree --no-overlay` of only the files in the plan (paths on stdin). Only the
instant between the last verification and `restore` remains, in which an editor saving a file would lose that
change.

The journal entry has `"worktree": {"stash", "files": [{"path", "change", "mode", "oid"}],
"raw"}` (the mode and the id of the index entry the file was returned to; `raw` is `null`
if not needed) and `"backup"` with the ref. `Journal::undo`/`redo` call
`GitIndex::restore_worktree` instead of `restore_refs`, and first check HEAD: it must be
the recorded one (`head.after` for the undo, `head.before` for the redo, equal for a discard),
otherwise `UndoConflict` (a checkout or a commit from the terminal) and nothing changes.

- **undo**: each file must still have the recorded entry in the index and the worktree equal to
  the index (no unstaged changes, no flag that would hide it); otherwise
  `UndoConflict` with one line per file and nothing changes. Then `git restore --source=<stash>
  --worktree --no-overlay` (a file that is not in the commit, because it was deleted, is deleted
  again) and the `raw` files rewritten with their bytes. The overwritten worktree is equal to the
  index: nothing is lost.
- **redo**: same index entry and worktree equal to the backup (same bytes and mode, as above:
  even a change of line endings alone blocks it), then `restore --worktree` again. No new
  backup: the backup already has that content.
- After writing, it is re-read (bytes and mode for the undo, equal to the index for the redo): a file
  not as expected goes in `warnings`.

The `MemoryBackend` records backups in `MemoryBackend::worktree_backups`
(`refs/project/backup/discard-<n>`, with the status entry and the content of each file).
