# Local branches: create, checkout, delete, rename (branch.rs, view/branches.rs).

branch-start-commit = commit { $id }
branch-create-label = Creazione del branch «{ $name }»
branch-tag-label = Creazione del tag “{ $name }”
branch-tag-created = Tag “{ $tag }” creato su { $id }. “Annulla” lo elimina
branch-tag-created-annotated = Tag annotato “{ $tag }” creato su { $id }. “Annulla” lo elimina
branch-tag-title = Nuovo tag
branch-tag-button = Crea tag
branch-tag-text = Il tag punta a { $start } ({ $id }). Con un messaggio è annotato (autore, data e messaggio), senza è leggero. Non viene pubblicato sul remote.
branch-tag-name-hint = es. v1.0.0
branch-tag-message-hint = Messaggio (facoltativo: crea un tag annotato)
branch-rename-label = Rinomina di «{ $name }»
branch-checkout-label = Checkout di «{ $name }»
branch-delete-label = Eliminazione di «{ $name }»
branch-checkout-refused = Checkout di «{ $name }» non possibile: { $error }
branch-checkout-failed = Checkout di «{ $name }» non riuscito: { $error }
branch-delete-refused = Eliminazione di «{ $name }» non possibile: { $error }
branch-delete-failed = Eliminazione di «{ $name }» non riuscita: { $error }
branch-unexpected-response = risposta di `{ $command }` inattesa
branch-unexpected-result = { $command }: risultato inatteso: { $error }
branch-warning = ; attenzione: { $warning }
branch-created = Branch «{ $branch }» creato su { $id }. «Annulla» lo elimina
branch-checked-out = Passato al branch «{ $branch }». «Annulla» torna a quello di prima
branch-already-active = «{ $branch }» è già il branch attivo
branch-checkout-status-error = Checkout fatto, ma lo stato non si rilegge: { $error }
branch-deleted = Branch «{ $branch }» eliminato (era su { $id }{ $unmerged ->
        [0] {""}
        [one] , con 1 commit non unito
       *[other] , con { $unmerged } commit non uniti
    }); backup in { $backup }. «Annulla» lo ricrea
branch-deleted-upstream = Branch «{ $branch }» eliminato (era su { $id }{ $unmerged ->
        [0] {""}
        [one] , con 1 commit non unito
       *[other] , con { $unmerged } commit non uniti
    }); backup in { $backup }; tolta anche l'impostazione dell'upstream. «Annulla» lo ricrea con la sua configurazione
branch-deleted-config = Branch «{ $branch }» eliminato (era su { $id }{ $unmerged ->
        [0] {""}
        [one] , con 1 commit non unito
       *[other] , con { $unmerged } commit non uniti
    }); backup in { $backup }; tolta anche la configurazione { $keys }. «Annulla» lo ricrea con la sua configurazione
branch-renamed = Branch «{ $previous }» rinominato in «{ $branch }». «Annulla» gli rimette il nome di prima
branch-refs-warning = Fatto, ma i ref non si rileggono: { $error }

# Dialogs.
branch-new-title = Nuovo branch
branch-create-button = Crea il branch
branch-rename-title = Rinomina il branch
branch-rename-button = Rinomina
branch-create-text = Il branch parte da { $start } ({ $id }). Si crea senza passarci: il branch attivo non cambia.
branch-rename-text = Nuovo nome per «{ $name }». Reflog e upstream seguono il branch.
branch-name-hint = es. feature/nuova
branch-close = Chiudi
branch-delete-heading = Eliminare «{ $name }» ({ $id })?
branch-delete-unmerged = Il branch non è unito: ha commit che non sono nel branch attivo (né nel suo upstream). Eliminandolo quei commit non sono più raggiungibili da nessun branch.
branch-reason = Motivo:
branch-delete-backup = Prima di eliminare si salva un backup del commit (refs/project/backup/delete-…): con «Annulla» nella barra in alto il branch torna sullo stesso commit, con la sua configurazione (upstream compreso), finché l'app resta aperta: il diario delle operazioni è in memoria. Dopo la chiusura resta solo il backup del commit in refs/project/backup/, da cui il branch si ricrea a mano.
branch-delete-button = Elimina il branch
branch-dont-delete = Non eliminare
branch-refused-heading = Checkout di «{ $name }» non possibile
branch-refused-text = Il checkout sovrascriverebbe modifiche locali: questi percorsi sono modificati (in stage o no) o non tracciati (anche dentro una cartella che nel branch è un file, o in un submodule), e nel branch di destinazione sono diversi. Niente è stato modificato. Committare, scartare o mettere da parte queste modifiche e riprovare.
branch-refused-files = { $n ->
        [0] Git non ha indicato i file.
        [one] File:
       *[other] { $n } file:
    }
