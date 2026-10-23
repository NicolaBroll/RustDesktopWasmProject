# App messages: command replies, network bar (lib.rs, executor.rs).

app-commit-not-loaded = commit { $id } is not among the loaded ones
app-index-backup = Index backup saved in { $backup }
app-write-stale = { $label } failed: the file has changed: review the diff and redo the selection (the diff has been reloaded)
app-done-status-unread = Done, but the status could not be reloaded: { $error }
app-no-result = `{ $command }` ended without a result
app-stop-fetch-hint = Stops the fetch: remote branches already updated stay, local branches and files are not touched
app-stop-add-remote-hint = Stops the fetch: the remote stays added, remote branches already downloaded stay
app-stop-pull-hint = Stops the pull during the fetch: the branch is not touched. If the integration has already started it finishes anyway
app-stop-push-hint = Stops the push: if sending had already finished the remote may be updated (a Fetch shows it)
