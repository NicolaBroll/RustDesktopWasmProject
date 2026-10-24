engine-stash-nothing-untracked = niente da mettere da parte: nessuna modifica né file non tracciato
engine-stash-nothing-untracked-paths = niente da mettere da parte: nessuna modifica né file non tracciato fra i percorsi scelti
engine-stash-nothing = niente da mettere da parte: nessuna modifica ai file tracciati (i non tracciati entrano solo con `include_untracked`)
engine-stash-nothing-paths = niente da mettere da parte: nessuna modifica ai file tracciati (i non tracciati entrano solo con `include_untracked`) fra i percorsi scelti
engine-stash-blocked = `{ $command }` di `{ $stash }` sovrascriverebbe file con modifiche locali, non tracciati o ignorati ({ $files }): niente modificato. Committarli, metterli da parte o spostarli e riprovare
engine-stash-apply-conflicts = ci sono file in conflitto ({ $files }): risolverli prima di applicare uno stash, niente modificato
engine-stash-intent-to-add = ci sono file `intent-to-add` (`git add -N`: { $files }): git non riesce a salvarli in uno stash. Metterli in stage o toglierli dallo stage e riprovare, niente modificato
engine-stash-push-conflicts = ci sono file in conflitto ({ $files }): risolverli prima di mettere da parte le modifiche, niente modificato
engine-stash-unborn = il branch non ha ancora commit: uno stash si appoggia a HEAD, niente messo da parte
engine-stash-message-control = il messaggio dello stash è una riga sola, senza a capo né caratteri di controllo
engine-stash-memory-unborn = il branch non ha ancora commit
engine-stash-gone = `{ $command }`: lo stash { $id } non è più nell'elenco, niente modificato
engine-stash-none = non ci sono stash, niente modificato
engine-stash-no-index = non c'è `stash@{"{"}{ $index }{"}"}`: gli stash sono { $n } (da `stash@{"{"}0{"}"}` a `stash@{"{"}{ $last }{"}"}`), niente modificato
engine-stash-now-at = ora è `{ $name }`
engine-stash-now-gone = non è più nell'elenco
engine-stash-list-changed = l'elenco degli stash è cambiato da quando è stato visto: `stash@{"{"}{ $index }{"}"}` è { $id }, lo stash { $seen } { $now }. Niente modificato; rileggere gli stash (`stash.list`) e riprovare
# `$files`: the tracked files, -1 if unknown; `$untracked`: `yes` or `no`.
engine-stash-drop-confirm = lo stash `{ $name }` ({ $id } «{ $message }»{ $files ->
    [-1] {""}
   *[other] , { $files } file tracciati
}{ $untracked ->
    [yes] , con file non tracciati
   *[no] {""}
}) verrebbe eliminato. Prima si salverebbe il suo commit in `refs/project/backup/stash-...`, e `edit.undo` lo rimetterebbe in cima all'elenco. Per eliminarlo ripetere con `"expected": "{ $id }"`
engine-stash-confirm-without-expected = `confirm` va insieme a `expected`, l'id completo dello stash da eliminare (quello dato dalla conferma o da `stash.list`): la conferma vale solo per lo stash visto
engine-stash-expected-hex = `expected` deve essere l'id completo ({ $len } cifre esadecimali) dello stash: `{ $value }`
# The key is `INDEX_NOT_RESTORED` (`git/stash.rs`): the undo of `stash.push` recognizes it.
engine-stash-index-not-restored = con `restore_index` le modifiche in stage di `{ $name }` non si applicano sopra l'indice attuale: niente modificato; riprovare senza `restore_index` (tutto torna fuori stage)
engine-stash-drop-list-changed = l'elenco degli stash è cambiato mentre si preparava `{ $command }` (un altro processo?): lo stash non è stato eliminato, rileggere gli stash e riprovare
engine-stash-drop-gone = `{ $command }`: lo stash { $id } non è più nell'elenco, niente eliminato
engine-stash-apply-refused = `{ $command }` di `{ $name }` rifiutato da git: sovrascriverebbe modifiche locali a { $files }. Niente modificato; committarle, metterle da parte o scartarle e riprovare
engine-stash-apply-refused-partly = `{ $command }` di `{ $name }` rifiutato da git: sovrascriverebbe modifiche locali a { $files }. I file tracciati non sono stati toccati, ma git potrebbe aver già rimesso i non tracciati dello stash (l'apply può essere rimasto a metà; sono anche nello stash, che resta nell'elenco)
engine-stash-apply-untracked-exist = `{ $command }` di `{ $name }` è rimasto a metà: git ha già applicato le modifiche ai file tracciati, ma non ha rimesso i non tracciati perché ci sono già ({ $files }). Lo stash resta nell'elenco; controllare il worktree prima di riprovare
engine-stash-apply-assume-unchanged = file segnati `--assume-unchanged` che lo stash cambia ({ $files }): git ne sovrascriverebbe le modifiche senza controllarle. Niente modificato; togliere il segno (`git update-index --no-assume-unchanged`) e riprovare
engine-stash-not-created = niente da mettere da parte: git non ha creato lo stash ({ $output }), niente modificato
engine-stash-push-assume-unchanged = file segnati `--assume-unchanged` ({ $files }): git non ne metterebbe da parte le modifiche e poi le cancellerebbe riportando il worktree a HEAD. Niente modificato; togliere il segno (`git update-index --no-assume-unchanged`) o escluderli con `paths`, e riprovare
engine-stash-store-not-on-top = dopo `git stash store` lo stash { $id } non è in cima all'elenco: rileggere gli stash
engine-stash-ls-tree-line = riga non riconosciuta: `{ $line }`
engine-stash-not-on-top = lo stash { $id } è stato creato, ma non è in cima all'elenco (un altro processo?): rileggere gli stash
engine-stash-list-unreadable = elenco degli stash non riconosciuto: `{ $output }`
engine-stash-push-stopped = lo stash { $id } è stato creato, ma poi git si è fermato ({ $stderr }): le modifiche sono nello stash; controllare il worktree prima di riprovare
engine-stash-not-dropped = lo stash `{ $name }` è stato applicato ma non tolto dall'elenco ({ $error }): le modifiche sono nei file; eliminarlo con `stash.drop` (non ripetere il pop)
engine-stash-kept-conflicts = lo stash `{ $name }` resta nell'elenco perché ci sono conflitti: risolverli (`index.stage` dei file) e poi eliminarlo con `stash.drop`
engine-stash-list-unexpected = dopo `{ $command }` l'elenco degli stash non è quello atteso (un altro processo lo ha cambiato?): rileggerlo; lo stash eliminato è in `{ $backup }`
engine-stash-list-unexpected-no-backup = dopo `{ $command }` l'elenco degli stash non è quello atteso (un altro processo lo ha cambiato?): rileggerlo; lo stash eliminato è fra i backup
engine-stash-list-unreadable-after = dopo `{ $command }` gli stash non si rileggono ({ $error })
