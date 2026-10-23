# Messaggi dell'app: risposte dei comandi, barra di rete (lib.rs, executor.rs).

app-commit-not-loaded = il commit { $id } non è tra quelli caricati
app-index-backup = Backup dell'indice salvato in { $backup }
app-write-stale = { $label } non riuscito: il file è cambiato: rileggi il diff e rifai la selezione (il diff è stato riletto)
app-done-status-unread = Fatto, ma lo stato non si rilegge: { $error }
app-no-result = `{ $command }` è terminato senza risultato
app-stop-fetch-hint = Ferma il fetch: i branch remoti già aggiornati restano, i branch locali e i file non sono toccati
app-stop-add-remote-hint = Ferma il fetch: il remoto resta aggiunto, i branch remoti già scaricati restano
app-stop-pull-hint = Ferma il pull durante il fetch: il branch non viene toccato. Se l'integrazione è già partita finisce comunque
app-stop-push-hint = Ferma il push: se l'invio era già finito il remoto potrebbe essere aggiornato (un Fetch lo mostra)
