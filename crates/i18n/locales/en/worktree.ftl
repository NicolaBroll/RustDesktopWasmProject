# Linked worktrees: the section of the refs column, its menu, the "New worktree" dialog and the
# removal (worktree.rs, view/worktree_view.rs, view/refs.rs).
# $path: the folder of a worktree; $branch, $name: a branch name; $id: a short commit id.

worktree-section = Worktrees ({ $n })
worktree-list-error = Worktrees not read: { $error }
worktree-new-hint = New worktree: another folder with a checkout of this repository, on another branch
worktree-menu-hint = Open in a new tab, show the folder, remove
worktree-new-from-branch = New worktree from this branch…
worktree-new-from-branch-hint = Check out “{ $name }” in another folder, to work on it without leaving the branch open here (a new branch from it if it is already checked out)

worktree-badge-current = here
worktree-badge-locked = locked
worktree-badge-prunable = missing
worktree-bare = bare repository
worktree-detached = detached at { $id }
worktree-no-commit = no commits yet
worktree-tooltip-main = Main worktree
worktree-tooltip-current = Open in this tab
worktree-tooltip-locked = Locked (git worktree lock): it cannot be removed from here
worktree-tooltip-locked-reason = Locked ({ $reason }): it cannot be removed from here
worktree-tooltip-prunable = Its folder no longer exists: only its entry is left
worktree-bare-hint = The bare repository has no files to open

worktree-open-tab = Open in a new tab
worktree-open-tab-hint = Opens this worktree in a tab of its own
worktree-open-current = Open in a new tab
worktree-open-current-hint = Already open in this tab
worktree-show-folder = Show in file manager
worktree-show-folder-hint = Shows the folder of this worktree in the system's file manager
worktree-remove = Remove…
worktree-remove-hint = Deletes the folder of this worktree, after a confirmation; refused if it has changes, untracked or ignored files
worktree-remove-main = The main worktree is never removed
worktree-remove-current = This worktree is open here: open another one to remove it

worktree-add-title = New worktree
worktree-add-explanation = A worktree is another folder with a checkout of this repository: commits, branches and stashes are shared, the files and the current branch are its own. It opens in a new tab.
worktree-add-path = Folder
worktree-add-path-hint = /path/to/new/folder
worktree-add-browse-hint = Choose the folder of the new worktree (absent or empty)
worktree-add-checkout = Check out
worktree-mode-existing = Existing branch
worktree-mode-existing-hint = A local branch that is not checked out in any worktree
worktree-mode-new = New branch
worktree-mode-new-hint = A new local branch, created at the starting point
worktree-mode-detached = Detached commit
worktree-mode-detached-hint = The starting point on a detached HEAD, without a branch
worktree-add-branch = Branch
worktree-add-branch-pick = Choose a branch
worktree-add-branch-hint = Only the branches not checked out in a worktree
worktree-add-no-free-branch = Every local branch is checked out in a worktree
worktree-add-new-branch = Name
worktree-add-new-branch-hint = name of the new branch
worktree-add-start = Starting point
worktree-add-start-hint = HEAD (or a branch, tag, commit)
worktree-add-rule = The folder must not exist or must be empty; a relative path is relative to the folder of this repository.
worktree-add-submit = Create
worktree-add-running = Creating the worktree…
worktree-add-need-path = Choose the folder
worktree-add-need-branch = Choose the branch
worktree-add-need-name = Write the name of the new branch
worktree-add-label = New worktree
worktree-added-branch = Worktree “{ $path }” created on { $branch }
worktree-added-new-branch = Worktree “{ $path }” created on the new branch { $branch }
worktree-added-detached = Worktree “{ $path }” created, detached at { $id }

worktree-remove-check-label = Checking worktree { $path }
worktree-remove-label = Removing worktree { $path }
worktree-remove-title = Remove this worktree?
worktree-remove-body = Its folder is deleted and git forgets the worktree. Its branch stays in the repository; it can be checked out again with a new worktree.
worktree-remove-button = Remove worktree
worktree-remove-refused-title = The worktree cannot be removed
worktree-close = Close
worktree-removed = Worktree “{ $path }” removed
worktree-removed-backup = Worktree “{ $path }” removed; its commits on no branch are saved in { $backup }
