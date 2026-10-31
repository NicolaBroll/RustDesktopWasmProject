# Engine errors (`EngineError`): the text of each variant. `Display` is the English text (CLI,
# MCP, tests); the UI shows them in its language. git's text (`$stderr`, `$message`) is kept
# as is.
engine-unknown-command = unknown command: `{ $id }`
engine-invalid-params = invalid parameters for `{ $id }`: { $reason }
engine-repo-not-found = `{ $path }` is not a git repository: { $reason }
engine-no-repo = no repository open: open one with `repo.open`
engine-unknown-rev = unknown revision: `{ $rev }`
engine-git-not-found = cannot run `git`: { $error }
engine-git-too-old = git { $found } is not supported: version { $required } or later is required
engine-git = `git { $command }` failed: { $stderr }
engine-index-locked = the index is locked: `{ $lock }` exists. Another git process is working on the repository; try again when it is done. If no git is running, the file was left behind by a crash and must be removed by hand
engine-not-allowed = operation refused: { $reason }
engine-stale-diff = the diff of `{ $path }` is no longer the one that was read ({ $reason }): nothing changed, reload the diff and select again
engine-nothing-to-commit = nothing to commit: the index matches HEAD. Stage the changes (`index.stage`), or use `amend` to change only the message of the last commit
engine-confirm-required = `{ $command }` needs confirmation: { $reason }. Nothing changed; to proceed, repeat the command with `"confirm": true`
engine-post-commit-id-backup = commit { $id } was created, but { $reason }; the commit rewritten by `amend` is saved in `{ $backup }`: do not repeat the commit, reload status and log
engine-post-commit-id = commit { $id } was created, but { $reason }: do not repeat the commit, reload status and log
engine-post-commit-backup = the commit was created, but { $reason }; the commit rewritten by `amend` is saved in `{ $backup }`: do not repeat the commit, reload status and log
engine-post-commit = the commit was created, but { $reason }: do not repeat the commit, reload status and log
engine-undo-conflict = `{ $command }` refused: the refs changed after the operation ({ $refs }). Nothing changed: so as not to overwrite work done elsewhere, the operation must be undone by hand
engine-checkout-conflict = checkout of `{ $target }` refused: it would overwrite local changes ({ $files }). Nothing changed: commit, stash or discard these changes and try again
engine-auth = authentication with remote `{ $remote }` failed: { $message }. Nothing changed: check the credentials (credential helper, SSH key loaded in the agent, host listed in `known_hosts`)
engine-auth-cancelled = authentication with remote `{ $remote }` cancelled: the credentials were not given. Nothing changed
engine-remote-unreachable = remote `{ $remote }` cannot be reached: { $message }. Nothing changed
engine-cancelled = `{ $command }` cancelled
engine-pull-conflict = pull of `{ $branch }` stopped: the { $operation } with `{ $upstream }` has conflicts in { $files }. The { $operation } was aborted and the branch is as it was (the fetch is done): to integrate `{ $upstream }` resolving the conflicts, run `git { $operation } { $upstream }` from a terminal, or use the other strategy (`strategy`)
engine-push-rejected = push of `{ $branch }` to `{ $remote }` rejected: { $reason }. Nothing changed ({ $message })
# Errors from the layers below the engine (core, domain): their text.
engine-other = { $text }
engine-ignored = files ignored by .gitignore (or by another exclude file), nothing staged: { $files }
engine-unmerged = conflicted files, nothing unstaged (the conflict versions would be lost; to resolve it, stage it): { $files }
engine-unmerged-in-folder = the folder contains conflicted files, nothing staged (staging them would mark them as resolved: they must be named one by one): { $files }
engine-empty-commit-message = the message is empty after git's cleanup (`commit.cleanup` removes comment lines and whitespace), nothing committed: write a message that is not only comments
# Command parameters (`builtin.rs`).
engine-params = { $error }
engine-path-empty = the path is empty: `{ $path }`
engine-path-nul = the path contains a NUL byte: `{ $path }`
engine-path-not-relative = the path must be relative to the repo root, with `/` as separator and without empty, `.` or `..` components: `{ $path }`
engine-discard-confirm = the unstaged changes of { $n ->
    [one] 1 file
   *[other] { $n } files
} would be lost ({ $files }): the worktree would go back to the index. A backup would first be saved in `refs/project/backup/discard-...`, and `edit.undo` would put them back
engine-discard-lines-confirm = { $lost } changed { $lost ->
    [one] line
   *[other] lines
} of `{ $path }` would be lost and { $back } { $back ->
    [one] line
   *[other] lines
} of the index version would come back: the other changes stay. A backup of the file would first be saved in `refs/project/backup/discard-...`, and `edit.undo` would put it back
engine-status-unreadable = the status cannot be read again ({ $error })
engine-lines-empty = `lines` is empty: give at least one changed line of the diff
engine-paths-empty = `paths` is empty: give at least one path
engine-log-stashes-without-all = `stashes` adds the stash bases to the graph of all refs: it goes together with `"all": true`
engine-log-rev-and-all = `rev` and `all` are alternatives: `all` already starts from all branches, tags and HEAD
engine-search-empty = the text to search for is empty
engine-search-line = the text to search for must be on a single line
engine-search-no-fields = `in` is empty: give at least one of "message", "author", "id" (or omit it for all three)
engine-search-content-alone = "content" in `in` cannot be combined with "message", "author" or "id": git cannot AND a content search with the others
engine-search-content-options = `regex` and `ignore_case` apply only to a content search (`"in": ["content"]`)
engine-search-bad-regex = invalid regex: { $stderr }
engine-memory-repos = in-memory repos available: { $repos }
engine-memory-no-repo = no in-memory repo
# Branch names (`git/branch.rs`).
engine-with-name = { $reason }: `{ $name }`
engine-branch-name-empty = the branch name is empty
engine-branch-name-full = use the short branch name (without `refs/heads/`)
engine-branch-name-invalid = not a valid branch name
engine-branch-name-dash = a branch name cannot start with `-`
engine-branch-name-reserved = name reserved by git
engine-tag-name-empty = the tag name is empty
engine-tag-name-full = use the short tag name (without `refs/tags/`)
engine-tag-name-dash = a tag name cannot start with `-`
engine-tag-name-invalid = not a valid tag name
engine-tag-exists = a tag `{ $name }` already exists, nothing changed (a tag is never overwritten)
engine-tag-name-taken-git = the tag name `{ $name }` is already taken, nothing changed (a tag is never overwritten): { $stderr }
engine-tag-unborn-target = the repository has no commits yet: nothing to tag
engine-tag-not-found = there is no tag `{ $name }` here
engine-tag-changed = `{ $full }` changed since it was read (expected { $expected }, now { $now }): nothing deleted
engine-tag-missing = `{ $full }` no longer exists: nothing deleted
engine-tag-delete-confirm = deletes the lightweight tag `{ $name }` (on commit { $target }); the tag is first saved in `refs/project/backup/tag-delete-...` and `edit.undo` puts it back. The remote is not touched. To confirm: "confirm": true
engine-tag-delete-confirm-annotated = deletes the annotated tag `{ $name }` (tag object { $object } on commit { $target }: its message and author exist only there); the tag object is first saved in `refs/project/backup/tag-delete-...` and `edit.undo` puts it back. The remote is not touched. To confirm: "confirm": true
engine-tag-moved = tag `{ $name }` changed since it was seen: it was { $seen }, now it is { $value }. Nothing deleted; reload the tags and try again
engine-tag-expected-hex = `expected` must be the full id ({ $len } hexadecimal digits) of the tag shown in the confirmation: `{ $value }`
# Operation in progress (`check_no_operation`): `$what` is one of the `engine-busy-*`.
engine-operation-in-progress = { $operation ->
    [merge] a merge
    [rebase] a rebase
    [cherry-pick] a cherry-pick
    [revert] a revert
    [bisect] a bisect
    [squash] a squash merge
   *[am] a `git am`
} is in progress: `{ $command }` { $what } in the middle of the operation, nothing changed. Finish or abort the operation and try again
engine-busy-checkout = would switch branch
engine-busy-branch-delete = would delete a branch
engine-busy-branch-rename = would rename a branch
engine-busy-stash-apply = would apply the stash
engine-busy-stash-push = would stash the changes
engine-busy-stash-undo = would change the stashes
engine-busy-restore = would move the refs
engine-busy-reset = would reset the branch
engine-busy-pull = would integrate the upstream
engine-busy-remote-remove = would remove a remote
engine-busy-remote-restore = would put back a remote
engine-busy-remote-rename = would rename a remote
engine-busy-cherry-pick = would start a cherry-pick
engine-busy-revert = would start a revert
engine-busy-worktree = would rewrite files
engine-busy-branch-move = would move the branch
engine-busy-merge = would start another merge
engine-busy-rebase = would start a rebase
# Refs changed from outside (`UndoConflict`, one line per ref).
engine-head-branch = on branch `{ $name }`
engine-head-detached = detached at { $id }
engine-head-changed = HEAD: should have been { $expected }, now it is { $now }
engine-ref-changed = `{ $name }`: should have been { $expected }, now it is { $now }
engine-ref-value = { $id }
engine-ref-missing = missing
engine-discard-index-changed = `{ $path }`: the staged version changed after the discard
engine-restore-head-move = `{ $command }` would have to move HEAD { $head_expected } to { $head_target }: not supported yet, nothing changed
engine-journal-bad-ref = invalid ref name in the journal: `{ $name }`, nothing changed
# Operations in progress, with the article (`operation_name`).
engine-op-merge = a merge
engine-op-rebase = a rebase
engine-op-cherry-pick = a cherry-pick
engine-op-revert = a revert
engine-op-bisect = a bisect
engine-op-am = a `git am`
engine-op-squash = a squash merge
# Opening the repository (`repo.open`).
engine-open-empty-path = empty path
engine-open-bare = it is a bare repository, without a working tree: not supported
engine-open-object-format = object format `{ $format }` not supported (sha1 only)
engine-open-object-format-unknown = unknown object format `{ $format }`
engine-index-flags-not-restored = the index was written correctly, but `{ $flag }` could not be set again on the files that had it (set it again by hand with `git update-index { $flag } <path>`): { $error }
engine-backup-no-name = no free name for the backup after `{ $base }`
engine-git-unexpected-output = unexpected output: `{ $output }`
engine-git-no-commit = no commit read for `{ $hex }`
engine-range-not-commit = `{ $rev }` is not a commit
engine-range-no-base = `{ $from }` and `{ $to }` have no common history: there is no merge base to compare against
engine-command-quoted = `{ $id }`
# An error with a note at the end (what was left changed, where the backup is).
engine-note-newline =
    { $text }
    ({ $note })
