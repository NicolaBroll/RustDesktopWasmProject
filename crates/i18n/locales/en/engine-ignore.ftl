engine-ignore-no-patterns = no patterns: nothing to ignore
engine-ignore-pattern-empty = an empty pattern
engine-ignore-pattern-blank = a pattern of only spaces
engine-ignore-pattern-line-break = the pattern `{ $pattern }` has a line break or a NUL byte: one pattern per line
engine-ignore-pattern-comment = the pattern `{ $pattern }` starts with `#`, which git reads as a comment: escape it as `\#`
engine-ignore-conflicted = `.gitignore` has a conflict: resolve it first
engine-ignore-not-checked-out = `.gitignore` is tracked but not in the worktree (sparse checkout or `--skip-worktree`): nothing written, restore it first
engine-ignore-not-a-file = `{ $file }` is not a regular file (a link, a folder or a special file): nothing written, edit it outside the app
engine-ignore-dir-not-real = the folder of `{ $file }` is not a real folder (a link or a file): nothing written
engine-ignore-unreadable = `{ $file }` cannot be read: { $error }
engine-ignore-changed = `{ $file }` changed while the patterns were being added: nothing written, try again
engine-ignore-backup-incomplete = the backup (`{ $backup }`) does not have the bytes of `{ $file }`: nothing written, try again
engine-ignore-write-failed = `{ $file }` could not be written ({ $error }): the file is as before; the backup is in `{ $backup }`
engine-ignore-not-as-expected = after `{ $command }` `{ $file }` does not have the bytes written (another program changed it?); the bytes are in `{ $backup }`
engine-ignore-file-changed-undo = `{ $file }` changed since the operation: nothing restored
engine-ignore-tracked = { $n ->
    [one] 1 tracked file matches the new patterns
   *[other] { $n } tracked files match the new patterns
}: git keeps tracking { $n ->
    [one] it
   *[other] them
} (`git rm --cached` would stop it): { $files }
