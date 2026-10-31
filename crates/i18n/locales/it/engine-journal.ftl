engine-journal-amend = Amend { $id } «{ $summary }» (era { $old })
engine-journal-commit = Commit { $id } «{ $summary }»
engine-journal-discard-one = Scarta le modifiche di «{ $path }»
engine-journal-discard-many = Scarta le modifiche di { $n } file ({ $names })
engine-journal-quoted = «{ $name }»
engine-journal-branch-create = Crea branch `{ $name }` su { $id }
engine-journal-tag-create = Crea tag `{ $name }` su { $id }
engine-journal-tag-delete = Elimina tag `{ $name }` su { $id }
engine-journal-branch-delete = Elimina branch `{ $name }` (era { $id })
engine-journal-branch-rename = Rinomina branch `{ $from }` in `{ $to }`
engine-journal-head-branch = `{ $name }`
engine-journal-head-detached = { $id } (HEAD staccata)
engine-journal-checkout = Checkout di { $after } (da { $before })
engine-journal-pull-ff = fast-forward
engine-journal-pull-rebase = rebase su { $upstream }
engine-journal-pull-merge = merge di { $upstream }
engine-journal-pull = Pull di `{ $branch }` ({ $how }: da { $before } a { $after })
engine-journal-reset = Reset ({ $mode }) di `{ $branch }` da { $before } a { $after }
engine-journal-merge = Merge di { $source } in `{ $branch }` ({ $strategy ->
    [fast_forward] fast-forward
   *[other] commit di merge
}: da { $before } a { $after })
engine-journal-pick = { $kind ->
    [revert] Revert
   *[other] Cherry-pick
} di { $source } su `{ $branch }` (da { $before } a { $after })
engine-journal-rebase = Rebase di `{ $branch }` ({ $n ->
    [one] 1 commit
   *[other] { $n } commit
}, da { $before } a { $after })
engine-journal-remote-add = Aggiungi remoto `{ $name }`
engine-journal-remote-add-fetched = Aggiungi remoto `{ $name }` (con fetch: { $n } branch remoti)
engine-journal-remote-remove = Rimuovi remoto `{ $name }` ({ $n } branch remoti{ $upstreams ->
    [0] {""}
   *[other] , upstream di { $upstreams } branch
})
engine-journal-stash-push = Metti da parte le modifiche (stash { $id } «{ $message }»)
engine-journal-stash-drop = Elimina { $name } ({ $id } «{ $message }»)
engine-journal-branch-create-track = Crea branch `{ $name }` su { $id } che segue `{ $upstream }`
engine-journal-upstream-set = Imposta l'upstream di `{ $name }` a `{ $upstream }`
engine-journal-upstream-unset = Togli l'upstream `{ $previous }` di `{ $name }`
