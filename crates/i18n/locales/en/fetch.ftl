# Fetch from the top bar and network error windows (fetch.rs).
# $label: the fetch description (fetch-label…); $who: fetch-who…

fetch-label = Fetch
fetch-label-remote = Fetch from “{ $remote }”
fetch-what-branch = Fetch of “{ $branch }” from “{ $remote }”
fetch-unexpected-response = unexpected response from `{ $command }`
fetch-unexpected-result = { $command }: unexpected result: { $error }
fetch-cancelled = { $label } cancelled: remote-tracking branches already updated stay, no local branch or file was touched
fetch-failed-auth = { $label } failed: authentication rejected or required
fetch-failed-unreachable = { $label } failed: remote unreachable
fetch-not-possible = { $label } not possible: { $error }
fetch-failed = { $label } failed: { $error }
fetch-failed-title = Fetch failed
fetch-who = The remote
fetch-who-remote = The remote “{ $remote }”
fetch-nothing-changed = Nothing was changed.
fetch-auth-title = Authentication failed
fetch-auth-explanation = { $who } rejected the credentials, or they would need to be asked for (username, password, SSH key passphrase, host key confirmation). This app never asks for credentials: configure a credential helper (git config credential.helper) or load the key into ssh-agent (ssh-add); for a new SSH host, add it to known_hosts with a first fetch from a terminal. Then try again.
fetch-unreachable-title = Remote unreachable
fetch-unreachable-explanation = { $who } is not responding: check the remote's address (git remote -v), the network, and that the repository exists on the server.
fetch-ref-tag = tag { $tag }
fetch-done = { $what } done
fetch-nothing-new = { $what }: nothing new
fetch-ref-new = { $name } new
fetch-ref-removed = { $name } deleted
fetch-and-more = and { $n } more
fetch-updated = { $what }: { $n ->
    [one] 1 ref updated
   *[other] { $n } refs updated
} ({ $names })
fetch-pruned-backups = ; { $n ->
    [one] the commits only on a deleted branch are kept in a backup
   *[other] the commits only on { $n } deleted branches are kept in backups
} (refs/project/backup/prune-…)
fetch-warning = ; warning: { $warning }
fetch-refs-not-reread = Fetch done, but the refs could not be reread: { $error }
fetch-progress-remote = remote: { $phase }
fetch-phase-enumerating = Enumerating objects
fetch-phase-counting = Counting objects
fetch-phase-compressing = Compressing objects
fetch-phase-receiving = Receiving objects
fetch-phase-unpacking = Unpacking objects
fetch-phase-resolving = Resolving deltas
fetch-phase-writing = Writing objects
fetch-phase-total = Total
fetch-phase-expanding-graph = Collecting commits
fetch-phase-writing-graph = Writing the commit-graph
fetch-auto-label = Automatic fetch
fetch-auto-news = Automatic fetch: { $news }
fetch-auto-new-commits = { $n ->
    [one] 1 new commit on { $branch }
   *[other] { $n } new commits on { $branch }
}
fetch-auto-branch-moved = { $branch } updated
fetch-auto-new-branch = new branch { $branch }
fetch-auto-branch-gone = { $branch } removed
fetch-auto-suspended = Automatic fetch paused: the remote rejected the credentials.
fetch-auto-suspended-hint = The app never asks for credentials: git takes them from its configuration (credential helper, ssh-agent). Fix them, then press “Retry”.
fetch-auto-retry = Retry
fetch-auto-retry-hint = Fetch again now and, if it works, resume the automatic fetch.
fetch-auto-offline = The remote cannot be reached: the automatic fetch tries again in { $minutes ->
    [one] 1 minute
   *[other] { $minutes } minutes
} (the wait doubles at every error, up to 30 minutes).
