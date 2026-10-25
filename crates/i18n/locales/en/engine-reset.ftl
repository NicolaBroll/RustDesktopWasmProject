engine-reset-target-empty = `target` is empty: give the commit to reset to (an id, a branch, a tag)
engine-reset-unborn = the branch `{ $name }` has no commits yet: there is nothing to reset
engine-reset-expected-hex = `expected` must be the full id ({ $len } hexadecimal digits) of the commit HEAD was on: `{ $value }`
engine-reset-head-moved = HEAD moved since the confirmation (it was on { $seen }, now it is on { $now }): nothing changed, look at the history again and confirm again
engine-reset-confirm = a hard reset to { $after } would overwrite the local changes of { $n ->
    [one] 1 file
   *[other] { $n } files
} ({ $files }). A backup would first be saved in `refs/project/backup/reset-...` (`git stash apply` puts them back); untracked files are not touched
engine-reset-published = { $n ->
    [one] 1 commit that leaves
   *[other] { $n } commits that leave
} `{ $branch }` { $n ->
    [one] is
   *[other] are
} already on `{ $upstream }`: the next push will have to overwrite the remote branch (push with lease)
engine-reset-bad-request = `{ $command }`: invalid reset request (HEAD and the branch to move do not match)
engine-reset-intent-to-add = there are `intent-to-add` files (`git add -N`: { $files }): a hard reset would delete them and the backup (`git stash create`) cannot save them. Nothing changed: stage them (`index.stage`) or unstage them (`index.unstage`) and try again
engine-reset-files-changed = the local changes changed since the confirmation (confirmed: { $confirmed }; now: { $now }): nothing changed, reload the status and confirm again
engine-reset-undo-staged = `{ $path }`: the index changed after the reset (staged changes or a conflict)
engine-reset-undo-changed = `{ $path }`: the file has local changes
engine-reset-moved = HEAD or the branch changed while the reset was being prepared ({ $changes }): nothing changed, reload and try again
engine-reset-in-the-way = untracked or ignored files (or folders) are where the target commit puts files: { $files }. A hard reset would overwrite or delete them and the backup does not contain them. Nothing changed: move them and try again
engine-reset-not-commit = `{ $target }` is not a commit: nothing changed
engine-reset-head-changed-after = after moving the ref HEAD is { $now } instead of { $target } (another process changed it at that instant): index and worktree were not touched by `{ $command }`
engine-reset-index-copy = cannot copy the index `{ $path }` for the backup: { $error }
engine-reset-backup-incomplete = the worktree backup (`{ $backup }`) does not have the current content of { $files } (a file that changed just now?): nothing changed, try again
engine-reset-note-backups = the backups stay in { $backups }
engine-reset-note-moved = the branch has already moved (it was on { $before }), index and worktree may not match it; backups: { $backups }
engine-reset-undo-backup = the local changes overwritten by the hard reset are not put back: they stay in `{ $backup }` (`git stash apply { $stash }`)
engine-reset-hidden = files marked `--assume-unchanged` or `--skip-worktree` differ between HEAD and the target ({ $files }): git does not see their local changes and a hard reset could overwrite them. Nothing changed: clear the flag (`git update-index --no-assume-unchanged` / `--no-skip-worktree`) and try again
