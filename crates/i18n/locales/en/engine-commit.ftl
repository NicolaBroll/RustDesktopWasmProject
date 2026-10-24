engine-commit-amend-published = `amend` would rewrite a commit that is already published or tagged (reachable from { $refs }): whoever already fetched it would see a different history, the push would have to be forced and the tags would stay on the old commit. The rewritten commit would remain in a backup ref
engine-commit-amend-unborn = `amend` without a commit to rewrite: the branch has no commits yet, nothing committed
engine-commit-amend-merge = a merge is in progress: the last commit cannot be amended, nothing committed. Committing without `amend` concludes the merge
engine-commit-conflicts = there are conflicted files, nothing committed: resolve them and stage them (`index.stage`) before committing: { $files }
engine-commit-message-empty = the message is empty: write at least the commit subject
engine-commit-message-nul = the message contains a NUL byte
engine-commit-operation = { $operation ->
    [rebase] a rebase
    [cherry-pick] a cherry-pick
    [am] a `git am`
   *[revert] a revert
} is in progress: a commit from here would change it in a way that is hard to see, nothing committed. Finish or abort the operation (also from a terminal) and try again
engine-commit-amend-empty = `amend` would make the last commit empty (same content as its parent), nothing committed
engine-commit-memory-merge-head = the in-memory repo has no `MERGE_HEAD` for this merge (set with `set_operation`): the merge commit is not supported, nothing committed
engine-commit-head-unreadable = HEAD cannot be read again ({ $error })
engine-commit-head-no-commit = HEAD does not point to a commit
engine-commit-unreadable = the commit cannot be read ({ $error })
engine-commit-rewritten-unreadable = the rewritten commit cannot be read ({ $error })
engine-commit-reflog-unreadable = the reflog of `{ $name }` cannot be read ({ $error })
engine-commit-head-moved = HEAD ({ $head }) is not the commit just created on top of { $previous }: another git process or a hook (e.g. `post-commit`) moved HEAD before or after the commit
engine-commit-head-moved-unborn = HEAD ({ $head }) is not the commit just created on an unborn branch: another git process or a hook (e.g. `post-commit`) moved HEAD before or after the commit
engine-commit-silent-failure = exited with { $status } without messages (a hook that refuses without writing anything?)
engine-commit-backup-kept = HEAD is no longer { $old }: the commit to be rewritten is saved in `{ $name }`
engine-commit-backup-note =
    { $text }
    
    { $note }
