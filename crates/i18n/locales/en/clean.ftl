# Deleting untracked files (clean.rs and its confirmation dialog).

clean-label-one = Deleting { $path }
clean-label-many = Deleting { $n ->
    [one] 1 untracked file
   *[other] { $n } untracked files
}
clean-check-label-one = Checking before deleting { $path }
clean-check-label-many = Checking before deleting { $n ->
    [one] 1 untracked file
   *[other] { $n } untracked files
}
clean-not-allowed = { $label } not possible: { $error }
clean-done-one = { $path } deleted; backup in { $backup }. “Undo” puts it back
clean-done-many = { $n ->
    [one] 1 untracked file
   *[other] { $n } untracked files
} deleted; backup in { $backup }. “Undo” puts them back
clean-status-not-reread = Files deleted, but the status can't be reread: { $reason }
clean-size-bytes = { $n ->
    [one] 1 byte
   *[other] { $n } bytes
}
clean-size-kb = { $n } KB
clean-size-mb = { $n } MB
clean-size-gb = { $n } GB
clean-confirm-title = { $n ->
    [one] Delete the untracked file?
   *[other] Delete { $n } untracked files?
}
clean-confirm-body = { $n ->
    [one] The file is not in git: it is removed from the disk, and folders left empty are removed too.
   *[other] The files are not in git: they are removed from the disk, and folders left empty are removed too.
}
clean-confirm-backup = A backup with their exact content is kept in refs/project/backup/clean-…: “Undo” in the top bar puts them back, as long as nothing has been created at the same path.
clean-confirm-files = { $n ->
    [one] 1 file:
   *[other] { $n } files:
}
clean-confirm-files-size = { $n ->
    [one] 1 file, { $size }:
   *[other] { $n } files, { $size } in total:
}
clean-confirm-button = { $n ->
    [one] Delete 1 file
   *[other] Delete { $n } files
}
