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
branch-here-text = Il branch parte dalla HEAD staccata ({ $id }) e poi diventa il branch attivo: i nuovi commit restano su di esso.
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
branch-tag-delete-label = Eliminazione del tag «{ $name }»
branch-tag-delete-refused = Impossibile eliminare il tag «{ $name }»: { $error }
branch-tag-delete-failed = Eliminazione del tag «{ $name }» non riuscita: { $error }
branch-tag-deleted = Tag «{ $tag }» eliminato (era su { $id }); backup in { $backup }. «Annulla» lo ricrea
branch-tag-deleted-annotated = Tag annotato «{ $tag }» eliminato (era su { $id }); l'oggetto tag è in { $backup }. «Annulla» lo ricrea
branch-tag-delete-heading = Eliminare il tag «{ $name }»?
branch-tag-delete-text = Il tag viene eliminato solo qui: i remoti tengono la loro copia (per quello c'è «Elimina dal remoto…»).
branch-tag-delete-backup = Prima di eliminarlo, il tag viene salvato in un backup (refs/project/backup/tag-delete-…), con messaggio e autore se è annotato: «Annulla» nella barra in alto lo ricrea sullo stesso valore, finché l'app resta aperta.
branch-tag-delete-button = Elimina il tag

# Branch che seguono un remoto e upstream (tracking.rs).
branch-created-tracking = Branch «{ $branch }» creato su { $id }, segue { $upstream }. «Annulla» lo elimina con il suo upstream
branch-track-label = Creazione di «{ $name }» che segue { $upstream }
branch-track-failed = Impossibile creare il branch locale «{ $name }»: { $error }
branch-track-name-taken = Esiste già un branch locale «{ $name }» che non segue questo branch remoto: scegli un altro nome.
branch-track-checkbox = Segui { $upstream }
branch-track-hint = Lo imposta come upstream del nuovo branch: Pull e Push lo usano e il branch mostra avanti/indietro
branch-upstream-label = Impostazione dell'upstream di «{ $name }» a { $upstream }
branch-upstream-unset-label = Rimozione dell'upstream di «{ $name }»
branch-upstream-unset-failed = Impossibile rimuovere l'upstream di «{ $name }»: { $error }
branch-upstream-set = «{ $branch }» ora segue { $upstream }. «Annulla» ripristina l'upstream precedente
branch-upstream-unset = «{ $branch }» non segue più { $previous }. «Annulla» lo ripristina
branch-upstream-unchanged = L'upstream di «{ $branch }» non è cambiato
branch-upstream-title = Upstream di «{ $name }»
branch-upstream-text-current = Ora segue { $upstream }. Scegli il branch remoto che Pull e Push devono usare.
branch-upstream-text-none = Non ha upstream. Scegli il branch remoto che Pull e Push devono usare.
branch-upstream-filter-hint = Filtra i branch remoti
branch-upstream-no-remote-branches = Nessun branch remoto: aggiungi un remoto e fai fetch prima.
branch-upstream-no-match = Nessun branch remoto corrisponde al filtro.
branch-upstream-current-mark = { $upstream } (attuale)
branch-upstream-button = Imposta upstream
