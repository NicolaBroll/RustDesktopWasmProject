# Discarding unstaged changes (discard.rs).

discard-reason-conflict = Conflicted file: resolve it first, it can't be discarded from here
discard-reason-untracked = Untracked file: it isn't discarded (that would mean deleting it, with no backup)
discard-label-one = Discarding changes to { $path }
discard-label-many = Discarding changes to { $n ->
    [one] 1 file
   *[other] { $n } files
}
discard-check-label-one = Checking before discarding changes to { $path }
discard-check-label-many = Checking before discarding changes to { $n ->
    [one] 1 file
   *[other] { $n } files
}
discard-not-allowed = { $label } not possible: { $error }
discard-done-one = Changes to { $path } discarded; backup in { $backup }. “Undo” brings them back
discard-done-many = Changes to { $n ->
    [one] 1 file
   *[other] { $n } files
} discarded; backup in { $backup }. “Undo” brings them back
discard-status-not-reread = Changes discarded, but the status can't be reread: { $reason }
