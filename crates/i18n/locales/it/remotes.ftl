# Remotes and upstream (remotes.rs).

remotes-ahead = { $n } commit da pubblicare
remotes-behind = { $n } commit da scaricare
remotes-in-sync = Allineato con { $name }
remotes-relative = { $counts } rispetto a { $name }
remotes-upstream-gone = upstream sparito
remotes-upstream-gone-hint = L'upstream { $name } non c'è più (tolto sul remoto o il remoto non lo ha più): un Fetch non lo riporta; Push lo ricrea

# “Remotes” section, adding and removing (remotes.rs, view/remotes_view.rs).
remotes-no-url = (senza URL)
remotes-no-branches = nessun branch
remotes-branches = { $n } branch
remotes-badge-default = predefinito
remotes-badge-default-hint = «Fetch» nella barra scarica da qui (upstream del branch attivo, origin o l'unico remoto)
remotes-badge-push = push
remotes-badge-push-hint = remote.pushDefault: i push dei branch senza pushRemote vanno su «{ $name }»
remotes-badge-mirror = mirror
remotes-badge-mirror-hint = remote.<nome>.mirror: un push rispecchia tutti i ref locali
remotes-tooltip =
    Remoto «{ $name }»
    URL: { $url }
remotes-tooltip-push-url = Push: { $url }
remotes-tooltip-credentials = L'URL contiene credenziali, salvate in chiaro nella configurazione del repo (qui mascherate): meglio un credential helper
remotes-credentials-warning = L'URL contiene credenziali: git le salva in chiaro nella configurazione del repo (.git/config). Meglio un URL senza e un credential helper (o una chiave SSH).
remotes-add-label = Aggiunta del remoto «{ $name }»
remotes-add-label-fetch = Aggiunta del remoto «{ $name }» e fetch
remotes-remove-label = Rimozione del remoto «{ $name }»
remotes-remove-check-label = Controllo per la rimozione di «{ $name }»
remotes-add-failed = Aggiunta del remoto «{ $name }» non riuscita: { $error }
remotes-remove-refused = Rimozione del remoto «{ $name }» non possibile: { $error }
remotes-remove-failed = Rimozione del remoto «{ $name }» non riuscita: { $error }
remotes-refs-not-reread = i ref non si rileggono: { $error }
remotes-remotes-not-reread = i remoti non si rileggono: { $error }
remotes-reread-warning = { $what }, ma { $problems }
remotes-added-anyway = Il remoto è stato aggiunto comunque, senza branch scaricati: riprovare con «Fetch» nella sezione Remoti, oppure «Annulla» per toglierlo.
remotes-added = Remoto «{ $name }» aggiunto ({ $url })
remotes-added-fetch-cancelled = ; fetch interrotto: i branch remoti già scaricati restano, «Fetch» nella sezione Remoti lo riprende
remotes-added-fetch-auth = ; fetch non riuscito: autenticazione rifiutata o richiesta
remotes-added-fetch-unreachable = ; fetch non riuscito: remoto non raggiungibile
remotes-added-fetch-failed = ; fetch non riuscito
remotes-fetch-failed-title = Fetch non riuscito
remotes-added-undo = . «Annulla» lo toglie
remotes-added-short = Remoto aggiunto
remotes-quoted = «{ $name }»
remotes-removed = Remoto «{ $name }» rimosso ({ $n ->
    [0] nessun branch remoto da togliere
    [one] tolto 1 branch remoto
   *[other] tolti { $n } branch remoti
})
remotes-removed-upstreams = ; senza upstream: { $names }
remotes-removed-push-remotes = ; tolto il pushRemote di { $names }
remotes-removed-push-default = ; tolto remote.pushDefault
remotes-removed-backup = ; backup in { $backup }
remotes-removed-backups = ; { $n } backup in refs/project/backup/remote-…
remotes-removed-undo = . «Annulla» lo rimette
remotes-removed-short = Remoto rimosso
remotes-delete-branch-check-label = Controllo eliminazione di «{ $name }»
remotes-delete-branch-label = Eliminazione di «{ $name }» dal remoto
remotes-delete-branch-refused = Eliminare «{ $name }» dal remoto non è possibile: { $error }
remotes-delete-branch-failed = Eliminazione di «{ $name }» dal remoto non riuscita: { $error }
remotes-branch-deleted = Branch «{ $name }» eliminato dal remoto
remotes-branch-deleted-only-here = ; { $n ->
    [one] 1 commit era
   *[other] { $n } commit erano
} solo lì
remotes-branch-deleted-backup = ; backup in { $backup }
remotes-branch-deleted-upstreams = ; upstream sparito per { $names }
remotes-branch-deleted-short = Branch eliminato dal remoto
remotes-section = Remoti
remotes-section-count = Remoti ({ $n })
remotes-none = Nessun remoto configurato
remotes-loading = Lettura dei remoti…
remotes-list-error = Remoti non letti: { $error }
remotes-add-button-hint = Aggiunge un remoto (nome e URL) e, se scelto, ne scarica subito i branch. «Annulla» lo toglie
remotes-row-hint =
    { $tooltip }
    Clic destro: fetch, modifica URL, rinomina, rimozione
remotes-menu-hint = Fetch, modifica URL, rinomina, rimozione
remotes-fetch-button = Fetch
remotes-fetch-button-hint = Scarica commit, branch remoti e tag da «{ $name }». Non tocca branch locali, indice né file
remotes-remove-button = Rimuovi…
remotes-remove-button-hint = Toglie il remoto da questo repo, con i suoi branch remoti scaricati e gli upstream che lo usano; prima chiede conferma. Il repository sul server non cambia; «Annulla» lo rimette
remotes-add-title = Aggiungi remoto
remotes-add-explanation = Un remoto è un altro repository (sul server o in una cartella) da cui scaricare e su cui pubblicare. Si aggiunge con i branch remoti nome/* e niente cambia nei branch locali; «Annulla» lo toglie.
remotes-add-name = Nome
remotes-add-name-hint = es. origin, upstream
remotes-add-url = URL
remotes-add-url-hint = https://host/progetto.git, git@host:utente/progetto.git
remotes-show-url = Mostra l'URL
remotes-show-url-hint = Mostra l'URL com'è scritto, per correggerlo
remotes-hide-credentials = Nascondi le credenziali
remotes-hide-credentials-hint = Mostra l'URL con le credenziali mascherate
remotes-add-fetch = Scarica subito (fetch)
remotes-add-fetch-hint = Dopo l'aggiunta scarica commit, branch remoti e tag. Se il fetch non riesce (o lo si interrompe) il remoto resta aggiunto
remotes-add-submit = Aggiungi
remotes-close = Chiudi
remotes-adding = Aggiunta…
remotes-connecting = Connessione al remoto…
remotes-cancel = Interrompi
remotes-cancel-hint = Ferma il fetch: il remoto resta aggiunto, i branch remoti già scaricati restano
remotes-cancelling = Interruzione in corso…
remotes-remove-title = Rimuovere il remoto «{ $name }»?
remotes-remove-explanation = Si tolgono da questo repo la configurazione del remoto e i suoi branch remoti scaricati; i branch locali che lo seguono restano, ma senza upstream. Il repository sul server non cambia e i tag scaricati restano.
remotes-remove-what = Cosa succede:
remotes-remove-undo = Con «Annulla» nella barra in alto il remoto torna com'era (URL, branch remoti, upstream), finché l'app resta aperta: il diario delle operazioni è in memoria. Dopo la chiusura restano solo i backup dei commit in refs/project/backup/remote-…: il remoto si riaggiunge a mano e un Fetch riscarica i branch.
remotes-remove-confirm = Rimuovi il remoto
remotes-remove-cancel = Non rimuovere
remotes-delete-branch-title = Eliminare «{ $name }» dal remoto?
remotes-delete-branch-explanation = Il branch «{ $branch }» viene eliminato sul remoto «{ $remote }»: sparisce per tutti quelli che lo usano, e sparisce anche qui il suo branch remoto locale. I branch locali non cambiano.
remotes-delete-branch-undo = Non si annulla con «Annulla»: il remoto è anche di altri. Il commit resta qui in un backup (refs/project/backup/push-delete-…): per rimettere il branch, crearne uno su quel commit e pubblicarlo. Se qualcuno ha aggiornato il branch dopo il tuo ultimo Fetch, l'eliminazione è rifiutata e non cambia niente.
remotes-delete-branch-confirm = Elimina dal remoto
remotes-delete-branch-cancel = Non eliminare
remotes-delete-tag-check-label = Lettura del tag «{ $name }» su «{ $remote }»
remotes-delete-tag-label = Eliminazione del tag «{ $name }» da «{ $remote }»
remotes-delete-tag-refused = Eliminazione del tag «{ $name }» da «{ $remote }» non possibile: { $error }
remotes-delete-tag-failed = Eliminazione del tag «{ $name }» da «{ $remote }» non riuscita: { $error }
remotes-tag-deleted = Tag «{ $name }» eliminato da «{ $remote }»
remotes-tag-deleted-short = Tag eliminato dal remoto
remotes-delete-tag-title = Eliminare il tag «{ $name }» da «{ $remote }»?
remotes-delete-tag-explanation = Il tag «{ $name }» viene eliminato sul remoto «{ $remote }»: sparisce per tutti quelli che lo usano. Il tag locale non cambia.
remotes-delete-tag-undo = Non si annulla con «Annulla»: il remoto è anche di altri. Il valore resta qui in un backup (refs/project/backup/push-delete-…). Se il tag è cambiato sul remoto dopo la lettura, l'eliminazione è rifiutata e non cambia niente.

# Modifica URL e rinomina (remote_edit.rs, S59).
remotes-edit-url-button = Modifica URL…
remotes-edit-url-button-hint = Cambia l'URL da cui il remoto scarica (e, se serve, un URL di push separato). Branch remoti e upstream restano; «Annulla» rimette l'URL di prima
remotes-rename-button = Rinomina…
remotes-rename-button-hint = Rinomina il remoto: i suoi branch remoti e gli upstream che lo usano seguono il nuovo nome. Non si perde niente; «Annulla» rimette il nome di prima
remotes-edit-url-title = Modifica URL di «{ $name }»
remotes-edit-url-explanation = I branch remoti e gli upstream dei branch locali restano come sono; il prossimo Fetch usa il nuovo URL. «Annulla» rimette quello di prima.
remotes-edit-fetch-url = URL di fetch
remotes-edit-push-url = URL di push separato
remotes-edit-push-url-tooltip = Facoltativo: dove va il Push, se non all'URL di fetch (remote.<nome>.pushurl). Vuoto lo lascia com'è
remotes-edit-push-url-hint = facoltativo: lo stesso dell'URL di fetch
remotes-edit-masked-note = L'URL attuale contiene credenziali ed è mostrato mascherato come suggerimento: scrivi tutto il nuovo URL, o lascia il campo vuoto per tenerlo.
remotes-edit-to-ssh = Da HTTPS a SSH: git userà la tua chiave SSH invece della password o del token salvati; la chiave deve essere registrata sul server.
remotes-edit-to-https = Da SSH a HTTPS: git non userà più la tua chiave SSH e chiederà nome utente e password o token (o userà il credential helper).
remotes-edit-url-submit = Salva URL
remotes-rename-title = Rinomina il remoto «{ $name }»
remotes-rename-explanation = I branch remoti { $name }/* passano sotto il nuovo nome, e i branch locali che li seguono vanno con loro. Tag e branch locali non cambiano; «Annulla» rimette il nome di prima.
remotes-rename-submit = Rinomina
remotes-set-url-label = Modifica dell'URL di «{ $name }»
remotes-set-push-url-label = Modifica dell'URL di push di «{ $name }»
remotes-rename-label = Rinomina del remoto «{ $name }» in «{ $new }»
remotes-set-url-failed = Modifica dell'URL di «{ $name }» non riuscita: { $error }
remotes-set-push-url-failed = Modifica dell'URL di push di «{ $name }» non riuscita: { $error }
remotes-rename-failed = Rinomina del remoto «{ $name }» non riuscita: { $error }
remotes-url-set = URL di «{ $name }» cambiato in { $url }
remotes-push-url-set = URL di push di «{ $name }» cambiato in { $url }
remotes-url-unchanged = L'URL di «{ $name }» era già { $url }: niente cambiato
remotes-push-url-unchanged = L'URL di push di «{ $name }» era già { $url }: niente cambiato
remotes-url-set-undo = ; «Annulla» rimette quello di prima
remotes-url-set-short = URL cambiato
remotes-renamed = Remoto «{ $name }» rinominato in «{ $new }»
remotes-renamed-refs = { $n ->
    [one] ; 1 branch remoto spostato
   *[other] ; { $n } branch remoti spostati
}
remotes-renamed-upstreams = ; upstream di { $names } ora su «{ $new }»
remotes-renamed-undo = . «Annulla» rimette il nome di prima
remotes-renamed-short = Remoto rinominato
