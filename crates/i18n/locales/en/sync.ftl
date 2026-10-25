# Pull and push (sync.rs, view/sync_view.rs).
# $label: the operation description (sync-label-…); $what: sync-label-pull-branch.

sync-label-pull = Pull
sync-label-pull-branch = Pull of “{ $branch }”
sync-label-push = Push
sync-label-push-branch = Push of “{ $branch }”
sync-label-force = Force push
sync-label-force-branch = Force push of “{ $branch }”
sync-unexpected-response = unexpected pull or push response
sync-unexpected-result = { $command }: unexpected result: { $error }
sync-pull-cancelled = { $label } cancelled: the branch was not touched (remote-tracking branches already downloaded stay)
sync-push-cancelled = { $label } cancelled: if the upload had already finished the remote may have been updated; Fetch to check
sync-failed-auth = { $label } failed: authentication rejected or required
sync-failed-unreachable = { $label } failed: remote unreachable
sync-pull-conflict = { $label } aborted: the { $operation } has conflicts, the repo is as it was
sync-rejected = { $label } rejected: { $reason }
sync-not-possible-title = { $label } not possible
sync-not-possible = { $label } not possible: { $error }
sync-failed-title = { $label } failed
sync-failed = { $label } failed: { $error }
sync-rejection-non-fast-forward = The remote has commits you don't have: Pull to integrate them, then push again. Nothing changed, neither on the remote nor locally.
sync-rejection-stale-lease = The branch on the remote has moved since the last fetch: the force push (with lease) was rejected so as not to delete commits you haven't seen. Fetch and check the incoming commits before deciding again. Nothing changed.
sync-rejection-remote = The remote rejected the update (a protected branch, or a pre-receive or update hook on the server). The reason, if the server gives one, is in git's message. Nothing changed.
sync-rejection-hook = A pre-push hook (or a local git check, such as unpublished submodules) stopped the push before it started. Its output is below. Nothing changed.
sync-rejection-other = Git rejected the push. Nothing changed.
sync-rejection-short-non-fast-forward = the remote has commits you don't have: Pull
sync-rejection-short-stale-lease = the remote has moved since the last fetch: Fetch
sync-rejection-short-remote = the remote rejected the update
sync-rejection-short-hook = a pre-push hook stopped it
sync-rejection-short-other = rejected by git
sync-pull-stopped = { $what }: merge of { $upstream } stopped before the commit (a hook or signing?): finish it with “Continue merge…” in the top panel, or abort it
sync-pull-stopped-conflicts = { $what }: merge of { $upstream } stopped: { $n ->
    [one] 1 conflicted file
   *[other] { $n } conflicted files
}. Resolve them and mark them as resolved in the staging area, then “Continue merge…” in the top panel (or “Abort merge…”, which puts the branch back as it was)
sync-pull-stopped-rebase = { $what }: rebase onto { $upstream } stopped before a commit (a hook or signing?): finish it with “Continue rebase…” in the top panel, or abort it
sync-pull-stopped-conflicts-rebase = { $what }: rebase onto { $upstream } stopped: { $n ->
    [one] 1 conflicted file
   *[other] { $n } conflicted files
}. Resolve them and mark them as resolved in the staging area, then “Continue rebase…” in the top panel (or “Abort rebase…”, which puts the branch back as it was)
sync-pull-up-to-date = { $what }: already up to date with { $upstream }
sync-pull-up-to-date-ahead = { $what }: already up to date with { $upstream } ({ $n ->
    [one] 1 local commit
   *[other] { $n } local commits
} to publish with Push)
sync-pull-fast-forward = { $what }: fast-forward to { $upstream } ({ $range }, { $n ->
    [one] 1 new commit
   *[other] { $n } new commits
})
sync-pull-merge = { $what }: merge of { $upstream } ({ $range })
sync-pull-rebase = { $what }: rebase of { $n ->
    [one] 1 local commit
   *[other] { $n } local commits
} onto { $upstream } ({ $range })
sync-pull-other = { $what }: { $kind } ({ $range })
sync-pull-merge-default = ; merge because pull.rebase is not configured (git config pull.rebase true to rebase)
sync-pull-backup-in-progress = ; the branch as it was is in { $backup }
sync-pull-backup = ; backup in { $backup }, “Undo” reverts it
sync-warning = ; warning: { $warning }
sync-pull-status-not-reread = Pull done, but the status could not be reread: { $error }
sync-pull-refs-not-reread = Pull done, but the refs could not be reread: { $error }
# $summary: empty or “ (summary)” with the leading space.
sync-push-new = Push of “{ $branch }”: created { $target } on the remote{ $summary }
sync-push-fast-forward = Push of “{ $branch }” to { $target }{ $summary }
sync-push-forced = Force push of “{ $branch }” to { $target }{ $summary }: { $n ->
    [one] 1 commit
   *[other] { $n } commits
} removed from the remote
sync-push-up-to-date = Push of “{ $branch }”: { $target } is already up to date
sync-push-other = Push of “{ $branch }” to { $target }: { $kind }
sync-push-to-push-remote = ; to “{ $remote }” (branch.{ $branch }.pushRemote)
sync-push-to-push-default = ; to “{ $remote }” (remote.pushDefault)
sync-push-backup = ; the remote as it was is saved in { $backup }
sync-push-set-upstream = ; upstream set to { $target }
sync-push-status-not-reread = Push done, but the status could not be reread: { $error }
sync-push-refs-not-reread = Push done, but the refs could not be reread: { $error }
sync-close = Close
sync-conflict-title = Pull aborted: conflicts
sync-conflict-explanation = The { $operation } with the upstream stopped on a conflict and was aborted: the repo is as it was before the pull (branch, index and files didn't change). The fetch, however, is done: the remote-tracking branch shows the incoming commits.
sync-conflict-hint = You can pull again with { $other } (or the same strategy): a pull that stops on a conflict now stays in progress and the files are resolved here, then “Continue…” or “Abort…” in the top panel; or keep working on the branch and integrate later, or resolve from a terminal.
sync-conflict-no-files = Git didn't list the files.
sync-conflict-files = { $n ->
    [one] Conflicted file:
   *[other] { $n } conflicted files:
}
sync-engine-message = Engine message
sync-pull-with = Pull with { $other }
sync-pull-with-hint = Retry the pull with { $other } instead of { $operation }, just this once (the configuration doesn't change)
sync-rejected-title = Push rejected
sync-rejected-branch = Branch: “{ $branch }”
sync-pull-button = Pull
sync-pull-button-hint = Downloads and integrates the remote's commits, then the push can be retried
sync-fetch-button = Fetch
sync-fetch-button-hint = Downloads the commits that arrived on the remote, to check them
sync-force-button = Force (with lease)…
sync-force-button-hint = Overwrites the branch on the remote with yours (force-with-lease: rejected if the remote has moved since the last fetch). If the remote would lose commits it asks for confirmation and lists them
sync-force-confirm-title = Overwrite the branch on the remote?
sync-force-confirm-title-branch = Force push of “{ $branch }”: overwrite the branch on the remote?
sync-force-confirm-explanation = A force push replaces the branch on the remote with yours: the commits below disappear from the remote, even for those who already fetched them or are working on them. A push can't be undone from here.
sync-force-confirm-backup = Before the push the remote branch as it was is saved in a backup (refs/project/backup/push-…), from which it can be republished by hand. With the lease, if the remote has moved in the meantime the push is rejected.
sync-force-confirm-changed = The remote branch has changed since you saw this confirmation: nothing was published. Check the commits below again.
sync-force-confirm-lost = What is lost on the remote:
sync-force-confirm-button = Force push
sync-force-confirm-gone = The remote-tracking branch no longer exists: close, Fetch and try again
sync-force-confirm-cancel = Don't force
sync-label-push-tag = Push of tag “{ $tag }” to “{ $remote }”
sync-push-tag-new = Tag “{ $tag }” published on “{ $remote }” (at { $id })
sync-push-tag-new-annotated = Annotated tag “{ $tag }” published on “{ $remote }” (at { $id })
sync-push-tag-up-to-date = Tag “{ $tag }” is already on “{ $remote }” with the same value
sync-rejection-tag-exists = The remote already has a tag with this name, on another value: a tag is never overwritten. Nothing changed. To replace it, first “Delete from the remote…” (with its confirmation), then push again.
sync-rejection-short-tag-exists = the remote already has this tag on another value
