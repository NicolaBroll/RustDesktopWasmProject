# Interactive rebase editor (rebase.rs, view/rebase_view.rs).
# The action names (pick, reword, squash, fixup, drop) stay in the code, as in git.

rebase-menu-item = Interactive rebase from here…
rebase-menu-hint = Edit the plan of the commits from { $id } up to HEAD of “{ $branch }”: reorder, combine, drop or reword them, with a preview before running
rebase-menu-root = { $id } is the first commit: it has no parent to rebase onto
rebase-menu-not-on-branch = Only the commits of the current branch’s history (following first parents from HEAD) can be rebased
rebase-heading = Interactive rebase of “{ $branch }”
rebase-heading-loading = Interactive rebase
rebase-loading = Reading the commits…
rebase-intro = Commits from { $what } up to HEAD, oldest first as in git: drag them by the handle to reorder them; “squash” and “fixup” join a commit into the one above it.
rebase-steps = Plan
rebase-result = Result
rebase-result-empty = The plan is not valid: no preview
rebase-published = published
rebase-published-hint = Already on a remote branch: rewriting it needs a confirmation
rebase-drag-hint = Drag to move this commit
rebase-up-hint = Move up
rebase-down-hint = Move down
rebase-action-pick-hint = Keep the commit as it is
rebase-action-reword-hint = Keep the commit with a new message
rebase-action-squash-hint = Join into the commit above, combining the messages
rebase-action-fixup-hint = Join into the commit above, keeping the message of the commit above
rebase-action-drop-hint = Remove the commit
rebase-message-reword = New message of { $id }:
rebase-message-group = Message of the combined commit (empty: { $default }):
rebase-message-default-squash = the messages joined
rebase-message-default-fixup = the message of the first commit
rebase-message-none = Choose “reword”, or the last “squash”/“fixup” of a group, to write a message
rebase-source-combined = messages combined
rebase-source-new = new message
rebase-from = from { $ids }
rebase-noop = The plan changes nothing: change an action or the order first
rebase-run = Run rebase
rebase-run-hint = Rewrites “{ $branch }” with this plan; the branch as it was is kept in a backup and “Undo” puts it back
rebase-close = Close
rebase-label = Interactive rebase of “{ $branch }”
rebase-refused = Not possible: { $error }
rebase-failed = Rebase of “{ $branch }” failed: { $error }. The repository is reloaded to show its current state
rebase-stopped = Rebase of “{ $branch }” stopped: { $files }. Resolve them and mark them as resolved in the staging area, then “Continue rebase…” in the panel at the top (or “Abort rebase…”)
rebase-stopped-before-commit = Rebase of “{ $branch }” stopped before a commit (a hook or signing?): finish it with “Continue rebase…” in the panel at the top, or abort it
rebase-stopped-short = Rebase stopped
rebase-done = Rebase of “{ $branch }” done: { $before ->
        [one] 1 commit
       *[other] { $before } commits
    } became { $after ->
        [one] 1
       *[other] { $after }
    }, the branch is at { $id }; the branch as it was is in { $backup }. “Undo” puts it back
rebase-confirm-heading = Rewrite published commits?
rebase-confirm-text = These commits are already on a remote branch. After the rebase the branch differs from the remote one: pushing it will need a force push, and whoever has the old commits will have to realign.
rebase-confirm-button = Rewrite anyway
rebase-confirm-back = Back to the plan
rebase-error-unknown = Commit { $id } is not in the range being rebased
rebase-error-duplicate = Commit { $id } appears more than once
rebase-error-missing = Commit { $id } is missing from the plan
rebase-error-all-dropped = Every commit is dropped: keep at least one
rebase-error-nothing-to-combine = Step { $n }: the first commit kept cannot be “squash” or “fixup”, there is nothing above it to join it into
rebase-error-missing-message = Step { $n }: “reword” needs a message
rebase-error-unexpected-message = Step { $n }: a message is allowed only on “reword” and on the last “squash”/“fixup” of a group
rebase-error-reword-in-group = Step { $n }: a “reword” cannot be followed by “squash”/“fixup”; use “pick” and write the message on the last step of the group
rebase-error-empty-message = Step { $n }: the message is empty
rebase-error-nul = Step { $n }: the message contains a NUL character
