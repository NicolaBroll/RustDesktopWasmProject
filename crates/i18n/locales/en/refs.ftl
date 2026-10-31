# Refs column and ref menu (view/refs.rs).

refs-local-branches = Local branches
refs-tags = Tags
refs-section = { $title } ({ $n })
refs-tooltip-head =
    { $name }
    { $target }
    Click: select the commit
    Right click: more actions
refs-tooltip-local =
    { $name }
    { $target }
    Click: select the commit
    Double click: checkout
    Right click: more actions
refs-tooltip-other =
    { $name }
    { $target }
    Click: select the commit
    Right click: new branch from here, merge into the current branch
refs-tooltip-stash =
    { $name }
    { $target }
    Click: select the commit
refs-checkout = Checkout
refs-checkout-hint = Switches to “{ $name }”: local changes are kept, unless they touch files that differ in the branch (then nothing changes)
refs-checkout-remote-hint = Switches to the local branch that tracks “{ $name }”, creating it with the same name if there is none
refs-set-upstream = Set upstream…
refs-set-upstream-hint = Picks the remote branch that Pull and Push of “{ $name }” use
refs-unset-upstream = Unset upstream
refs-unset-upstream-hint = “{ $name }” stops tracking { $upstream } (the remote branch is not touched). “Undo” restores it
refs-already-active = It is already the current branch
refs-new-branch = New branch from here…
refs-new-branch-hint = Creates a branch at { $id } without switching to it
refs-new-tag = New tag here…
refs-new-tag-hint = Creates a tag at { $id }
refs-rename = Rename…
refs-pull-no-upstream = “{ $name }” has no upstream: Push first, the first push sets it
refs-pull-head-hint = Fetches { $upstream } and integrates it: fast-forward, merge or rebase depending on the configuration
refs-pull-other-hint = Fetches { $upstream } and moves “{ $name }” forward without switching to it (fast-forward only: if it has diverged you need to switch to it)
refs-pull = Pull
refs-push = Push
refs-push-hint = Publishes “{ $name }” to { $upstream }. Never forced
refs-push-first-hint = First push of “{ $name }”: publishes it and sets the upstream
refs-delete = Delete
refs-delete-hint = Deletes the branch. If it is merged it is deleted right away; if it has commits that are nowhere else it asks for confirmation. Either way the commit stays in a backup (refs/project/backup/delete-…) and “Undo” recreates the branch
refs-delete-head = It is the current branch: switch to another branch first to delete it
refs-delete-remote = Delete from the remote…
refs-delete-remote-hint = Deletes the branch “{ $branch }” from the remote “{ $remote }”, for everyone who uses it. Asks for confirmation first; the commit stays in a backup (refs/project/backup/push-delete-…). If someone pushed to it after your last Fetch, the deletion is refused
refs-current-branch = current branch
refs-merge-item = Merge into “{ $into }”
refs-merge-hint = Merges “{ $what }” into “{ $into }”: fast-forward if “{ $into }” has no commits of its own, otherwise a merge commit. With conflicts the merge stays in progress (resolve or abort it from the panel at the top). “Undo” undoes a completed merge
refs-merge-no-ff-item = Merge into “{ $into }” with a merge commit
refs-merge-no-ff-hint = Merges “{ $what }” into “{ $into }” always with a merge commit, even where a fast-forward would do (--no-ff)
refs-filter-hint = Filter branches and tags
refs-filter-clear = Clear the filter (Esc)
refs-none = None
refs-tag-push = Push
refs-tag-no-remote = No remote configured: add one in “Remotes”
refs-tag-push-to = Push to “{ $remote }”
refs-tag-push-hint = Publishes only the tag “{ $name }” to “{ $remote }”. Never forced: if the remote has this tag on another value, nothing changes
refs-tag-delete = Delete…
refs-tag-delete-hint = Deletes the tag “{ $name }” here, after a confirmation; the remotes are not touched. A backup is saved and “Undo” recreates it
refs-tag-delete-remote = Delete from “{ $remote }”…
refs-tag-delete-remote-hint = Deletes the tag “{ $name }” from the remote “{ $remote }”, for everyone who uses it. Reads it from the remote and asks for confirmation first; the value stays in a backup (refs/project/backup/push-delete-…)
