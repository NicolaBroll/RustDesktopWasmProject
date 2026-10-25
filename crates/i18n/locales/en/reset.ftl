reset-menu-item = Reset “{ $branch }” to here…
reset-menu-item-detached = Reset HEAD to here…
reset-menu-hint = Move the current branch (or the detached HEAD) to { $id }: choose soft, mixed or hard and see what changes before confirming
reset-menu-unborn = The branch “{ $branch }” has no commits yet: there is nothing to reset
reset-menu-operation = An operation is in progress (merge, rebase, cherry-pick, revert or bisect): finish or abort it first
reset-branch-name = “{ $branch }”
reset-detached-name = HEAD (detached)
reset-title = Reset { $moving } to { $what }
reset-mode-soft-hint = Moves only the branch: the changes of the commits that leave it stay staged, the files are not touched.
reset-mode-mixed-hint = Moves the branch and the stage: those changes stay in the files, unstaged. Nothing in the files is lost.
reset-mode-hard-hint = Moves the branch, the stage and the tracked files: the files become those of the commit. Local changes are overwritten (a backup is saved first).
reset-loading = Checking what this reset would do…
reset-not-possible = This reset is not possible: { $error }
reset-plan-move = { $moving } moves from { $before } to { $after }.
reset-plan-same = { $moving } is already on { $id }: only the stage and the files are reset.
reset-hard-clean = There are no local changes to tracked files: nothing is overwritten.
reset-hard-files = { $n ->
    [one] The local changes of 1 file are overwritten:
   *[other] The local changes of { $n } files are overwritten:
}
reset-hard-backup = They are first saved in a backup (refs/project/backup/reset-...): “git stash apply” puts them back. Undo moves the branch back but does not restore them.
reset-hard-untracked = Untracked files are not touched.
reset-warning = Warning: { $warning }
reset-button = Reset ({ $mode })
reset-cancel = Cancel
reset-sending = Resetting…
reset-label = Reset ({ $mode }) of { $moving } to { $id }
reset-refused = The reset was refused, nothing changed: { $error }
reset-failed = { $label } failed: { $error }
reset-done = { $moving } reset ({ $mode }) to { $id }
reset-done-backup = ; the local changes of { $n ->
    [one] 1 file are
   *[other] { $n } files are
} saved in { $backup } (git stash apply { $stash })
reset-done-index-backup = ; the previous stage is saved in { $backup }
reset-done-commit-backups = ; commits no longer reachable saved in { $refs }
reset-done-short = Reset done
reset-done-warning = ; warning: { $warning }
