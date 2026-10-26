# Conflict editor (conflict.rs, view/conflict_view.rs).
# $command: command id; $path: file path; $backup: backup ref or path.

conflict-worktree-not-text = the working tree file is not text or does not exist
conflict-no-blocks = the result has no blocks to choose
conflict-label-use-ours = Resolving { $path } with our version
conflict-label-use-theirs = Resolving { $path } with their version
conflict-label-delete = Deleting { $path } to resolve the conflict
conflict-label-resolve = Resolving the conflict in { $path }
conflict-resolved-path = Conflict in { $path } resolved and staged
conflict-resolved = Conflict resolved and staged
conflict-resolved-backup = ; the previous file is saved in { $backup }
conflict-resolved-index-backup = ; index backup in { $backup }
conflict-stale = The file changed after it was read: reloaded, make your choices again
conflict-resolve-failed = Resolution failed: { $error }
conflict-title = Conflict
conflict-close = Close
conflict-close-hint = Back to the list: choices not marked as resolved are lost
conflict-restart = Start over
conflict-restart-hint = Rereads the file from disk: the choices made here are lost
conflict-loading = Reading the versions…
conflict-load-failed = Versions not read: { $error }
conflict-retry = Retry
conflict-incoming = incoming
conflict-column-base = Base (common ancestor)
conflict-column-ours = Ours ({ $label })
conflict-column-theirs = Theirs ({ $label })
conflict-result = Result
conflict-blocks = { $n ->
    [0] no conflict blocks
    [one] 1 conflict block
   *[other] { $n } conflict blocks
}
conflict-all-theirs = All theirs
conflict-all-theirs-hint = Every block with their lines
conflict-all-ours = All ours
conflict-all-ours-hint = Every block with our lines
conflict-markers-unreadable = The markers can no longer be read ({ $error }): no per-block choices, the text stays editable. “Start over” rereads the file
conflict-missing = (missing)
conflict-binary = (binary, { $size } bytes)
conflict-block = Block { $n } (line { $line }):
conflict-choice-ours = Ours
conflict-choice-ours-hint = Keeps our lines
conflict-choice-theirs = Theirs
conflict-choice-theirs-hint = Keeps their lines
conflict-choice-both = Both
conflict-choice-both-hint = Our lines, then theirs
conflict-choice-base = Base
conflict-choice-base-hint = The lines of the common ancestor
conflict-sides-missing = The file is not in the working tree. It can only be resolved with a whole version (the current file is backed up first).
conflict-sides-not-text = The file is not text. It can only be resolved with a whole version (the current file is backed up first).
conflict-sides-unreadable = The conflict markers of the file cannot be read. It can only be resolved with a whole version (the current file is backed up first).
conflict-use-ours = Use ours
conflict-use-theirs = Use theirs
conflict-use-side-hint = Writes that version to the file and stages it
conflict-wait = Wait for the current operation
conflict-side-unavailable = That version is missing or is not text
conflict-confirm-markers = { $n ->
    [one] Still contains markers (1 block): resolve anyway? The file is staged as it is
   *[other] Still contains markers ({ $n } blocks): resolve anyway? The file is staged as it is
}
conflict-confirm-markers-button = Resolve anyway
conflict-confirm-delete = The file is deleted from the working tree and the deletion is staged. The current file is backed up first
conflict-confirm-delete-button = Delete the file
conflict-cancel = Cancel
conflict-mark-resolved = Mark as resolved
conflict-mark-resolved-hint = Writes the result to the file and stages it. The previous file is backed up first
conflict-wait-dialog = Wait for the current operation (or close the dialog)
conflict-delete = Delete the file…
conflict-delete-hint = Resolves the conflict by deleting the file: asks for confirmation
conflict-resolving = Resolving…
