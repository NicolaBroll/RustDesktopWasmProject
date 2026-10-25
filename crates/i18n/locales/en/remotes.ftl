# Remotes and upstream (remotes.rs).

remotes-ahead = { $n ->
    [one] 1 commit to push
   *[other] { $n } commits to push
}
remotes-behind = { $n ->
    [one] 1 commit to pull
   *[other] { $n } commits to pull
}
remotes-in-sync = Up to date with { $name }
remotes-relative = { $counts } compared to { $name }
remotes-upstream-gone = upstream gone
remotes-upstream-gone-hint = The upstream { $name } no longer exists (deleted on the remote, or the remote no longer has it): a Fetch won't bring it back; Push recreates it

# “Remotes” section, adding and removing (remotes.rs, view/remotes_view.rs).
remotes-no-url = (no URL)
remotes-no-branches = no branches
remotes-branches = { $n ->
    [one] 1 branch
   *[other] { $n } branches
}
remotes-badge-default = default
remotes-badge-default-hint = “Fetch” in the top bar fetches from here (upstream of the current branch, origin, or the only remote)
remotes-badge-push = push
remotes-badge-push-hint = remote.pushDefault: pushes of branches without pushRemote go to “{ $name }”
remotes-badge-mirror = mirror
remotes-badge-mirror-hint = remote.<name>.mirror: a push mirrors all local refs
remotes-tooltip =
    Remote “{ $name }”
    URL: { $url }
remotes-tooltip-push-url = Push: { $url }
remotes-tooltip-credentials = The URL contains credentials, stored in plain text in the repo configuration (masked here): a credential helper is better
remotes-credentials-warning = The URL contains credentials: git stores them in plain text in the repo configuration (.git/config). Better a URL without them and a credential helper (or an SSH key).
remotes-add-label = Adding remote “{ $name }”
remotes-add-label-fetch = Adding remote “{ $name }” and fetching
remotes-unexpected-result = { $command }: unexpected result: { $error }
remotes-unexpected-response = unexpected response from `{ $command }`
remotes-remove-label = Removing remote “{ $name }”
remotes-remove-check-label = Checking removal of “{ $name }”
remotes-add-failed = Adding remote “{ $name }” failed: { $error }
remotes-remove-refused = Removing remote “{ $name }” not possible: { $error }
remotes-remove-failed = Removing remote “{ $name }” failed: { $error }
remotes-warning = ; warning: { $warning }
remotes-refs-not-reread = the refs could not be reread: { $error }
remotes-remotes-not-reread = the remotes could not be reread: { $error }
remotes-reread-warning = { $what }, but { $problems }
remotes-added-anyway = The remote was added anyway, with no branches fetched: try again with “Fetch” in the Remotes section, or “Undo” to remove it.
remotes-added = Remote “{ $name }” added ({ $url })
remotes-added-fetch-cancelled = ; fetch cancelled: remote-tracking branches already fetched stay, “Fetch” in the Remotes section resumes it
remotes-added-fetch-auth = ; fetch failed: authentication rejected or required
remotes-added-fetch-unreachable = ; fetch failed: remote unreachable
remotes-added-fetch-failed = ; fetch failed
remotes-fetch-failed-title = Fetch failed
remotes-added-undo = . “Undo” removes it
remotes-added-short = Remote added
remotes-quoted = “{ $name }”
remotes-removed = Remote “{ $name }” removed ({ $n ->
    [0] no remote-tracking branches to delete
    [one] deleted 1 remote-tracking branch
   *[other] deleted { $n } remote-tracking branches
})
remotes-removed-upstreams = ; no upstream anymore: { $names }
remotes-removed-push-remotes = ; removed the pushRemote of { $names }
remotes-removed-push-default = ; removed remote.pushDefault
remotes-removed-backup = ; backup in { $backup }
remotes-removed-backups = ; { $n } backups in refs/project/backup/remote-…
remotes-removed-undo = . “Undo” restores it
remotes-removed-short = Remote removed
remotes-delete-branch-check-label = Checking deletion of “{ $name }”
remotes-delete-branch-label = Deleting “{ $name }” from the remote
remotes-delete-branch-refused = Deleting “{ $name }” from the remote not possible: { $error }
remotes-delete-branch-failed = Deleting “{ $name }” from the remote failed: { $error }
remotes-branch-deleted = Branch “{ $name }” deleted from the remote
remotes-branch-deleted-only-here = ; { $n ->
    [one] 1 commit was
   *[other] { $n } commits were
} only there
remotes-branch-deleted-backup = ; backup in { $backup }
remotes-branch-deleted-upstreams = ; upstream gone for { $names }
remotes-branch-deleted-short = Branch deleted from the remote
remotes-section = Remotes
remotes-section-count = Remotes ({ $n })
remotes-none = No remotes configured
remotes-loading = Reading remotes…
remotes-list-error = Remotes not read: { $error }
remotes-add-button-hint = Adds a remote (name and URL) and, if chosen, fetches its branches right away. “Undo” removes it
remotes-row-hint =
    { $tooltip }
    Right-click: fetch, remove
