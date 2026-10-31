# Discarding changes

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

## Commands

### `worktree.discard`

Parameters: `{"paths", "confirm"}`.

With backup (below).

### `worktree.discard_lines`

Parameters: `{"path", "lines", "digest", "confirm"}`. Discards only some lines, with backup
("Discarding lines" below).

### `worktree.clean`

Parameters: `{"paths", "confirm"}`. Deletes untracked files, with backup ("Deleting untracked
files" below).

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
  there would be no backup); inside a directory they stay where they are. They are deleted
  by the separate `worktree.clean` (below), which has its own backup. Submodules requested by name are refused, inside a directory skipped; the
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

## Discarding lines

`worktree.discard_lines {"path", "lines", "digest", "confirm"}` (code:
`crates/engine/src/git/discard_lines.rs`, `crates/git/src/backend/discard_lines.rs`): the chosen
changed lines of the `diff.worktree` diff of `path` go back to the index version, the others stay in
the worktree. `lines` and `digest` as for `index.stage_lines` ([stage.md](stage.md)): non-empty,
numbers of the seen diff, `StaleDiff` if the reread diff has another digest. Choices:

- **The patch.** `discard_line_patch` (in `lines.rs`, sharing the file choice and the digest check
  with `line_patch`) applies the `build_patch` **unstage** rule to the worktree diff: there the
  worktree is the new side, as the index is in `diff.index`, so the patch's preimage is the clean
  worktree content and its postimage the content with the selected lines back as in the index. The
  new content is computed with the strict `apply_patch` (each hunk only at its position).
- **Refusals**, always before the confirmation and without writing: those of `worktree.discard`
  (`discard_lines_target` calls `discard_plan` on the path: branch without commits, conflicted or
  `intent-to-add` files in the repo, untracked, submodule, something in the way) and those of the
  line commands (binaries, links, two files on the path, a file stored in Git LFS on the `CliBackend`); only a **modified** file (`Change::Modified`,
  same path on both sides): a file deleted from the worktree, renamed or `intent-to-add` →
  `NotAllowed`, discard the whole file.
- **`"confirm": true` is mandatory.** Without it `ConfirmRequired` with how many worktree lines would
  be lost and how many index lines would come back.
- **Filters and line endings.** The diff is of the clean content. The `CliBackend` cleans the bytes
  on disk (`hash-object -w --stdin --path=<path>`): they must give the diff's new id, otherwise
  `StaleDiff`. It also requires that smudging that blob (`cat-file --filters --path=<path>`) gives
  back exactly the bytes on disk; otherwise `NotAllowed` (discard the whole file): rewriting the file
  through the filters would also change lines that were not chosen (e.g. CRLF with
  `core.autocrlf=input`, a filter that does not round-trip). The bytes written are the smudged form
  of the new content (`hash-object -w --no-filters`, then `cat-file --filters`).
- **The index is never touched**, and the mode (executable bit) of the file is kept: the new file
  gets the old file's permissions.

The `CliBackend`, in this order: status without fsmonitor and `discard_lines_target`; the index
entry (`ls-files -s`) equal to the status one; the diff and `discard_line_patch`, with the diff's
old id equal to that entry; every directory on the path a real
directory and the file a regular file (`lstat`; bytes and permissions read once). The mode recorded (backup, result, journal) is the
diff's new mode, the one git sees: with `core.fileMode=false` or on Windows the permissions on disk
may differ from it, and they are never compared with it; clean and smudge checks
above; the new content and its bytes; **backup**: a commit with only the file and its exact bytes
and mode (`hash-object -w --no-filters`, temporary index, fixed identity, never signed) whose
**parent** is a commit with the bytes to write (so that both blobs of the journal entry stay
reachable after a `git gc`), held by `update-ref refs/project/backup/discard-<seconds>-<nanoseconds>
<commit> ""` and verified with `ls-tree`; if any of this fails nothing is written. **Last check**:
the directories again and the file still has exactly the bytes and permissions read (otherwise `StaleDiff`,
nothing written; the backup ref stays, harmless). Then the **atomic write**: a new temporary file
`.project-<pid>-<nanoseconds>.tmp` in the same directory (without the file's name, which could
exceed the name limit) (`create_new`), the bytes, the old
file's permissions, `sync_all`, `rename` over the file (on failure the temporary file is removed and
the error names the backup). The rename gives the file a new inode: a hard link to the old file
keeps the old content. Only the instant between the last check and the rename remains, in which an
editor saving the file would lose that save (it is in neither the backup nor the result). After
the write the file is reread: if it does not have the bytes written, a warning in `warnings`.

Result: `{"path", "mode", "before", "after", "backup", "warnings", "status", "status_error"?,
"can_undo", "can_redo", "next_undo", "next_redo"}` (`before`/`after`: blobs with the exact bytes
before and after).

The journal entry has `"lines": {"path", "mode", "before", "after"}` and `"backup"`; the
`"worktree"` entry of `worktree.discard` is not reused, because its undo requires the worktree equal
to the index, which after discarding only some lines is not the case. Undo and redo check, like
those of `worktree.discard`, that no operation is in progress and that HEAD is the recorded one,
then call `GitIndex::restore_lines`:

- **undo**: the file must be a regular file with exactly the bytes of `after` (the permissions are
  not compared: the write keeps them), otherwise `UndoConflict` and nothing changes; then the bytes of `before` are written
  atomically as above. Nothing is lost: the bytes overwritten are blob `after`.
