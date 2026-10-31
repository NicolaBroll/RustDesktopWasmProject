# Fetch from the top bar and network error windows (fetch.rs).
# $label: the fetch description (fetch-label…); $who: fetch-who…

fetch-label = Fetch
fetch-label-remote = Fetch da «{ $remote }»
fetch-what-branch = Fetch di «{ $branch }» da «{ $remote }»
fetch-cancelled = { $label } interrotto: i branch remoti già aggiornati restano, nessun branch locale né file è stato toccato
fetch-failed-auth = { $label } non riuscito: autenticazione rifiutata o richiesta
fetch-failed-unreachable = { $label } non riuscito: remoto non raggiungibile
fetch-not-possible = { $label } non possibile: { $error }
fetch-failed-title = Fetch non riuscito
fetch-who = Il remoto
fetch-who-remote = Il remoto «{ $remote }»
fetch-nothing-changed = Niente è stato modificato.
fetch-auth-title = Autenticazione non riuscita
fetch-auth-explanation = { $who } ha rifiutato le credenziali, oppure non è stato possibile chiederle (nome utente, password, passphrase della chiave SSH, conferma della chiave dell'host). Controllare quanto inserito e riprovare; un credential helper (git config credential.helper) o la chiave caricata in ssh-agent (ssh-add) evitano che vengano chieste ogni volta.
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
fetch-pruned-backups = ; { $n ->
    [one] i commit presenti solo su un branch eliminato restano in un backup
   *[other] i commit presenti solo su { $n } branch eliminati restano in backup
} (refs/project/backup/prune-…)
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
fetch-phase-expanding-graph = Raccolta dei commit
fetch-phase-writing-graph = Scrittura del commit-graph
fetch-auto-label = Fetch automatico
fetch-auto-news = Fetch automatico: { $news }
fetch-auto-new-commits = { $n ->
    [one] 1 nuovo commit su { $branch }
   *[other] { $n } nuovi commit su { $branch }
}
fetch-auto-branch-moved = { $branch } aggiornato
fetch-auto-new-branch = nuovo branch { $branch }
fetch-auto-branch-gone = { $branch } rimosso
fetch-auto-suspended = Fetch automatico sospeso: il remote ha rifiutato le credenziali.
fetch-auto-suspended-hint = In pausa finché le credenziali non funzionano: premi «Riprova» per inserirle di nuovo, oppure configura un credential helper o ssh-agent perché git le trovi da solo.
fetch-auto-retry = Riprova
fetch-auto-retry-hint = Rifà il fetch adesso e, se funziona, riprende il fetch automatico.
fetch-auto-offline = Il remote non è raggiungibile: il fetch automatico riprova tra { $minutes ->
    [one] 1 minuto
   *[other] { $minutes } minuti
} (l'attesa raddoppia a ogni errore, fino a 30 minuti).