engine-note-paren = { $text } ({ $note })
engine-note-semicolon = { $text }; { $note }
engine-git-spawn = { $program }: { $error }
engine-git-exit-status = exited with { $status }
engine-git-version-unknown = unrecognized `git --version` output: `{ $output }`
engine-memory-not-a-commit = fatal: { $id } is not a commit of the repo
engine-memory-unknown-blob = fatal: unknown blob { $oid }
engine-memory-unknown-staged = error: { $target }: unknown staged content
engine-memory-backup-missing = worktree backup { $stash } not found
engine-nothing = nothing
engine-head-moved-after = after `{ $command }` HEAD is no longer on `{ $full }`: a hook or another process changed it; check HEAD before continuing
engine-ref-moved-after = after `{ $command }` `{ $name }` points to { $now } instead of { $target }: another process moved it during the operation
engine-blame-missing-rev = `{ $path }` is not a file in commit `{ $rev }`
engine-blame-missing-worktree = `{ $path }` is not a tracked file of the working tree (missing, a folder or never added): the blame needs a file in HEAD or in the index
engine-blame-no-commits = the branch has no commits yet: there is nothing to blame
engine-blame-binary = `{ $path }` is a binary file: the blame works on text files only
engine-commit-graph-disabled = `core.commitGraph` is `false`: git would not read the commit-graph, nothing written
engine-commit-graph-shallow = Shallow repository: git neither writes nor reads a commit-graph here, nothing written
engine-commit-graph-replaced = Replace refs (refs/replace) or grafts: git neither writes nor reads a commit-graph here, nothing written
engine-busy-upstream = would change the upstream of a branch
