# Staging area and commit message box (stage.rs, view/stage_view.rs).

stage-label-stage-one = Staging { $path }
stage-label-stage-many = Staging { $n ->
    [one] 1 file
   *[other] { $n } files
}
stage-label-unstage-one = Unstaging { $path }
stage-label-unstage-many = Unstaging { $n ->
    [one] 1 file
   *[other] { $n } files
}
stage-heading = Uncommitted changes
stage-stash-button = Stash…
stage-writing-fallback = Writing
stage-in-progress = { $label } in progress…
stage-hide = Hide
stage-commit-message = Commit message
stage-subject-too-long = The subject is longer than { $limit } characters: the log and many tools cut it. The rest can go in the body.
stage-subject-count-hint = Subject characters
stage-subject-hint = Subject (one short line)
stage-body-hint = Description (optional)
stage-amend-checkbox = Amend (change the last commit)
stage-amend-help = Replaces the last commit with a new one: same author and parents, the message written here and the staged changes added. The replaced commit stays saved in a backup ref.
stage-amend-warning-summary = The last commit { $id } “{ $summary }” will change: a new commit will replace it.
stage-amend-warning-id = The last commit { $id } will change: a new commit will replace it.
stage-amend-warning = The last commit will change: a new commit will replace it.
stage-amend-proposed = The message is the current one: edit it if needed.
stage-commit-amend-button = Commit (amend)
stage-commit-button = Commit
stage-commit-help = Create the commit (Ctrl+Enter)
stage-commit-shortcut = Ctrl+Enter
stage-copy-output = Copy output
stage-copy-output-help = Copy all of git's output to the clipboard
stage-staged-title = Staged
stage-unstage-all = Unstage all
stage-unstage-all-help = Unstages all the files in this list; the working tree doesn't change
stage-unstaged-title = Unstaged
stage-stage-all = Stage all
stage-stage-all-help = Stages all the files in this list
stage-section-title = { $title } ({ $n })
stage-discard-all = Discard all…
stage-discard-all-help = Discards the unstaged changes of all the tracked files in this list (asks for confirmation, can be undone). Untracked files are not touched
stage-more-hint = More actions on this list
stage-empty-staged = No staged files.
stage-empty-unstaged = No unstaged changes.
stage-stage = Stage
stage-unstage = Unstage
stage-resolve-in-editor = Resolve in editor…
stage-show-diff = Show diff
stage-mark-resolved = Mark as resolved…
stage-discard-file = Discard changes…
stage-discard-file-help = Restores the file to the staged version (or the last commit's): asks for confirmation, can be undone
stage-wait = Wait for the operation in progress
stage-conflict-both-deleted = deleted by both
stage-conflict-added-by-us = added by us
stage-conflict-deleted-by-them = deleted by them
stage-conflict-added-by-them = added by them
stage-conflict-deleted-by-us = deleted by us
stage-conflict-both-added = added by both
stage-conflict-both-modified = modified by both
stage-row-mark-resolved-help = Mark the conflict as resolved (asks for confirmation)
stage-row-stage-help = Stage this file
stage-row-unstage-help = Unstage this file; the working tree doesn't change
stage-tooltip-conflict = { $path } (conflicted: { $conflict })
stage-tooltip-click = Click: show the diff
stage-tooltip-double-resolve = Double-click: mark the conflict as resolved (asks for confirmation)
stage-tooltip-double-stage = Double-click: stage
stage-tooltip-double-unstage = Double-click: unstage
stage-tooltip-right-discard = Right-click: more actions, including discarding the changes
stage-tooltip-right = Right-click: more actions
stage-blame-untracked = The file is not tracked yet: the blame needs a file in HEAD or in the index
stage-blame-deleted = The file is deleted: there is nothing to blame
stage-delete-file = Delete…
stage-delete-file-help = Deletes this untracked file (asks for confirmation; a backup is kept and “Undo” puts it back)
stage-ignore = Ignore
stage-ignore-help = Adds a line to .gitignore or .git/info/exclude so git stops showing this file (or all files like it). “Undo” removes it
stage-ignore-shared = In .gitignore (shared, committed)
stage-ignore-local = Only for me (.git/info/exclude)
stage-delete-untracked = { $n ->
    [one] Delete the untracked file…
   *[other] Delete all { $n } untracked files…
}
stage-delete-untracked-help = Deletes the untracked files of this list (asks for confirmation; a backup is kept and “Undo” puts them back). Tracked files are not touched
stage-tooltip-right-delete = Right-click: more actions, including deleting the file
