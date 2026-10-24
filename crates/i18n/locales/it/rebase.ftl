# Editor del rebase interattivo (rebase.rs, view/rebase_view.rs).
# I nomi delle azioni (pick, reword, squash, fixup, drop) restano nel codice, come in git.

rebase-menu-item = Rebase interattivo da qui…
rebase-menu-hint = Modifica il piano dei commit da { $id } fino a HEAD di «{ $branch }»: riordinali, uniscili, eliminali o cambiane il messaggio, con un'anteprima prima di eseguire
rebase-menu-root = { $id } è il primo commit: non ha un genitore su cui fare il rebase
rebase-menu-not-on-branch = Si possono riscrivere solo i commit della storia del branch attivo (seguendo i primi genitori da HEAD)
rebase-heading = Rebase interattivo di «{ $branch }»
rebase-heading-loading = Rebase interattivo
rebase-loading = Lettura dei commit…
rebase-intro = Commit da { $what } fino a HEAD, dal più vecchio come in git: trascinali per la maniglia per riordinarli; «squash» e «fixup» uniscono un commit a quello sopra.
rebase-steps = Piano
rebase-result = Risultato
rebase-result-empty = Il piano non è valido: nessuna anteprima
rebase-published = pubblicato
rebase-published-hint = Già su un branch remoto: riscriverlo richiede una conferma
rebase-drag-hint = Trascina per spostare questo commit
rebase-up-hint = Sposta su
rebase-down-hint = Sposta giù
rebase-action-pick-hint = Tiene il commit com'è
rebase-action-reword-hint = Tiene il commit con un nuovo messaggio
rebase-action-squash-hint = Lo unisce al commit sopra, combinando i messaggi
rebase-action-fixup-hint = Lo unisce al commit sopra, tenendo il messaggio del commit sopra
rebase-action-drop-hint = Elimina il commit
rebase-message-reword = Nuovo messaggio di { $id }:
rebase-message-group = Messaggio del commit unito (vuoto: { $default }):
rebase-message-default-squash = i messaggi uniti
rebase-message-default-fixup = il messaggio del primo commit
rebase-message-none = Scegli «reword», o l'ultimo «squash»/«fixup» di un gruppo, per scrivere un messaggio
rebase-source-combined = messaggi uniti
rebase-source-new = nuovo messaggio
rebase-from = da { $ids }
rebase-noop = Il piano non cambia nulla: modifica prima un'azione o l'ordine
rebase-run = Esegui rebase
rebase-run-hint = Riscrive «{ $branch }» con questo piano; il branch com'era resta in un backup e «Annulla» lo rimette
rebase-close = Chiudi
rebase-label = Rebase interattivo di «{ $branch }»
rebase-refused = Non possibile: { $error }
rebase-failed = Rebase di «{ $branch }» non riuscito: { $error }. Il repository viene ricaricato per mostrarne lo stato attuale
rebase-stopped = Rebase di «{ $branch }» fermo: { $files }. Risolvili e segnali come risolti nell'area di stage, poi «Continua rebase…» nel pannello in alto (o «Interrompi rebase…»)
rebase-stopped-before-commit = Rebase di «{ $branch }» fermo prima di un commit (un hook o la firma?): concludilo con «Continua rebase…» nel pannello in alto, o interrompilo
rebase-stopped-short = Rebase fermo
rebase-done = Rebase di «{ $branch }» fatto: { $before ->
        [one] 1 commit è diventato
       *[other] { $before } commit sono diventati
    } { $after }, il branch è su { $id }; il branch com'era è in { $backup }. «Annulla» lo rimette
rebase-confirm-heading = Riscrivere commit pubblicati?
rebase-confirm-text = Questi commit sono già su un branch remoto. Dopo il rebase il branch differisce da quello remoto: per pubblicarlo servirà un push forzato, e chi ha i vecchi commit dovrà riallinearsi.
rebase-confirm-button = Riscrivi comunque
rebase-confirm-back = Torna al piano
rebase-error-unknown = Il commit { $id } non è nell'intervallo del rebase
rebase-error-duplicate = Il commit { $id } compare più di una volta
rebase-error-missing = Il commit { $id } manca dal piano
rebase-error-all-dropped = Tutti i commit sono eliminati: tienine almeno uno
rebase-error-nothing-to-combine = Passo { $n }: il primo commit tenuto non può essere «squash» o «fixup», non c'è nulla sopra a cui unirlo
rebase-error-missing-message = Passo { $n }: «reword» richiede un messaggio
rebase-error-unexpected-message = Passo { $n }: un messaggio è ammesso solo su «reword» e sull'ultimo «squash»/«fixup» di un gruppo
rebase-error-reword-in-group = Passo { $n }: un «reword» non può essere seguito da «squash»/«fixup»; usa «pick» e scrivi il messaggio sull'ultimo passo del gruppo
rebase-error-empty-message = Passo { $n }: il messaggio è vuoto
rebase-error-nul = Passo { $n }: il messaggio contiene un carattere NUL
