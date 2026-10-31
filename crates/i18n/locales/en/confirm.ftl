# Confirmations before writing: conflicts to mark resolved, discard, amend (view/confirm.rs).

confirm-resolve-title = { $n ->
    [one] Mark the conflict as resolved?
   *[other] Mark the conflicts as resolved?
}
confirm-resolve-body = Staging a conflicted file marks it as resolved: the index gets the current content of the file in the working tree, as it is, including any conflict markers (<<<<<<<, =======, >>>>>>>) left in it. The conflict versions (base, ours and theirs) leave the index. Check first that the file is really resolved.
confirm-resolve-files = { $n ->
    [one] Conflicted file:
   *[other] { $n } conflicted files:
}
confirm-and-more = and { $n } more
confirm-resolve-others = { $n ->
    [one] Along with 1 other file without conflicts, staged normally.
   *[other] Along with { $n } other files without conflicts, staged normally.
}
confirm-resolve-button = { $n ->
    [one] Mark as resolved
   *[other] Mark all as resolved
}
confirm-cancel = Cancel
confirm-discard-title = Discard changes?
confirm-discard-body = Unstaged changes are removed: each file goes back to its staged version, or to the last commit if nothing of it is staged. Staged changes and untracked files are not touched.
confirm-discard-backup = “Undo” in the top bar brings the files back as they are now, as long as they are not modified again.
confirm-discard-details = Details
confirm-discard-details-body = Before discarding, a backup is saved in refs/project/backup/discard-…; then each file is restored from the git index (the staging area).
confirm-discard-files = { $n ->
    [one] File:
   *[other] { $n } files:
}
confirm-discard-button = { $n ->
    [one] Discard changes
   *[other] Discard all changes
}
confirm-discard-keep = Don't discard
confirm-discard-lines-title = Discard { $n ->
    [one] 1 changed line
   *[other] { $n } changed lines
} of { $path }?
confirm-discard-hunk-title = Discard hunk { $hunk } of { $path }?
confirm-discard-lines-body = The chosen lines go back to their staged version (or to the last commit if nothing of the file is staged); the other changes of the file stay. Staged changes are not touched.
confirm-discard-lines-backup = A backup of the file as it is now is saved in refs/project/backup/discard-…: “Undo” in the top bar puts it back, as long as the file is not modified again.
confirm-discard-lines-button = { $n ->
    [one] Discard 1 line
   *[other] Discard { $n } lines
}
confirm-discard-hunk-button = Discard hunk
confirm-amend-title = Rewrite a commit that is already published?
confirm-amend-body = The last commit is already published or tagged: a remote branch or a tag contains it. Amend replaces it with a new commit: remote branches and tags stay on the old commit, the local branch diverges from the remote one, and publishing the change will need a force push, which rewrites history also for anyone who has already fetched the commit. The current commit stays saved in a backup ref.
confirm-amend-reason = Reason:
confirm-amend-left-behind = Also left on the old commit:
confirm-amend-button = Rewrite the commit
