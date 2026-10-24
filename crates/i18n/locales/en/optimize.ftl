# The commit-graph notice under the bar and repo.optimize (optimize.rs, view/optimize_view.rs).
# $seconds: how long the first page of the history took.

optimize-label = Optimizing the repository
optimize-slow = The history took { $seconds } s to load: this repository has no commit-graph, so git reads every commit before showing the first ones.
optimize-running = Writing the commit-graph: on a big repository it takes a while, the app stays usable.
optimize-explanation = Optimize writes the commit-graph once (git commit-graph write --reachable): a cache derived from the commits that makes the history load in milliseconds. No branch, file or commit changes.
optimize-button = Optimize
optimize-not-now = Not now
optimize-not-now-hint = Hide this notice until the repository is opened again
optimize-after-operation = After the current operation
optimize-stop-hint = Stop writing the commit-graph: the repository stays as before
optimize-done = Commit-graph written: the history now loads quickly
optimize-cancelled = Optimization stopped: nothing was changed
optimize-failed = Optimization failed: { $error }
optimize-unexpected-response = unexpected response from `{ $command }`
