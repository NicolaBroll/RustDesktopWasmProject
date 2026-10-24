engine-stash-nothing-untracked = nothing to stash: no changes and no untracked files
engine-stash-nothing-untracked-paths = nothing to stash: no changes and no untracked files among the chosen paths
engine-stash-nothing = nothing to stash: no changes to tracked files (untracked files are included only with `include_untracked`)
engine-stash-nothing-paths = nothing to stash: no changes to tracked files among the chosen paths (untracked files are included only with `include_untracked`)
engine-stash-blocked = `{ $command }` of `{ $stash }` would overwrite files with local changes, untracked or ignored ({ $files }): nothing changed. Commit, stash or move them and try again
engine-stash-apply-conflicts = there are conflicted files ({ $files }): resolve them before applying a stash, nothing changed
engine-stash-intent-to-add = there are `intent-to-add` files (`git add -N`: { $files }): git cannot save them in a stash. Stage or unstage them and try again, nothing changed
engine-stash-push-conflicts = there are conflicted files ({ $files }): resolve them before stashing the changes, nothing changed
engine-stash-unborn = the branch has no commits yet: a stash is based on HEAD, nothing stashed
engine-stash-message-control = the stash message is a single line, without newlines or control characters
engine-stash-memory-unborn = the branch has no commits yet
engine-stash-gone = `{ $command }`: stash { $id } is no longer in the list, nothing changed
engine-stash-none = there are no stashes, nothing changed
engine-stash-no-index = there is no `stash@{"{"}{ $index }{"}"}`: there are { $n } stashes (from `stash@{"{"}0{"}"}` to `stash@{"{"}{ $last }{"}"}`), nothing changed
engine-stash-now-at = is now `{ $name }`
engine-stash-now-gone = is no longer in the list
engine-stash-list-changed = the stash list changed since it was seen: `stash@{"{"}{ $index }{"}"}` is { $id }, stash { $seen } { $now }. Nothing changed; reload the stashes (`stash.list`) and try again
# `$files`: the tracked files, -1 if unknown; `$untracked`: `yes` or `no`.
engine-stash-drop-confirm = stash `{ $name }` ({ $id } “{ $message }”{ $files ->
    [-1] {""}
    [one] , 1 tracked file
   *[other] , { $files } tracked files
}{ $untracked ->
    [yes] , with untracked files
   *[no] {""}
}) would be dropped. Its commit would first be saved in `refs/project/backup/stash-...`, and `edit.undo` would put it back at the top of the list. To drop it, repeat with `"expected": "{ $id }"`
engine-stash-confirm-without-expected = `confirm` goes together with `expected`, the full id of the stash to drop (the one given by the confirmation or by `stash.list`): the confirmation applies only to the stash that was seen
engine-stash-expected-hex = `expected` must be the full id ({ $len } hexadecimal digits) of the stash: `{ $value }`
# The key is `INDEX_NOT_RESTORED` (`git/stash.rs`): the undo of `stash.push` recognizes it.
engine-stash-index-not-restored = with `restore_index`, the staged changes of `{ $name }` do not apply on top of the current index: nothing changed; try again without `restore_index` (everything goes back to unstaged)
engine-stash-drop-list-changed = the stash list changed while `{ $command }` was being prepared (another process?): the stash was not dropped, reload the stashes and try again
engine-stash-drop-gone = `{ $command }`: stash { $id } is no longer in the list, nothing dropped
engine-stash-apply-refused = `{ $command }` of `{ $name }` refused by git: it would overwrite local changes to { $files }. Nothing changed; commit, stash or discard them and try again
engine-stash-apply-refused-partly = `{ $command }` of `{ $name }` refused by git: it would overwrite local changes to { $files }. The tracked files were not touched, but git may have already put back the untracked files of the stash (the apply may have stopped halfway; they are also in the stash, which stays in the list)
engine-stash-apply-untracked-exist = `{ $command }` of `{ $name }` stopped halfway: git already applied the changes to the tracked files, but did not put back the untracked ones because they already exist ({ $files }). The stash stays in the list; check the worktree before trying again
engine-stash-apply-assume-unchanged = files marked `--assume-unchanged` that the stash changes ({ $files }): git would overwrite their changes without checking them. Nothing changed; remove the mark (`git update-index --no-assume-unchanged`) and try again
engine-stash-not-created = nothing to stash: git did not create the stash ({ $output }), nothing changed
engine-stash-push-assume-unchanged = files marked `--assume-unchanged` ({ $files }): git would not stash their changes and would then delete them by resetting the worktree to HEAD. Nothing changed; remove the mark (`git update-index --no-assume-unchanged`) or exclude them with `paths`, and try again
engine-stash-store-not-on-top = after `git stash store`, stash { $id } is not at the top of the list: reload the stashes
engine-stash-ls-tree-line = unrecognized line: `{ $line }`
engine-stash-not-on-top = stash { $id } was created, but it is not at the top of the list (another process?): reload the stashes
engine-stash-list-unreadable = unrecognized stash list: `{ $output }`
engine-stash-push-stopped = stash { $id } was created, but then git stopped ({ $stderr }): the changes are in the stash; check the worktree before trying again
engine-stash-not-dropped = stash `{ $name }` was applied but not removed from the list ({ $error }): the changes are in the files; drop it with `stash.drop` (do not repeat the pop)
engine-stash-kept-conflicts = stash `{ $name }` stays in the list because there are conflicts: resolve them (`index.stage` of the files) and then drop it with `stash.drop`
engine-stash-list-unexpected = after `{ $command }` the stash list is not the expected one (did another process change it?): reload it; the dropped stash is in `{ $backup }`
engine-stash-list-unexpected-no-backup = after `{ $command }` the stash list is not the expected one (did another process change it?): reload it; the dropped stash is among the backups
engine-stash-list-unreadable-after = after `{ $command }` the stashes cannot be read ({ $error })
