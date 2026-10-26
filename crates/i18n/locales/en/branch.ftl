# Local branches: create, checkout, delete, rename (branch.rs, view/branches.rs).

branch-start-commit = commit { $id }
branch-create-label = Creating branch “{ $name }”
branch-tag-label = Creating tag “{ $name }”
branch-tag-created = Tag “{ $tag }” created at { $id }. “Undo” deletes it
branch-tag-created-annotated = Annotated tag “{ $tag }” created at { $id }. “Undo” deletes it
branch-tag-title = New tag
branch-tag-button = Create tag
branch-tag-text = The tag points to { $start } ({ $id }). With a message it is annotated (author, date and message), without it is lightweight. It is not pushed to the remote.
branch-tag-name-hint = e.g. v1.0.0
branch-tag-message-hint = Message (optional: makes an annotated tag)
branch-rename-label = Renaming “{ $name }”
branch-checkout-label = Checking out “{ $name }”
branch-delete-label = Deleting “{ $name }”
branch-checkout-refused = Cannot check out “{ $name }”: { $error }
branch-checkout-failed = Checkout of “{ $name }” failed: { $error }
branch-delete-refused = Cannot delete “{ $name }”: { $error }
branch-delete-failed = Deleting “{ $name }” failed: { $error }
branch-created = Branch “{ $branch }” created at { $id }. “Undo” deletes it
branch-checked-out = Switched to branch “{ $branch }”. “Undo” goes back to the previous one
branch-already-active = “{ $branch }” is already the current branch
branch-checkout-status-error = Checkout done, but the status could not be reloaded: { $error }
branch-deleted = Branch “{ $branch }” deleted (was at { $id }{ $unmerged ->
        [0] {""}
        [one] , with 1 unmerged commit
       *[other] , with { $unmerged } unmerged commits
    }); backup in { $backup }. “Undo” recreates it
branch-deleted-upstream = Branch “{ $branch }” deleted (was at { $id }{ $unmerged ->
        [0] {""}
        [one] , with 1 unmerged commit
       *[other] , with { $unmerged } unmerged commits
    }); backup in { $backup }; its upstream setting was removed too. “Undo” recreates it with its configuration
branch-deleted-config = Branch “{ $branch }” deleted (was at { $id }{ $unmerged ->
        [0] {""}
        [one] , with 1 unmerged commit
       *[other] , with { $unmerged } unmerged commits
    }); backup in { $backup }; the configuration { $keys } was removed too. “Undo” recreates it with its configuration
branch-renamed = Branch “{ $previous }” renamed to “{ $branch }”. “Undo” restores the previous name
branch-refs-warning = Done, but the refs could not be reloaded: { $error }

# Dialogs.
branch-new-title = New branch
branch-create-button = Create branch
branch-rename-title = Rename branch
branch-rename-button = Rename
branch-create-text = The branch starts at { $start } ({ $id }). It is created without switching to it: the current branch does not change.
branch-rename-text = New name for “{ $name }”. Reflog and upstream follow the branch.
branch-name-hint = e.g. feature/new
branch-close = Close
branch-delete-heading = Delete “{ $name }” ({ $id })?
branch-delete-unmerged = The branch is not merged: it has commits that are not in the current branch (nor in its upstream). After deleting it, those commits are no longer reachable from any branch.
branch-reason = Reason:
branch-delete-backup = Before deleting, a backup of the commit is saved (refs/project/backup/delete-…): with “Undo” in the top bar the branch comes back at the same commit, with its configuration (upstream included), as long as the app stays open: the operation journal is kept in memory. After closing, only the commit backup in refs/project/backup/ remains, from which the branch can be recreated by hand.
branch-delete-button = Delete branch
branch-dont-delete = Don't delete
branch-refused-heading = Cannot check out “{ $name }”
branch-refused-text = The checkout would overwrite local changes: these paths are modified (staged or not) or untracked (also inside a folder that is a file in the branch, or in a submodule), and they differ in the target branch. Nothing was changed. Commit, discard or stash these changes and try again.
branch-refused-files = { $n ->
        [0] Git did not list the files.
        [one] File:
       *[other] { $n } files:
    }
branch-tag-delete-label = Deleting tag “{ $name }”
branch-tag-delete-refused = Cannot delete tag “{ $name }”: { $error }
branch-tag-delete-failed = Deleting tag “{ $name }” failed: { $error }
branch-tag-deleted = Tag “{ $tag }” deleted (was at { $id }); backup in { $backup }. “Undo” recreates it
branch-tag-deleted-annotated = Annotated tag “{ $tag }” deleted (was at { $id }); the tag object is in { $backup }. “Undo” recreates it
branch-tag-delete-heading = Delete tag “{ $name }”?
branch-tag-delete-text = The tag is deleted only here: the remotes keep their copy (use “Delete from the remote…” for that).
branch-tag-delete-backup = Before deleting, the tag is saved in a backup (refs/project/backup/tag-delete-…), with the message and author of an annotated tag: “Undo” in the top bar recreates it on the same value, as long as the app stays open.
branch-tag-delete-button = Delete tag
