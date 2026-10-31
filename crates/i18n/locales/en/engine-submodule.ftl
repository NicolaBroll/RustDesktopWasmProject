# Submodules (`submodule.list`, `submodule.update`, impl-docs/git/submodules.md).

engine-submodule-paths-empty = `paths` must list at least one submodule, without empty paths
engine-submodule-unknown = `{ $path }` is neither a submodule nor a folder containing one, nothing updated
engine-submodule-not-initialized = `{ $path }` is not initialized: update it with "init"
engine-submodule-no-mapping = `{ $path }` has no entry in `.gitmodules`: git cannot update it
engine-submodule-suspicious-url = the URL of `{ $path }` starts with `-`: it is not used
engine-submodule-conflict = `{ $path }` is conflicted in this repository: resolve the conflict first
engine-submodule-operation = an operation is in progress inside `{ $path }`: finish or abort it first
engine-submodule-local-changes = `{ $path }` has changes: commit or stash them inside the submodule first
engine-submodule-untracked = `{ $path }` has untracked files: commit, move or remove them first
engine-submodule-unreferenced = the checked out commit of `{ $path }` has commits on no branch, tag or remote branch: create a branch there first, otherwise the update would leave them behind
engine-submodule-not-at-recorded = after the update `{ $path }` is on { $head }, not on the recorded commit { $recorded }
engine-submodule-update-failed = { $error } (the previous HEADs of the submodules are saved in: { $backups })
