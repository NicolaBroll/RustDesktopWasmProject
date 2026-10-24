engine-discard-changed = the files to discard changed since the confirmation (confirmed: { $confirmed }; now: { $now }): nothing discarded, reload the status and confirm again
engine-discard-nothing = no unstaged changes to discard in { $paths }
engine-discard-submodules = submodules, nothing discarded: the changes of a submodule are discarded inside the submodule: { $files }
engine-discard-untracked = untracked files, nothing discarded: discarding them would mean deleting them, and the worktree backup (`git stash create`) does not save them: { $files }
engine-discard-in-the-way = untracked or ignored files where the files to discard would go: { $files } (recreating the files would delete them, and the backup does not contain them). Nothing discarded: move them or discard other paths
engine-discard-intent-to-add = there are `intent-to-add` files (`git add -N`: { $files }): the worktree backup (`git stash create`) cannot be created, nothing discarded. Stage them (`index.stage`) or unstage them (`index.unstage`) and try again
engine-discard-conflicts = there are conflicted files ({ $files }): the worktree backup (`git stash create`) cannot be created with a conflicted index, nothing discarded. Resolve the conflicts first
engine-discard-unborn = the branch has no commits yet: the worktree backup (`git stash create`) cannot be created, nothing discarded
engine-discard-no-paths = no paths: nothing to discard
engine-discard-worktree-changed-redo = `{ $path }`: the file changed after the discard was undone
engine-discard-worktree-changed = `{ $path }`: the file changed after the discard
engine-discard-unreadable = `{ $path }` cannot be read: { $error }
engine-discard-nothing-saved = `git stash create` found no changes to save (did the worktree change in the meantime?): nothing discarded, reload the status
engine-discard-backup-incomplete-stash = the worktree backup (`git stash create`, { $stash }) does not have the current content of { $files } (a file that changed just now?): nothing discarded, try again
engine-discard-backup-incomplete = the worktree backup (`{ $backup }`) does not have the current content of { $files } (a file that changed just now?): nothing discarded, try again
engine-discard-in-the-way-git = in the worktree { $files } { $n ->
    [one] is not
   *[other] are not
} what git expects (a file or a link instead of a folder, or a folder instead of a file): recreating the files would delete it, and the backup does not contain it. Nothing written: move it and try again
engine-discard-index-moved = the index changed while the discard was being prepared (another git process?): nothing discarded, try again
engine-discard-no-backup = the worktree backup cannot be created, nothing discarded
engine-discard-backup-saved = the worktree before the discard is saved in `{ $backup }`
engine-discard-backup-kept = the worktree before the discard stays in commit { $commit }
engine-discard-rewrite-failed = `{ $path }` cannot be rewritten ({ $error })
engine-discard-not-as-expected = after `{ $command }` { $files } { $n ->
    [one] is
   *[other] are
} not as expected (a `clean`/`smudge` filter or another program?); { $note }
