engine-journal-amend = Amend { $id } “{ $summary }” (was { $old })
engine-journal-commit = Commit { $id } “{ $summary }”
engine-journal-discard-one = Discard changes to “{ $path }”
engine-journal-discard-many = Discard changes to { $n } files ({ $names })
engine-journal-clean-one = Delete “{ $path }”
engine-journal-clean-many = Delete { $n } untracked files ({ $names })
engine-journal-quoted = “{ $name }”
engine-journal-branch-create = Create branch `{ $name }` at { $id }
engine-journal-tag-create = Create tag `{ $name }` at { $id }
engine-journal-tag-delete = Delete tag `{ $name }` at { $id }
engine-journal-branch-delete = Delete branch `{ $name }` (was { $id })
engine-journal-branch-rename = Rename branch `{ $from }` to `{ $to }`
engine-journal-head-branch = `{ $name }`
engine-journal-head-detached = { $id } (detached HEAD)
engine-journal-checkout = Checkout of { $after } (from { $before })
engine-journal-pull-ff = fast-forward
engine-journal-pull-rebase = rebase onto { $upstream }
engine-journal-pull-merge = merge of { $upstream }
engine-journal-pull = Pull of `{ $branch }` ({ $how }: from { $before } to { $after })
engine-journal-reset = Reset ({ $mode }) of `{ $branch }` from { $before } to { $after }
engine-journal-merge = Merge of { $source } into `{ $branch }` ({ $strategy ->
    [fast_forward] fast-forward
   *[other] merge commit
}: from { $before } to { $after })
engine-journal-pick = { $kind ->
    [revert] Revert
   *[other] Cherry-pick
} of { $source } onto `{ $branch }` (from { $before } to { $after })
engine-journal-rebase = Rebase of `{ $branch }` ({ $n ->
    [one] 1 commit
   *[other] { $n } commits
}, from { $before } to { $after })
engine-journal-remote-add = Add remote `{ $name }`
engine-journal-remote-add-fetched = Add remote `{ $name }` (with fetch: { $n ->
    [one] 1 remote branch
   *[other] { $n } remote branches
})
engine-journal-remote-remove = Remove remote `{ $name }` ({ $n ->
    [one] 1 remote branch
   *[other] { $n } remote branches
}{ $upstreams ->
    [0] {""}
    [one] , upstream of 1 branch
   *[other] , upstream of { $upstreams } branches
})
engine-journal-stash-push = Stash changes (stash { $id } “{ $message }”)
engine-journal-stash-drop = Drop { $name } ({ $id } “{ $message }”)
engine-journal-branch-create-track = Create branch `{ $name }` at { $id } tracking `{ $upstream }`
engine-journal-upstream-set = Set the upstream of `{ $name }` to `{ $upstream }`
engine-journal-upstream-unset = Remove the upstream `{ $previous }` of `{ $name }`
