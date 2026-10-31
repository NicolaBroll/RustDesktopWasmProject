engine-clean-no-paths = no paths: nothing to delete
engine-clean-nested-repo = nested git repositories, nothing deleted: their history is not in the backup; delete them outside the app or name the files to delete: { $files }
engine-clean-not-untracked = { $n ->
    [one] not an untracked file
   *[other] not untracked files
} (tracked, ignored, conflicted or missing), nothing deleted: { $files }
engine-clean-plan-changed = the untracked files to delete changed since the confirmation (confirmed: { $confirmed }; now: { $now }): nothing deleted, reload the status and confirm again
engine-clean-confirm = { $n ->
    [one] 1 untracked file
   *[other] { $n } untracked files
} would be deleted ({ $files }; { $bytes } bytes in total). A backup with their exact bytes would first be saved in `refs/project/backup/clean-...`, and `edit.undo` would put them back
engine-clean-recreated = `{ $path }`: something is at that path again (or in place of one of its folders): nothing restored, move it away and try again
engine-clean-changed-redo = `{ $path }`: the file is no longer the restored one (changed, staged or gone)
engine-clean-not-a-file = no longer an untracked file or link (a folder, a special file, or a link in place of one of its folders), nothing deleted: { $files }
engine-clean-changed-after-backup = changed after the backup
engine-clean-not-in-backup = not in the backup
engine-clean-unreadable = `{ $path }` cannot be read: { $error }
engine-clean-backup-incomplete = the backup (`{ $backup }`) does not have the current bytes of { $files } (a file that changed just now?): nothing deleted, try again
engine-clean-not-deleted = could not delete { $files } ({ $errors }): { $n ->
    [one] it stays
   *[other] they stay
}, the backup is in `{ $backup }`
engine-clean-nothing-deleted = nothing deleted ({ $errors }); the backup stays in `{ $backup }`
engine-clean-restore-failed = `{ $path }` cannot be recreated ({ $error }); the backup stays in commit { $commit }
engine-clean-not-as-expected = after `{ $command }` { $files } are not as in the backup; it stays in commit { $commit }
