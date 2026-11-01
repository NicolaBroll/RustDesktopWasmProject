# Linked worktrees (`worktree.list`, `worktree.add`, `worktree.remove`, impl-docs/git/worktrees.md).

engine-worktree-path-newline = the path of the new worktree must not contain a newline
engine-worktree-branch-and-new = give `branch` (an existing branch) or `new_branch` (a new one), not both
engine-worktree-branch-and-start = `start` goes with `new_branch` or a detached worktree, not with an existing `branch`
engine-worktree-branch-in-use = `{ $branch }` is already checked out in the worktree `{ $path }`: nothing created
engine-worktree-branch-not-rolled-back = the worktree could not be created ({ $error }) and the branch `{ $name }` just created could not be deleted again ({ $rollback }): delete it by hand if it is not needed
engine-worktree-unknown = `{ $path }` is not a worktree of this repository, nothing removed
engine-worktree-main = `{ $path }` is the main worktree: it is never removed
engine-worktree-current = `{ $path }` is the worktree open here: open another worktree to remove it
engine-worktree-locked = `{ $path }` is locked: unlock it with `git worktree unlock` first
engine-worktree-locked-reason = `{ $path }` is locked ({ $reason }): unlock it with `git worktree unlock` first
engine-worktree-operation = an operation ({ $operation }) is in progress in `{ $path }`: finish or abort it there first
engine-worktree-changes = `{ $path }` has { $count ->
    [one] a changed file
   *[other] { $count } changed files
} (`{ $first }`): commit or stash them there first
engine-worktree-untracked = `{ $path }` has { $count ->
    [one] an untracked file
   *[other] { $count } untracked files
} (`{ $first }`): commit, move or remove them there first
engine-worktree-ignored = `{ $path }` has { $count ->
    [one] an ignored file
   *[other] { $count } ignored files
} (`{ $first }`), which git would delete without a backup: move or remove them first
engine-worktree-hidden = `{ $path }` has { $count ->
    [one] a file
   *[other] { $count } files
} marked assume-unchanged or skip-worktree (`{ $first }`): git cannot tell whether they changed; clear the flag there first
engine-worktree-not-worktree = the folder `{ $path }` does not hold that worktree any more (`.git` missing or pointing elsewhere): nothing removed
engine-worktree-remove-confirm = the worktree `{ $path }` and its folder would be deleted (it has no changes, untracked or ignored files){ $backup ->
    [yes] ; its detached HEAD has commits on no branch, tag or remote branch: they would first be saved in `refs/project/backup/worktree-...`
   *[other] {""}
}. Repeat with "confirm": true
engine-worktree-changed = `{ $path }` changed while the removal was being prepared: check it and try again
engine-worktree-remove-failed = { $error } (the detached HEAD of the worktree is saved in `{ $backup }`)
engine-worktree-add-hook = the worktree `{ $path }` was created, but git reported an error afterwards (a `post-checkout` hook?): { $message }
engine-worktree-add-unexpected = the new worktree `{ $path }` is not on `{ $expected }` as requested but on `{ $found }` (empty: it is not listed): another process changed it meanwhile?
