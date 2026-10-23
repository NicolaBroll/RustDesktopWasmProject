# Commit message and commit.create response (commit.rs), commit menu (view/commits.rs).

commit-blocked-busy = Wait for the current operation to finish
commit-blocked-conflicts = There are conflicted files: resolve them and stage them before committing
commit-blocked-merge = Merge in progress: finish it with “Continue merge…” in the panel at the top (or abort it with “Abort merge…”)
commit-blocked-rebase = Rebase in progress: it is finished with the rebase commands, not with a commit
commit-blocked-cherry-pick = Cherry-pick in progress: it is finished with the cherry-pick commands, not with a commit
commit-blocked-revert = Revert in progress: it is finished with the revert commands, not with a commit
commit-blocked-am = `git am` in progress: it is finished with `git am --continue`, not with a commit
commit-blocked-operation = An operation in progress prevents committing
commit-blocked-no-commit = There is no commit to amend yet
commit-blocked-nothing-staged = Nothing staged: stage the changes to commit (or turn on amend to change only the message of the last commit)
commit-blocked-no-message = Write the commit message
commit-amend-label = Amending the last commit
commit-label = Commit
commit-unchanged = No change: the last commit { $id } stayed the same
commit-created = Commit { $id } created
commit-created-backup = Commit { $id } created; backup of the previous commit in { $backup }
commit-amended = Commit { $id } created in place of { $previous }; backup of the previous commit in { $backup }
commit-also-on = ; the previous commit is still on { $refs }
commit-git-failed = git commit failed: a hook (pre-commit, commit-msg…), signing or the identity (user.name, user.email) stopped the commit. The message you wrote is kept. Git output:
commit-no-output = (no output)
commit-failed-kept = Commit failed: { $error } The message you wrote is kept.
commit-unexpected-result = { $command }: unexpected result: { $error }
commit-failed = Commit failed: { $error }

# Commit list and commit menu (view/commits.rs).
commit-list-loading = Loading commits…
commit-list-empty = No commits.
commit-list-seeking = Looking for commit { $id } among older commits…
commit-list-loading-more = Loading more commits…
commit-new-branch = New branch from here…
commit-new-branch-hint = Creates a branch at { $id } without switching to it
commit-list-changes = Uncommitted changes
commit-list-changes-count = { $n ->
        [one] 1 file
       *[other] { $n } files
    }
commit-list-changes-staged = { $n ->
        [one] 1 file
       *[other] { $n } files
    }, { $staged } staged
commit-list-changes-hint = Click: show the staging area
commit-current-branch = current branch
commit-cherry-pick-item = Cherry-pick onto “{ $into }”
commit-cherry-pick-merge-item = Cherry-pick onto “{ $into }”…
commit-revert-item = Revert
commit-revert-merge-item = Revert…
commit-cherry-pick-hint = Applies the changes of “{ $what }” on top of “{ $into }” with a new commit (original author and message). The files must have no changes; with conflicts it stays in progress (resolve or abort it from the panel at the top). “Undo” removes it
commit-cherry-pick-merge-hint = Applies the changes of “{ $what }” on top of “{ $into }” with a new commit (original author and message). It is a merge commit: first choose the reference parent. The files must have no changes; with conflicts it stays in progress (resolve or abort it from the panel at the top). “Undo” removes it
commit-revert-hint = Creates a commit on “{ $into }” that reverts the changes of “{ $what }”. The files must have no changes; with conflicts it stays in progress (resolve or abort it from the panel at the top). “Undo” removes it
commit-revert-merge-hint = Creates a commit on “{ $into }” that reverts the changes of “{ $what }”. It is a merge commit: first choose the reference parent. The files must have no changes; with conflicts it stays in progress (resolve or abort it from the panel at the top). “Undo” removes it
