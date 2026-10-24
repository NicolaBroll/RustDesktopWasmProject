# Merge, cherry-pick, revert and the operation-in-progress panel (merge.rs, pick.rs, merge_view.rs).
# $name: merge, cherry-pick, revert, rebase (lowercase); $title: the same, capitalized.

merge-noun-merge-commit = merge commit
merge-noun-commit = commit
merge-terminal-merge = Finish it from the terminal: `git merge --continue` or `git merge --abort`
merge-terminal-rebase = Finish it from the terminal: `git rebase --continue` or `git rebase --abort`
merge-terminal-cherry-pick = Finish it from the terminal: `git cherry-pick --continue` or `git cherry-pick --abort`
merge-terminal-revert = Finish it from the terminal: `git revert --continue` or `git revert --abort`
merge-terminal-bisect = Finish it from the terminal: `git bisect reset` when the search is done
merge-terminal-am = Finish it from the terminal: `git am --continue` or `git am --abort`
merge-incoming = “{ $label }”
merge-incoming-summary = “{ $label }” ({ $summary })
merge-title = { $name } in progress
merge-title-rebase-branch = { $name } of “{ $branch }” in progress
merge-title-rebase-onto = { $name } in progress onto { $onto }
merge-title-rebase-branch-onto = { $name } of “{ $branch }” in progress onto { $onto }
merge-title-merge-incoming = { $name } in progress: merging { $incoming }
merge-title-incoming = { $name } in progress: { $incoming }
merge-gate-unborn = The current branch has no commits yet
merge-gate-detached = HEAD is detached: a current branch is needed. Switch to a branch first
merge-gate-operation = An operation is already in progress: finish or abort it from the panel at the top
merge-gate-conflicts = There are conflicted files: resolve them first
merge-unexpected-result = { $command }: unexpected result: { $error }
merge-unexpected-response = unexpected response from `{ $command }`
merge-start-label = Merge of “{ $what }” into “{ $into }”
merge-pick-label = { $title } of “{ $what }” onto “{ $into }”
merge-continue-label = Finishing the { $name }
merge-abort-check-label = Checking before aborting the { $name }
merge-abort-label = Aborting the { $name }
merge-refused-title = { $title } not possible
merge-failed-title = { $title } failed
merge-refused = { $what } not possible: { $error }
merge-failed = { $what } failed: { $error }. The repository is reloaded to show its current state
merge-done = { $title } done
merge-concluded = { $title } finished
merge-concluded-but = { $title } finished, but: { $error }
merge-concluded-notice = { $title } finished: { $noun } { $id } created. “Undo” undoes it
merge-concluded-notice-branch = { $title } finished: { $noun } { $id } created on “{ $branch }”. “Undo” undoes it
merge-abort-refused = Cannot abort the { $name }: { $error }
merge-abort-failed = Aborting the { $name } failed: { $error }
merge-aborted = { $title } aborted
merge-aborted-notice = { $title } aborted: index and working tree saved first in { $backup }{ $n ->
        [0] {""}
        [one] {" "}(1 file)
       *[other] {" "}({ $n } files)
    }; restore them with `git stash apply { $backup }`. It cannot be undone with “Undo”
merge-warning = warning: { $warning }
merge-status-not-reloaded = the status could not be reloaded: { $error }
merge-refs-not-reloaded = the refs could not be reloaded: { $error }
merge-done-but = { $what }, but { $problems }
merge-commits = { $n ->
    [one] 1 commit
   *[other] { $n } commits
}
merge-conflict-files = { $n ->
    [one] 1 conflicted file
   *[other] { $n } conflicted files
}
merge-stopped-before-commit = Merge of “{ $name }” into “{ $branch }” stopped before the commit (a hook?): finish it with “Continue merge…” in the panel at the top, or abort it
merge-stopped = Merge of “{ $name }” into “{ $branch }” stopped: { $files }. Resolve them and mark them as resolved in the staging area, then “Continue merge…” in the panel at the top (or “Abort merge…”)
merge-up-to-date = “{ $branch }” already contains “{ $name }”: nothing to merge
merge-fast-forward = Fast-forward: “{ $branch }” moved forward by { $commits } to “{ $name }” ({ $id }). “Undo” moves it back
merge-rebase-stopped = Rebase stopped again on the next commit: { $files }. Resolve them and mark them as resolved in the staging area, then “Continue rebase…” in the panel at the top (or “Abort rebase…”)
merge-rebase-stopped-before-commit = Rebase stopped again before a commit (a hook or signing?): finish it with “Continue rebase…” in the panel at the top, or abort it
merge-rebase-stopped-short = Rebase stopped again
merge-rebase-finished = Rebase finished: HEAD at { $id }
merge-rebase-finished-branch = Rebase of “{ $branch }” finished: the branch is at { $id }
merge-rebase-backup = ; the branch as it was is in { $backup }
merge-merged = “{ $name }” merged into “{ $branch }” ({ $commits }): merge commit { $id }. “Undo” undoes it
merge-pick-stopped-before-commit = { $title } of “{ $what }” onto “{ $branch }” stopped before the commit (a hook or signing?): finish it with “Continue { $name }…” in the panel at the top, or abort it
merge-pick-stopped = { $title } of “{ $what }” onto “{ $branch }” stopped: { $files }. Resolve them and mark them as resolved in the staging area, then “Continue { $name }…” in the panel at the top (or “Abort { $name }…”)
merge-cherry-picked = “{ $what }” applied onto “{ $branch }”: commit { $id }. “Undo” undoes it
merge-reverted = Changes of “{ $what }” reverted on “{ $branch }” with commit { $id }. “Undo” removes it

