# Top bar, status bar and welcome screen (view/top_bar.rs).

top-bar-refresh = Refresh
top-bar-refresh-hint = Reloads refs, status and log
top-bar-after-operation = After the current operation
top-bar-stash = Stash…
top-bar-undo-in-operation = { $hint } (not while an operation is in progress: finish or abort it from the panel at the top)
top-bar-undo-after-operation = { $hint } (after the current operation)
top-bar-changed-files = { $n ->
    [one] 1 changed file
   *[other] { $n } changed files
}
top-bar-head-unborn = { $name } (no commits yet)
top-bar-head-detached = HEAD detached at { $id }
top-bar-fetch = Fetch
top-bar-fetch-hint = Downloads commits, remote branches and tags from the remote of the current branch's upstream (otherwise origin, or the only remote). Does not touch local branches, the index or files
top-bar-fetch-from-menu = From…
top-bar-fetch-from = Fetch from “{ $remote }”
top-bar-fetch-from-hint = Choose the remote to fetch from
top-bar-pull = Pull
top-bar-push = Push
top-bar-detached-hint = HEAD is detached (or the branch has no commits): switch to a branch
top-bar-pull-hint = Downloads { $upstream } and integrates it into “{ $branch }”: fast-forward when possible, otherwise merge or rebase according to pull.rebase (merge if not configured). With local changes the merge does not start. “Undo” puts the branch back where it was
top-bar-push-hint = Publishes “{ $branch }” to { $upstream }. Never forced: if the remote has commits you don't have, the push is rejected and Pull is suggested. A push cannot be undone
top-bar-pull-no-upstream-hint = “{ $branch }” has no upstream: pull explains how to set it (or Push, which sets it)
top-bar-push-first-hint = First push of “{ $branch }”: publishes it to origin (or the only remote) with the same name and sets the upstream. A push cannot be undone
top-bar-connecting = Connecting to the remote…
top-bar-stop = Stop
top-bar-stopping = Stopping…
top-bar-running = { $what } in progress…
top-bar-running-queued = { $what } in progress… (+{ $n } queued)
top-bar-opening = Opening the repository…
top-bar-no-repo = No repository open
top-bar-open-repo = Open a repository…
top-bar-recent = Open recent
top-bar-recent-hint = Repositories opened recently: one opens in a tab, or shows its tab if already open
top-bar-recent-none = No repository opened yet
top-bar-recent-item = { $name } — { $path }
top-bar-recent-heading = Recent repositories
top-bar-recent-missing-hint = The folder { $path } no longer exists
top-bar-recent-forget = Remove missing
top-bar-recent-forget-hint = Drops from the recent list the repositories whose folder no longer exists (no folder is touched)