- **redo**: the same with the roles swapped (exactly the bytes of `before`, then `after` written).
  No new backup.

The `MemoryBackend` has no filters: the worktree bytes are the clean content. Its backups are in
`MemoryBackend::line_backups` (`refs/project/backup/discard-<n>`, numbered with the
`worktree.discard` ones); `fail_line_backups` makes the backup fail.

## Deleting untracked files

`worktree.clean {"paths", "confirm"}` is the safe `git clean` for named files (code:
`crates/engine/src/git/clean.rs`, `crates/git/src/backend/clean.rs`). Choices:

- **Only untracked, non-ignored files, as `repo.status` lists them.** The status is read with
  `--untracked-files=all`, which overrides `status.showUntrackedFiles`: every untracked file is its own
  entry, so a directory path means the untracked files under it (no `ls-files --others`
  expansion is needed). Tracked, ignored and conflicted files inside a requested directory stay. A
  requested path with no untracked file (tracked, ignored, conflicted, missing, a prefix that is not
  a directory) → `NotAllowed` listing them, nothing deleted. The checks are in `clean_plan`, shared
  by the backends.
- **Nested repositories are refused**, by name or inside a requested directory (status lists
  them as one entry `dir/`; their history is not in the backup), and so is a path inside one.
  Submodules are tracked: never untracked files.
- **`"confirm": true` is mandatory.** Without it, `ConfirmRequired` with the files and their total
  size; the refusals come first. `GitIndex::clean` also receives the confirmed plan (`[{"path",
  "mode", "size"}]`) and refuses if the plan read before writing differs.
- **Links** are saved and deleted as links (`symlink_metadata`, `read_link`, `remove_file`): the
  target is never followed. A link to a directory is a file for git. A path whose directories
  include a link (or a non-directory) is refused, and checked again right before each deletion.

The `CliBackend`, in this order: status without fsmonitor, `clean_plan`, `lstat` of every file
(regular file, mode `100755` if the user execute bit is set, otherwise `100644`; link `120000`
with its target as content; anything else → `NotAllowed`) and comparison with the confirmed plan;
the bytes read now saved with `hash-object -w --stdin --no-filters` (no `clean` filter or
`core.autocrlf` conversion: CRLF bytes stay CRLF); a tree built in a **temporary index**
(`GIT_INDEX_FILE` in the git directory, `update-index -z --index-info`, `write-tree`; the user's
index is never touched or locked) and a commit **without parent** (`commit-tree --no-gpg-sign`,
fixed backup identity, the paths in the message); `update-ref refs/project/backup/clean-<seconds>-<nanoseconds>
<commit> ""`; **last verification**: the backup tree has every file with the expected mode and
blob, and the bytes on disk now hash to the same blob (`hash-object --stdin --no-filters` without
`-w`); only then `remove_file` of each file, and `remove_dir` of the directories the deletion left
empty, from each file's directory up, stopping at the first non-empty one (so a pre-existing empty
directory is never removed, and neither is the worktree root). If any step before the deletion fails
(e.g. the backup ref cannot be created), nothing is deleted. A file that cannot be deleted after the
backup (read-only directory) stays and goes into `warnings` with the backup ref; if none could be
deleted, error with the backup ref. The repo may have no commits and HEAD may be detached.

**Residual race.** Between the last verification of a file (and of the directories on its path)
and its `remove_file` there is an instant that cannot be closed: std has no `unlinkat` relative to
a directory handle held open since the check, so the deletion goes by path. A file rewritten by
another process (an editor saving) in that instant loses the new bytes (the backup has the ones
verified just before); a directory swapped for a link in that instant would make the removal
follow it. The window is the time of one `lstat` + `remove_file` per file.

Result: `{"deleted": [path], "files": [{"path", "mode", "size"}], "dirs": [path], "backup",
"commit", "bytes", "warnings", "status", "status_error"?, "can_undo", "can_redo", "next_undo",
"next_redo"}` (`dirs` deepest first, `bytes` the total size of the deleted files).

The journal entry has `"clean": {"commit", "files": [{"path", "mode", "size"}]}` (only the files
actually deleted) and `"backup"`. Undo and redo do not look at HEAD (untracked files do not depend
on it) but are refused during an operation in progress, like the others.

- **undo**: every path must be absent (`lstat` gives `NotFound`; any other error is a refusal)
  and every existing directory on its way a real directory (not a link, not a file), or already
  hold exactly the backup's entry (kind, mode, bytes: what an earlier undo that stopped midway
  wrote, so the undo can be retried); otherwise `UndoConflict` with one line per file and nothing
  changes. Then each file not yet restored is recreated from the backup tree: missing directories
  created one level at a time, each checked right after to be a real directory (never written
  through a link), the file opened with `create_new` (never overwriting, even a file that
  appeared in the meantime), the execute bit set for
  `100755`, links recreated as links. Re-read afterwards: a file not as in the backup goes in
  `warnings`.
- **redo**: every file must still be untracked in the status and have the backup's kind, mode and
  bytes; otherwise `UndoConflict` and nothing changes. Then deleted again as above (no new
  backup), with the empty directories.

The `MemoryBackend` records backups in `MemoryBackend::clean_backups`
(`refs/project/backup/clean-<n>`, with the content of each file; mode always `100644`).