# Operation-in-progress panel.
merge-info-error = Could not read the operation details: { $error }
merge-onto = Onto { $onto }
merge-sequence-hint = It is a sequence of several commits started from the terminal: finish it there (a commit from here would leave the sequence half done, aborting would also remove the commits already applied)
merge-not-supported-hint = The app cannot finish or abort this operation: do it from the terminal
merge-abort-button = Abort { $name }…
merge-abort-hint = Goes back to how things were before the { $name }: asks for confirmation and explains what is lost. First saves the index and working tree in a backup ({ $backup })
merge-wait = Wait for the current operation (or close the dialog)
merge-continue-button = Continue { $name }…
merge-continue-hint = Creates the { $noun }: the message can be edited
merge-continue-hint-rebase = Goes on with the remaining commits of the rebase: it may stop again on the next conflict
merge-continue-conflicts = Conflicted files remain: resolve them and mark them as resolved (stage) in the changes area
merge-no-conflicts-continue = No conflicted files: finish with “Continue { $name }…”
merge-no-conflicts = No conflicted files
merge-conflicts-label = { $n ->
    [one] 1 conflicted file:
   *[other] { $n } conflicted files:
}
merge-more-files = and { $n } more…
merge-more-files-hint = Opens the staging area, which lists them all
merge-click-to-resolve = — click one to open the editor and resolve it
merge-file-hint = Opens the conflict editor for the file

# Dialogs: Continue, abort, parent of a merge commit.
merge-finish-heading = Finish the { $name }
merge-into-branch = on “{ $branch }”
merge-into-detached = on detached HEAD ({ $id })
merge-finish-text = Creates the { $noun } { $into } with what is staged. “Undo” in the top bar undoes it, as long as the app stays open.
merge-message-label = Message of the { $noun }
merge-create-button = Create { $noun }
merge-committing-hint = Commit in progress
merge-write-message = Write the message
merge-close = Close
merge-rebase-continue-text = Git goes on with the remaining commits of the rebase: the commit it stopped on is created with what is staged and keeps its message. It may stop again on the next conflict.
merge-rebase-continue-text-branch = Git goes on with the remaining commits of the rebase of “{ $branch }”: the commit it stopped on is created with what is staged and keeps its message. It may stop again on the next conflict; when it finishes, “Undo” in the top bar puts the branch back.
merge-rebase-continue-button = Continue { $name }
merge-continuing = Continuing…
merge-committing = Committing…
merge-abort-heading = Abort the { $name }?
merge-abort-text-rebase = The rebase in progress is aborted: the branch, the index and the files go back to how they were before the rebase; the commits rewritten so far are discarded (the backup keeps them reachable).
merge-abort-text = The { $name } in progress is aborted: the index and files go back to how they were before the { $name }, the branch does not move.
merge-what-happens = What happens:
merge-abort-backup = “Undo” in the top bar does not restore it: to get the resolutions back, the backup { $backup } remains (git stash apply <backup>).
merge-abort-confirm = Abort { $name }
merge-dont-abort = Don't abort
merge-mainline-heading = { $title } of a merge commit
merge-mainline-text-cherry-pick = “{ $what }” has { $n } parents. The changes the merge brought relative to the chosen parent are applied onto “{ $into }”: usually the first one (the branch it was merged into).
merge-mainline-text-revert = “{ $what }” has { $n } parents. The changes the merge brought relative to the chosen parent are reverted on “{ $into }”: usually the first one (the branch it was merged into).
merge-mainline-hint = With parent { $n } (mainline)
