engine-reset-target-empty = `target` è vuoto: indicare il commit su cui fare il reset (un id, un branch, un tag)
engine-reset-unborn = il branch `{ $name }` non ha ancora commit: non c'è niente da resettare
engine-reset-expected-hex = `expected` deve essere l'id completo ({ $len } cifre esadecimali) del commit su cui era HEAD: `{ $value }`
engine-reset-head-moved = HEAD si è spostato dopo la conferma (era su { $seen }, ora è su { $now }): niente cambiato, guardare di nuovo la cronologia e confermare di nuovo
engine-reset-confirm = un reset hard su { $after } sovrascriverebbe le modifiche locali di { $n ->
    [one] 1 file
   *[other] { $n } file
} ({ $files }). Prima verrebbe salvato un backup in `refs/project/backup/reset-...` (`git stash apply` le rimette); i file non tracciati non vengono toccati
engine-reset-published = { $n ->
    [one] 1 commit che esce
   *[other] { $n } commit che escono
} da `{ $branch }` { $n ->
    [one] è
   *[other] sono
} già su `{ $upstream }`: il prossimo push dovrà sovrascrivere il branch remoto (push con lease)
engine-reset-bad-request = `{ $command }`: richiesta di reset non valida (HEAD e il branch da spostare non corrispondono)
engine-reset-intent-to-add = ci sono file `intent-to-add` (`git add -N`: { $files }): un reset hard li cancellerebbe e il backup (`git stash create`) non può salvarli. Niente cambiato: metterli in stage (`index.stage`) o toglierli (`index.unstage`) e riprovare
engine-reset-files-changed = le modifiche locali sono cambiate dopo la conferma (confermate: { $confirmed }; ora: { $now }): niente cambiato, ricaricare lo stato e confermare di nuovo
engine-reset-undo-staged = `{ $path }`: l'indice è cambiato dopo il reset (modifiche in stage o un conflitto)
engine-reset-undo-changed = `{ $path }`: il file ha modifiche locali
engine-reset-moved = HEAD o il branch sono cambiati mentre si preparava il reset ({ $changes }): niente cambiato, ricaricare e riprovare
engine-reset-in-the-way = file (o cartelle) non tracciati o ignorati si trovano dove il commit di destinazione mette dei file: { $files }. Un reset hard li sovrascriverebbe o cancellerebbe e il backup non li contiene. Niente cambiato: spostarli e riprovare
engine-reset-not-commit = `{ $target }` non è un commit: niente cambiato
engine-reset-head-changed-after = dopo lo spostamento del ref HEAD è { $now } invece di { $target } (un altro processo l'ha cambiato in quell'istante): indice e worktree non sono stati toccati da `{ $command }`
engine-reset-index-copy = impossibile copiare l'indice `{ $path }` per il backup: { $error }
engine-reset-backup-incomplete = il backup del worktree (`{ $backup }`) non ha il contenuto attuale di { $files } (un file cambiato proprio adesso?): niente cambiato, riprovare
engine-reset-note-backups = i backup restano in { $backups }
engine-reset-note-moved = il branch si è già spostato (era su { $before }), indice e worktree potrebbero non corrispondere; backup: { $backups }
engine-reset-undo-backup = le modifiche locali sovrascritte dal reset hard non vengono rimesse: restano in `{ $backup }` (`git stash apply { $stash }`)
engine-reset-hidden = file marcati `--assume-unchanged` o `--skip-worktree` differiscono tra HEAD e la destinazione ({ $files }): git non vede le loro modifiche locali e un reset hard potrebbe sovrascriverle. Niente cambiato: togliere il flag (`git update-index --no-assume-unchanged` / `--no-skip-worktree`) e riprovare
