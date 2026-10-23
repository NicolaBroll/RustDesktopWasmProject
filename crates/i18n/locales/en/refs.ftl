# Refs column and ref menu (view/refs.rs).

refs-local-branches = Local branches
refs-remote-branches = Remote branches
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
refs-already-active = It is already the current branch
refs-new-branch = New branch from here…
refs-new-branch-hint = Creates a branch at { $id } without switching to it
refs-rename = Rename…
refs-pull-no-upstream = “{ $name }” has no upstream: Push first, the first push sets it
refs-pull-head-hint = Fetches { $upstream } and integrates it: fast-forward, merge or rebase depending on the configuration
refs-pull-other-hint = Fetches { $upstream } and moves “{ $name }” forward without switching to it (fast-forward only: if it has diverged you need to switch to it)
refs-pull = Pull
refs-pull-ff = Pull (fast-forward)
refs-push = Push
refs-push-hint = Publishes “{ $name }” to { $upstream }. Never forced
refs-push-first-hint = First push of “{ $name }”: publishes it and sets the upstream
refs-delete = Delete
refs-delete-hint = Deletes the branch. If it is merged it is deleted right away; if it has commits that are nowhere else it asks for confirmation. Either way the commit stays in a backup (refs/project/backup/delete-…) and “Undo” recreates the branch
refs-delete-head = It is the current branch: switch to another branch first to delete it
refs-current-branch = current branch
refs-merge-item = Merge into “{ $into }”
refs-merge-hint = Merges “{ $what }” into “{ $into }”: fast-forward if “{ $into }” has no commits of its own, otherwise a merge commit. With conflicts the merge stays in progress (resolve or abort it from the panel at the top). “Undo” undoes a completed merge
refs-merge-no-ff-item = Merge into “{ $into }” with a merge commit
refs-merge-no-ff-hint = Merges “{ $what }” into “{ $into }” always with a merge commit, even where a fast-forward would do (--no-ff)
