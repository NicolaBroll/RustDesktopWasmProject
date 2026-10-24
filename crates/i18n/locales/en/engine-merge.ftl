engine-merge-other-in-progress = { $other } is in progress, not { $operation }: `{ $command }` does not touch it, nothing changed. Finish or abort it with its own commands (or from a terminal)
engine-merge-not-in-progress = { $operation } is not in progress: `{ $command }` has nothing to do, nothing changed
engine-merge-local-changes = the merge would create a merge commit but there are local changes to tracked files ({ $files }): commit or stash them (`stash.push`) and try again (no `--autostash`); nothing changed
engine-merge-conflicts = there are conflicted files ({ $files }): resolve them before the merge; nothing changed
engine-merge-abort-unborn = HEAD has no commit: nothing aborted
engine-merge-abort-mixed = git does not abort the operation in progress: staged files that also have unstaged changes ({ $files }). Stage them (`index.stage`) or discard them (`worktree.discard`) and try again; nothing changed, `{ $command }` aborted nothing
engine-merge-blocked = { $what } would overwrite local changes or untracked files ({ $files }): commit, stash or move them and try again; nothing changed
engine-merge-not-fast-forward = `{ $target_name }` does not contain `{ $branch }`: the fast-forward is not possible, nothing changed
engine-merge-overwrite = the merge would overwrite local changes or untracked files ({ $files }): commit, stash or move them and try again; nothing changed
engine-merge-head-moved = HEAD changed while the merge was being prepared: nothing changed, try again
engine-merge-what-merge = the merge
engine-merge-what-pull = the pull
engine-merge-abort-lose-conflicts = the conflicted files ({ $files }) would go back to HEAD, with the resolutions made so far
engine-merge-abort-lose-changes = the changes of the { $operation } and those made afterwards, staged or not, would be lost ({ $files })
engine-merge-abort-lose-operation = the { $operation } in progress would be aborted
engine-merge-continue-conflicts = conflicted files remain ({ $files }): resolve them and stage them (`index.stage`), then repeat `merge.continue`; nothing committed
engine-merge-abort-confirm = { $lost }. The index and worktree would first be saved in `refs/project/backup/{ $backup }-abort-...` (they are put back with `git stash apply <ref>`); the abort does not go into the journal
engine-merge-diverged-ff-only = `{ $source }` and `{ $branch }` have diverged ({ $ahead ->
    [one] 1 commit
   *[other] { $ahead } commits
} only in the branch, { $behind } only in `{ $source }`): with `ff_only` only a fast-forward is possible, nothing changed
engine-merge-detached = HEAD is detached: the merge must be done on a branch, nothing changed. Switch to a branch (`branch.checkout`) or create one here (`branch.create`) and try again
engine-merge-unborn = branch `{ $name }` has no commits yet: nothing to merge, nothing changed
engine-merge-rev-empty = `rev` must be a branch name, a tag name or an id, not empty
engine-merge-no-ff-and-ff-only = `no_ff` (always a merge commit) and `ff_only` (fast-forward only) exclude each other
engine-merge-branch-not-moved = `git merge` finished without errors but `{ $full }` did not move ({ $text }): check index and worktree; the branch as it was is in `{ $backup }`
engine-merge-branch-gone = after the merge, `{ $full }` no longer exists
engine-merge-message-not-written = merge message not written: { $error }
# `$hint`: `mixed` (staged file with unstaged changes) or `none`; `$state`: `still` or `maybe`.
engine-merge-abort-failed = `git { $git } --abort` failed ({ $stderr }).{ $hint ->
    [mixed] {" "}This happens when a staged file also has unstaged changes: stage them (`index.stage`) or discard them (`worktree.discard`) and try again.
   *[none] {""}
} The operation { $state ->
    [still] is still
   *[maybe] may no longer be
} in progress and some files may already have been rewritten: reload the status. The index and worktree from before are saved in `{ $backup }` (`git stash apply { $backup }`)
engine-merge-target-missing = after the merge `{ $full }` ({ $after }) does not contain `{ $target }`: a hook or another process moved the branch during the operation
engine-merge-abort-head-moved = after the abort HEAD is no longer { $head }: a hook or another process moved it
engine-merge-unexpected-result = the branch is at { $after } but it is not the expected result of the merge (did a hook or another process move it?): the operation does not go in the journal, the branch as it was is in `{ $backup }`
engine-merge-head-moved-after = after the merge HEAD is no longer on `{ $full }`: a hook or another process changed it; check HEAD before continuing
