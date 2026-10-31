# Submodules: the section of the refs column, its menu, the update and the clone checkbox
# (submodule.rs, view/submodule_view.rs, view/clone_view.rs).
# $path: the submodule's path in the repository; $id: a short commit id.

submodule-section = Submodules ({ $n })
submodule-list-error = Submodules not read: { $error }
submodule-update-all-hint = Update all: check out in every submodule the commit recorded in this repository (the ones not checked out yet are cloned first). Submodules with changes or with commits on no branch are left as they are.
submodule-menu-hint = Update, open in a tab, show the folder

submodule-state-conflict = conflict
submodule-state-uninitialized = not checked out
submodule-state-local-changes = changes
submodule-state-different-commit = other commit
submodule-state-in-sync = up to date

submodule-tooltip-path = Submodule “{ $path }”
submodule-tooltip-url = URL: { $url }
submodule-tooltip-no-url = URL: none
submodule-tooltip-recorded = Recorded in this repository: { $id }
submodule-tooltip-recorded-conflict = Recorded in this repository: conflicted
submodule-tooltip-head-detached = Checked out: { $id } (detached HEAD)
submodule-tooltip-head-branch = Checked out: { $id } (on a branch)
submodule-tooltip-not-checked-out = Checked out: nothing yet
submodule-tooltip-changes = It has changes to tracked files
submodule-tooltip-untracked = It has untracked files
submodule-tooltip-operation = { $operation } in progress inside it
submodule-tooltip-unreferenced = The checked out commit has commits on no branch, tag or remote branch: create a branch there first, otherwise an update would leave them behind
submodule-tooltip-ignored = Ignored files are where the recorded commit adds files: an update would overwrite them, move them first
submodule-tooltip-no-mapping = Not in .gitmodules: git cannot update it

submodule-explain-conflict = Conflicted in this repository: resolve the conflict first
submodule-explain-uninitialized = Not checked out yet: “Initialize and update” clones it
submodule-explain-local-changes = Changes inside the submodule: commit or stash them there before updating
submodule-explain-different-commit = The checked out commit is not the one recorded here: “Update” checks out the recorded one
submodule-explain-in-sync = Already on the recorded commit
submodule-blocked-operation = An operation is in progress inside the submodule: finish or abort it first

submodule-update = Update
submodule-update-hint = Check out the commit recorded in this repository (detached HEAD); the previous one is saved in a backup ref inside the submodule
submodule-init = Initialize and update
submodule-init-hint = Clone the submodule and check out the commit recorded in this repository
submodule-open-tab = Open in a new tab
submodule-open-tab-hint = Open the submodule as a repository in its own tab
submodule-show-folder = Show in file manager
submodule-show-folder-hint = Show the submodule's folder in the system's file manager
submodule-not-checked-out = Not checked out yet

submodule-update-label = Update of submodule { $path }
submodule-init-label = Initialization of submodule { $path }
submodule-update-all-label = Update of the submodules
submodule-stop-hint = Stop the update: submodules already checked out stay as they are
submodule-update-cancelled = Cancelled: { $label }
submodule-update-done = Submodules: { $updated } updated, { $unchanged } already up to date
submodule-update-refused = { $n ->
    [one] 1 not updated
   *[other] { $n } not updated
}: { $list }

submodule-clone = With submodules
submodule-clone-hint = Also clone the submodules and check out the commits recorded (git clone --recurse-submodules)
