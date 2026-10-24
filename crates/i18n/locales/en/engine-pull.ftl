engine-pull-local-changes = the pull would do a { $mode } but there are local changes to tracked files ({ $files }): commit or stash them and try again (no `--autostash`); the branch was not touched
engine-pull-conflicts = there are conflicted files ({ $files }): resolve them before the pull; the branch was not touched
engine-pull-diverged-ff-only = `{ $branch }` and its upstream have diverged ({ $ahead } local { $ahead ->
    [one] commit
   *[other] commits
}, { $behind } from the upstream) and the strategy is fast-forward only (`pull.ff=only` or `strategy` `ff_only`): the fetch is done, nothing else changed. Repeat with `strategy` `merge` or `rebase`
engine-pull-diverged-not-current = `{ $branch }` and its upstream have diverged ({ $ahead } local { $ahead ->
    [one] commit
   *[other] commits
}, { $behind } from the upstream) and `{ $branch }` is not the current branch: only a fast-forward is possible. The fetch is done, nothing else changed; switch to the branch (`branch.checkout`) and repeat the pull
engine-pull-interactive = the configuration asks for an interactive rebase (`pull.rebase` or `branch.<name>.rebase` = `interactive`), which needs an editor: nothing changed. Give `strategy` (`merge`, `rebase` or `ff_only`)
engine-pull-config-value = unrecognized value of `{ $key }`: `{ $value }`; nothing changed (fix the configuration or give `strategy`)
engine-pull-no-branch = there is no branch `{ $branch }`
engine-pull-undo-overwrite = `{ $command }` would move `{ $moved }` back to { $target } but would overwrite local changes or untracked files ({ $files }): nothing changed; commit or stash them and try again
engine-pull-bad-request = `{ $command }`: invalid request for the current branch, nothing changed
engine-pull-overwrite = the pull would overwrite local changes or untracked files ({ $files }): commit, stash or move them and try again; the branch was not touched
engine-pull-rebase-untracked = the rebase would go through files that now exist in the worktree as untracked or ignored ({ $files }): git would overwrite them. The branch was not touched; move or rename them and try again
engine-pull-not-current-ff = `{ $branch }` is not the current branch: only a fast-forward is possible, nothing changed
engine-pull-head-moved = HEAD changed while the pull was being prepared: nothing changed, try again
engine-pull-tracking-missing = after the fetch `{ $tracking }` does not exist: nothing to integrate, the branch was not touched
engine-pull-no-refspec = no fetch refspec of remote `{ $remote }` fetches `{ $merge }` into a remote-tracking branch (`remote.{ $remote }.fetch`): nothing fetched
engine-pull-upstream-not-branch = the upstream of `{ $branch }` is not a branch (`{ $merge }`): nothing fetched
engine-pull-other-remote = `{ $branch }` tracks remote `{ $upstream }`, not `{ $remote }`: the pull always integrates the upstream. Nothing fetched; to track another remote, change the upstream
engine-pull-local-upstream = the upstream of `{ $branch }` is a local branch (`{ $merge }`): pulling from a local upstream is not supported, nothing changed
engine-pull-no-branch-commits = there is no local branch `{ $branch }` with commits: nothing fetched
engine-pull-detached = HEAD is detached: nothing fetched. Switch to a branch (`branch.checkout`) or give `branch`
engine-pull-no-upstream = branch `{ $branch }` has no upstream (`branch.{ $branch }.remote`/`branch.{ $branch }.merge`): nothing fetched or changed. Set it from a terminal with `git branch --set-upstream-to=<remote>/<branch> -- { $branch }`, or publish the branch with `remote.push`, which sets the upstream on the first push
engine-pull-branch-gone = after the pull, `{ $full }` no longer exists
engine-pull-overwrite-what = { $what } would overwrite local changes or untracked files ({ $files }): commit, stash or move them and try again; the branch was not touched
engine-pull-undo-bad-entry = `{ $command }`: invalid journal entry for the current branch, nothing changed
engine-pull-undo-overwrite-git = `{ $command }` would move `{ $name }` back to { $to } but would overwrite local changes: nothing changed; commit or stash them and try again ({ $stderr })
engine-pull-undo-untracked = `{ $command }` would recreate files that now exist in the worktree as untracked or ignored ({ $files }): nothing changed; move them and try again
engine-pull-other-worktree = `{ $branch }` is the current branch of worktree `{ $worktree }`: the pull must be done there, nothing changed
engine-pull-incoming-untracked = { $what } would bring files that now exist in the worktree as untracked or ignored ({ $files }): git would overwrite them. The branch was not touched; move or rename them and try again
engine-pull-aborted =
    { $text }
    (the { $operation } was aborted, nothing changed)
engine-pull-note-in-progress = the repository was left with an operation in progress; the branch as it was is in `{ $backup }`
engine-pull-note-moved = `{ $full }` however moved from { $from } to { $to }: git wrote something before failing. The pull does not go into the journal; the branch as it was is in `{ $backup }`
engine-pull-head-moved-merge = after the pull HEAD is no longer on `{ $full }`: a hook or another process changed it; check HEAD before continuing the merge
engine-pull-branch-moved = `{ $full }` points to { $now } instead of { $expected }: a hook or another process moved it; the branch as it was is in `{ $backup }`
engine-pull-merging-other = the merge in progress joins { $incoming } instead of `{ $name }` ({ $upstream }): a hook or another process changed it; check it before continuing it
engine-pull-upstream-missing = after the pull `{ $full }` ({ $after }) does not contain `{ $name }`: a hook or another process moved the branch during the operation
engine-pull-head-moved-after = after the pull HEAD is no longer on `{ $full }`: a hook or another process changed it; check HEAD before continuing
