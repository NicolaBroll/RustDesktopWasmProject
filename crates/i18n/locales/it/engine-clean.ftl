engine-clean-no-paths = nessun percorso: niente da cancellare
engine-clean-nested-repo = repository git annidati, niente cancellato: la loro storia non è nel backup; cancellarli fuori dall'app o indicare i file da cancellare: { $files }
engine-clean-not-untracked = { $n ->
    [one] non è un file non tracciato
   *[other] non sono file non tracciati
} (tracciati, ignorati, in conflitto o mancanti), niente cancellato: { $files }
engine-clean-plan-changed = i file non tracciati da cancellare sono cambiati dalla conferma (confermati: { $confirmed }; ora: { $now }): niente cancellato, rileggere lo stato e confermare di nuovo
engine-clean-confirm = { $n ->
    [one] 1 file non tracciato verrebbe cancellato
   *[other] { $n } file non tracciati verrebbero cancellati
} ({ $files }; { $bytes } byte in tutto). Prima si salverebbe un backup con i loro byte esatti in `refs/project/backup/clean-...`, e `edit.undo` li rimetterebbe
engine-clean-recreated = `{ $path }`: in quel percorso (o al posto di una delle sue cartelle) c'è di nuovo qualcosa: niente ripristinato, spostarlo e riprovare
engine-clean-changed-redo = `{ $path }`: il file non è più quello ripristinato (cambiato, in stage o sparito)
engine-clean-not-a-file = non più un file non tracciato o un link (una cartella, un file speciale, o un link al posto di una delle sue cartelle), niente cancellato: { $files }
engine-clean-changed-after-backup = cambiato dopo il backup
engine-clean-not-in-backup = non è nel backup
engine-clean-unreadable = `{ $path }` non si legge: { $error }
engine-clean-backup-incomplete = il backup (`{ $backup }`) non ha i byte attuali di { $files } (un file appena cambiato?): niente cancellato, riprovare
engine-clean-not-deleted = impossibile cancellare { $files } ({ $errors }): { $n ->
    [one] resta
   *[other] restano
}, il backup è in `{ $backup }`
engine-clean-nothing-deleted = niente cancellato ({ $errors }); il backup resta in `{ $backup }`
engine-clean-restore-failed = `{ $path }` non si ricrea ({ $error }); il backup resta nel commit { $commit }
engine-clean-not-as-expected = dopo `{ $command }` { $files } non sono come nel backup; resta nel commit { $commit }
