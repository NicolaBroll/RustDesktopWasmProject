# Ignoring files

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the
`CliBackend` and undo and backup of this function. "§n" always refers to the main document.

## Commands

### `worktree.ignore`

Parameters: `{"patterns": [string], "file"?: "gitignore" | "exclude"}` (`Access::Write`, no
`confirm`). Appends the patterns to the root `.gitignore` (default) or to `.git/info/exclude`
(ignored only in this clone, never committed). A pattern is a string, or a byte array when it is
not UTF-8 (as a `RepoPath`).

Result: `{"file": "gitignore" | "exclude", "label": ".gitignore" | ".git/info/exclude", "added":
[pattern], "already": [pattern], "ignored": [path], "tracked": [path], "created": bool, "backup":
string | null, "warnings": [Message], "status", "status_error"?, "can_undo", "can_redo",
"next_undo", "next_redo"}`:

- `added`: the patterns appended, in the given order; `already`: those already present as a line
  (nothing written for them). If every pattern is already there nothing is written: no backup
  (`null`), no journal entry.
- `ignored`: the untracked files that became ignored (they leave the status); `tracked`: the
  tracked files the new patterns match. Git keeps tracking them, so `warnings` has
  `engine-ignore-tracked` (`git rm --cached` would stop it).
- `created`: the file did not exist.

Errors: `InvalidParams` for no patterns, an empty pattern, one with NUL/CR/LF, only spaces/tabs,
or starting with `#` (git would read a comment: escape it as `\#`), an unknown `file`, unknown
fields. `NotAllowed` for a `.gitignore` with a conflict, a `.gitignore` tracked but missing from the
worktree (sparse checkout, `--skip-worktree`: git reads it from the index, and a new file with only
the new patterns would replace all its rules), a file that is a link, a folder or a
special file, a folder (`info/`, the root) that is a link, a file changed while the patterns were
being added, an incomplete backup. Write failure: `GitWithReason` naming the backup; the file is
as before.

### Building the pattern (UI)

`project_domain::ignore_pattern(path, kind)` builds the pattern for the UI choices, relative to
the root (valid for both files): `IgnoreKind::Path` → `/dir/name` (exactly that path),
`IgnoreKind::Folder` → `/dir/` (the folder and everything under it), `IgnoreKind::Extension` →
`*.ext` (the extension of the file name, anywhere; `None` without one, or for names like `.env`).
The bytes `\ * ? [ ! #` and trailing spaces are escaped with `\`. Integration tests check with
`git check-ignore --no-index` that each pattern matches exactly its path (spaces, `#`, `!`, `*`,
`?`, trailing space, a name starting with `-`, non-UTF-8 names) and not lookalikes.

## Steps and guarantees

Code: `crates/domain/src/git/ignore.rs` (patterns, append), `crates/engine/src/git/ignore.rs`
(types, checks), `crates/engine/src/builtin/ignore.rs` (command),
`crates/git/src/backend/ignore.rs` and `atomic.rs` (`CliBackend`). Choices:

- **Only the two root files.** Patterns are relative to the root; a `.gitignore` in a subfolder
  would need patterns relative to it.
- **Append only** (`append_ignore_patterns`): a pattern already present as a line (compared
  without the final `\r`, and a UTF-8 BOM on the first line) is not added again, unless a negation
  (`!...`) follows it (it may re-include what the pattern matches, and a later line wins), nor is a
  repetition in the request. The line ending
  is the one of the file's first line (`\r\n` if it ends so, `\n` otherwise or for a new file);
  a missing final newline is added first. Nothing else in the file changes.
- **No `confirm`**: nothing is lost, and the old bytes stay reachable (backup below).
- **Atomic write**: an existing file is replaced by a temporary file in the same folder
  (`.project-<pid>-<nanos>.tmp`: without the file's name, so never past the name limit) with the
  old file's permissions, synced, then renamed over it; a new file is linked in place
  (`hard_link`: never overwrites; on a file system without hard links, `create_new`). The
  replaced file is a new inode: owner, ACLs and extended attributes are not copied, and a hard link
  to the old file keeps the old content.
- **Links refused**: `.gitignore` (or `info/exclude`) that is a link, a folder or a special file,
  or a folder of it that is a link → `NotAllowed` (git itself does not follow a `.gitignore` link).
  `.git/info/` is created if missing.
- **Index untouched.** A new `.gitignore` shows up as an untracked file, a changed one as an
  unstaged change.

## CliBackend

In this order, and nothing is written if a step fails:

1. `.gitignore`: `ls-files -z --unmerged -- .gitignore` must be empty (no conflict); if the
   file is missing, `ls-files -z --cached -- .gitignore` must be empty too. The file is
   `<root>/.gitignore` or `git rev-parse --git-path info/exclude` (linked worktrees use the common
   directory, `GIT_DIR`/`GIT_COMMON_DIR` are followed).
2. `lstat` of its folder (a real folder; `info/` may be missing) and of the file (missing or a
   regular file), then its bytes.
3. `append_ignore_patterns`; if nothing is added, stop here.
4. The untracked files now: `ls-files -z --others --exclude-standard` (reads `.gitignore` files,
   `info/exclude`, `core.excludesFile`). The tracked files the new patterns alone match:
   `ls-files -z --cached --ignored --exclude=<pattern>...` (one argument per pattern, so a
   pattern never becomes an option; command-line patterns are root-relative as in
   `info/exclude`).
5. Backup: blobs of the bytes before and after (`hash-object -w --stdin --no-filters`), a commit
   with only the bytes written and, as its child, a commit with the bytes before (empty tree if
   the file did not exist), both with trees built in a temporary index, fixed identity,
   `--no-gpg-sign`; `update-ref refs/project/backup/ignore-<seconds>-<nanos> <commit> ""`; then
   `ls-tree` must show both blobs. In the commits the path is `.gitignore` or `info/exclude`.
6. **Last check**: the file still has the bytes read in step 2 (or is still missing); otherwise
   `NotAllowed` (the backup ref stays, harmless).
7. Atomic write (above).
8. The file reread: if it does not have the bytes written, a warning naming the backup. The
   untracked files again: those that left the list are `ignored` (an error here is a warning: the
   file is already written).

**Residual race.** Between the last check and the rename another program can write the file; its
bytes would be replaced (the window is one `lstat` + read + rename). The undo then refuses,
because the file does not have the bytes written by it.

## Undo and backup

`worktree.ignore` enters the journal (`JournalEntry::ignore`, field `"ignore": {"file", "before":
oid | null, "after": oid}`) when it writes. It moves no refs and does not depend on HEAD; undo and
redo are refused while an operation (merge, rebase...) is in progress.

- `edit.undo`: the file must have exactly the bytes of `after` (otherwise `UndoConflict`, nothing
  changes); then the bytes of `before` are written back atomically, or the file is removed if it
  was created (`info/` stays).
- `edit.redo`: the file must have exactly the bytes of `before` (or be missing); then the bytes
  of `after` are written again.
- Both blobs stay reachable from the backup ref (child commit: before; parent: after), so undo
  and redo work after `git gc --prune=now`.

`MemoryBackend`: `.gitignore` is the worktree file (or the index version of a tracked one),
`info/exclude` a field; the status hides untracked files matched by a simplified matcher (`*`,
`**`, `?`, `\`, `!`, anchoring, trailing `/`; no `[...]`, no subfolder `.gitignore`). Test
helpers: `ignore_file`, `set_ignore_file`.
