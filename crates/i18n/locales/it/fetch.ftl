# Fetch dalla barra e finestre di errore di rete (fetch.rs).
# $label: la descrizione del fetch (fetch-label…); $who: fetch-who…

fetch-label = Fetch
fetch-label-remote = Fetch da «{ $remote }»
fetch-what-branch = Fetch di «{ $branch }» da «{ $remote }»
fetch-unexpected-response = risposta di `{ $command }` inattesa
fetch-unexpected-result = { $command }: risultato inatteso: { $error }
fetch-cancelled = { $label } interrotto: i branch remoti già aggiornati restano, nessun branch locale né file è stato toccato
fetch-failed-auth = { $label } non riuscito: autenticazione rifiutata o richiesta
fetch-failed-unreachable = { $label } non riuscito: remoto non raggiungibile
fetch-not-possible = { $label } non possibile: { $error }
fetch-failed = { $label } non riuscito: { $error }
fetch-failed-title = Fetch non riuscito
fetch-who = Il remoto
fetch-who-remote = Il remoto «{ $remote }»
fetch-nothing-changed = Niente è stato modificato.
fetch-auth-title = Autenticazione non riuscita
fetch-auth-explanation = { $who } ha rifiutato le credenziali, oppure servirebbe chiederle (nome utente, password, passphrase della chiave SSH, conferma della chiave dell'host). Questa app non chiede mai credenziali: configurare un credential helper (git config credential.helper) oppure caricare la chiave in ssh-agent (ssh-add); per un host SSH nuovo, aggiungerlo a known_hosts con un primo fetch da terminale. Poi riprovare.
fetch-unreachable-title = Remoto non raggiungibile
fetch-unreachable-explanation = { $who } non risponde: controllare l'indirizzo del remoto (git remote -v), la rete e che il repository esista sul server.
fetch-ref-tag = tag { $tag }
fetch-done = { $what } fatto
fetch-nothing-new = { $what }: niente di nuovo
fetch-ref-new = { $name } nuovo
fetch-ref-removed = { $name } tolto
fetch-and-more = e altri { $n }
fetch-updated = { $what }: { $n ->
    [one] 1 ref aggiornato
   *[other] { $n } ref aggiornati
} ({ $names })
fetch-warning = ; attenzione: { $warning }
fetch-refs-not-reread = Fetch fatto, ma i ref non si rileggono: { $error }
fetch-progress-remote = remoto: { $phase }
fetch-phase-enumerating = Enumerazione oggetti
fetch-phase-counting = Conteggio oggetti
fetch-phase-compressing = Compressione oggetti
fetch-phase-receiving = Ricezione oggetti
fetch-phase-unpacking = Estrazione oggetti
fetch-phase-resolving = Risoluzione delta
fetch-phase-writing = Scrittura oggetti
fetch-phase-total = Totale
