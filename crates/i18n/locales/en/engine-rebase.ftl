# Interactive rebase (`rebase.plan`, `rebase.start`, `rebase.continue`, `rebase.abort`, S32).
engine-rebase-what = the rebase
engine-rebase-base-empty = `base` must be a commit id, a branch or a tag, not empty
engine-rebase-unborn = branch `{ $name }` has no commits yet: `{ $id }` has nothing to rewrite, nothing changed
engine-rebase-detached = HEAD is detached: `{ $id }` works on a branch, nothing changed. Switch to a branch (`branch.checkout`) and try again
engine-rebase-base-is-head = { $base } is HEAD: there are no commits to rewrite on top of it (`{ $id }` takes an earlier commit as `base`)
engine-rebase-not-ancestor = { $base } is not an ancestor of HEAD: `{ $id }` rewrites the commits of the branch on top of one of its earlier commits (to bring in other commits use `merge.start` or `remote.pull`)
engine-rebase-merges = the commits to rewrite include merge commits ({ $commits }): `{ $id }` supports only a linear history, nothing changed
engine-rebase-too-many = more than { $max } commits to rewrite: `{ $id }` refuses, choose a more recent `base`
engine-rebase-conflicts = there are conflicted files ({ $files }): resolve them before `rebase.start`; nothing changed
engine-rebase-local-changes = there are local changes to tracked files ({ $files }): commit or stash them (`stash.push`) and retry `rebase.start` (no autostash); nothing changed
engine-rebase-noop = the plan keeps every commit as it is, in the same order: `rebase.start` would change nothing
engine-rebase-published = the plan rewrites commits already published ({ $commits }, reachable from remote branches): whoever downloaded them would find a different history and the next push would have to be forced. Repeat with "confirm": true to rewrite them
engine-rebase-head-moved = HEAD changed while `rebase.start` was being prepared: nothing changed, try again
engine-rebase-range-changed = the commits to rewrite changed while `rebase.start` was being prepared: nothing changed, read the plan again
engine-rebase-untracked = the rebase would write files in place of untracked or ignored files ({ $files }): move or remove them and try again; nothing changed
engine-rebase-continue-conflicts = conflicted files remain ({ $files }): resolve them and stage them (`index.stage`), then repeat `rebase.continue`
engine-rebase-terminal = the rebase was not started by the app (or its data is missing or does not match it): it is concluded, but it does not go in the journal
engine-rebase-unexpected-result = the branch is at { $after } but it is not the expected result of the rebase (HEAD not on the branch, or the branch did not move): the operation does not go in the journal, the branch as it was is in `{ $backup }`
engine-rebase-count-mismatch = the rebase produced { $found } commits instead of the { $expected } of the plan (git drops a step resolved to nothing): check the history; the branch as it was is in `{ $backup }`
engine-rebase-not-checked = the result of the rebase could not be checked ({ $error })
engine-rebase-dropped-empty = the commit { $commit }, resolved as HEAD was, was dropped (nothing to commit), as git does
engine-rebase-memory-no-state = the rebase in progress has no data in the in-memory repo (`set_operation`): it cannot be continued
engine-rebase-todo-not-written = the plan of the rebase could not be written ({ $error }): nothing changed
engine-rebase-record-not-written = the data of the stopped rebase could not be saved ({ $error }): `rebase.continue` will conclude it without a journal entry; the branch as it was is in `{ $backup }`
engine-rebase-branch-not-moved = the rebase finished but `{ $full }` did not move ({ $text }): check the history; the branch as it was is in `{ $backup }`
engine-rebase-branch-gone = after the rebase `{ $full }` no longer exists; the branch as it was is in `{ $backup }`
engine-rebase-messages-not-applied = the rebase concluded but { $n } { $n ->
    [one] message
   *[other] messages
} of the plan were not applied (was it continued outside the app?): check the history; the branch as it was is in `{ $backup }`
engine-rebase-message-skipped = the pause after { $commit } did not follow the step the plan put it after: its message was not applied, check the history
engine-rebase-abort-untracked-refused = the abort would bring back files that now exist as untracked files ({ $files }): git would refuse halfway. Move or remove them and try again; nothing changed
engine-rebase-abort-untracked = the abort brought back files that were ignored ({ $files }): git overwrote them, they are in the backup
# Plan errors (`InvalidParams`): `engine-rebase-plan-step` wraps the others with the step (from 1).
engine-rebase-plan-step = step { $step }: { $reason }
engine-rebase-plan-unknown-commit = commit { $commit } is not in the range being rewritten
engine-rebase-plan-duplicate-commit = commit { $commit } appears more than once
engine-rebase-plan-missing-commit = commit { $commit } of the range is missing from the plan (use `drop` to remove it)
engine-rebase-plan-all-dropped = every commit is dropped (to remove all of them move the branch instead)
engine-rebase-plan-nothing-to-combine = the first commit kept cannot be `squash` or `fixup`: there is nothing to combine it with
engine-rebase-plan-missing-message = `reword` needs a `message`
engine-rebase-plan-unexpected-message = a `message` is allowed only on `reword` and on the last `squash`/`fixup` of a group
engine-rebase-plan-reword-in-group = `reword` cannot start a group with `squash`/`fixup`: use `pick` and put the message on the last step of the group
engine-rebase-plan-empty-message = the message is empty
engine-rebase-plan-nul-in-message = the message contains a NUL byte