remotes-menu-hint = Fetch, remove
remotes-fetch-button = Fetch
remotes-fetch-button-hint = Downloads commits, remote-tracking branches and tags from “{ $name }”. Doesn't touch local branches, the index or files
remotes-remove-button = Remove…
remotes-remove-button-hint = Removes the remote from this repo, with its fetched remote-tracking branches and the upstreams using it; asks for confirmation first. The repository on the server doesn't change; “Undo” restores it
remotes-add-title = Add remote
remotes-add-explanation = A remote is another repository (on a server or in a folder) to fetch from and push to. It is added with the remote-tracking branches name/* and nothing changes in local branches; “Undo” removes it.
remotes-add-name = Name
remotes-add-name-hint = e.g. origin, upstream
remotes-add-url = URL
remotes-add-url-hint = https://host/project.git, git@host:user/project.git
remotes-show-url = Show URL
remotes-show-url-hint = Shows the URL as written, to correct it
remotes-hide-credentials = Hide credentials
remotes-hide-credentials-hint = Shows the URL with the credentials masked
remotes-add-fetch = Fetch right away
remotes-add-fetch-hint = After adding, downloads commits, remote-tracking branches and tags. If the fetch fails (or is cancelled) the remote stays added
remotes-add-submit = Add
remotes-close = Close
remotes-adding = Adding…
remotes-connecting = Connecting to the remote…
remotes-cancel = Cancel
remotes-cancel-hint = Stops the fetch: the remote stays added, remote-tracking branches already fetched stay
remotes-cancelling = Cancelling…
remotes-remove-title = Remove remote “{ $name }”?
remotes-remove-explanation = The remote's configuration and its fetched remote-tracking branches are removed from this repo; local branches tracking it stay, but without upstream. The repository on the server doesn't change and fetched tags stay.
remotes-remove-what = What happens:
remotes-remove-undo = With “Undo” in the top bar the remote goes back to how it was (URL, remote-tracking branches, upstreams), as long as the app stays open: the operation journal is in memory. After closing, only the commit backups in refs/project/backup/remote-… remain: re-add the remote by hand and a Fetch downloads the branches again.
remotes-remove-confirm = Remove remote
remotes-remove-cancel = Don't remove
remotes-delete-branch-title = Delete “{ $name }” from the remote?
remotes-delete-branch-explanation = The branch “{ $branch }” is deleted on the remote “{ $remote }”: it disappears for everyone who uses it, and its remote-tracking branch disappears here too. Local branches don't change.
remotes-delete-branch-undo = It can't be undone with “Undo”: the remote belongs to others too. The commit stays here in a backup (refs/project/backup/push-delete-…): to put the branch back, create a branch on it and push it. If someone pushed to the branch after your last Fetch, the deletion is refused and nothing changes.
remotes-delete-branch-confirm = Delete from the remote
remotes-delete-branch-cancel = Don't delete
