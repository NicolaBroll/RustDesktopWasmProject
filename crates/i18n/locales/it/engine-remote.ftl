engine-remote-push-default-changed = `remote.pushDefault` è cambiato
engine-remote-upstreams-changed = l'upstream o il pushRemote dei branch che usano `{ $name }` è cambiato
engine-remote-ref-new = `{ $name }`: non doveva esserci, ora è { $now }
engine-remote-ref-gone = `{ $name }`: doveva essere { $expected }, ora non c'è
engine-remote-config-changed = la configurazione `remote.{ $name }.*` è cambiata
engine-remote-head-none = niente
engine-remote-head-changed = `refs/remotes/{ $name }/HEAD`: doveva puntare a { $expected }, ora a { $now }
engine-remote-changed = il remoto `{ $name }` è cambiato mentre `{ $command }` lo leggeva ({ $refs }): niente modificato, riprovare
engine-remote-exists-again = il remoto `{ $name }` esiste di nuovo (configurazione o branch remoti)
engine-remote-ref-exists = `{ $name }` c'è già
engine-remote-put-back = rimesso com'era
engine-remote-put-back-partly = rimesso solo in parte ({ $warnings }): la rimozione è nel diario (`edit.undo` la annulla) e i commit dei branch remoti nei backup
engine-remote-not-put-back = NON rimesso ({ $error }): la rimozione è nel diario (`edit.undo` la annulla) e i commit dei branch remoti nei backup
engine-remote-url-empty = l'URL del remoto è vuoto
engine-remote-url-control = l'URL contiene caratteri di controllo
engine-remote-url-spaces = l'URL inizia o finisce con spazi
engine-remote-url-dash = un URL non può iniziare con `-` (sembrerebbe un'opzione)
engine-remote-url-transport = il trasporto `{ $transport }` esegue comandi o usa descrittori aperti: non permesso
engine-remote-url-authority-dash = l'utente o l'host dell'URL non può iniziare con `-`
engine-remote-name-empty = il nome del remoto è vuoto
engine-remote-name-control = il nome del remoto contiene caratteri di controllo
engine-remote-name-invalid = non è un nome di remoto valido
engine-remote-none-configured = il repository non ha remoti configurati
engine-remote-configured = remoti configurati: { $names }
engine-remote-still-listed = dopo la rimozione git vede ancora il remoto `{ $name }` (è definito anche fuori dal file del repo): { $how }. Toglierlo da dove è definito
engine-remote-removed-unreadable = il remoto `{ $name }` è stato rimosso, ma l'elenco dei remoti non si rilegge ({ $error }): la rimozione è nel diario (`edit.undo` la annulla) e i commit dei branch remoti nei backup
engine-remote-name-dash = un nome di remoto non può iniziare con `-`: `{ $name }`
engine-remote-unknown = non esiste un remoto `{ $remote }` ({ $known }); niente scaricato
engine-remote-none = `{ $id }`: il repository non ha remoti configurati
engine-remote-which = `{ $id }`: il branch attivo non ha un upstream e non c'è un remoto `origin`: indicare `remote` fra { $names }
engine-remote-no-branch = il remoto `{ $remote }` non ha il branch `{ $branch }`, niente scaricato
engine-remote-missing = non esiste un remoto `{ $remote }`
engine-remote-exists = esiste già un remoto `{ $name }`, niente modificato
engine-remote-remove-config = la configurazione `remote.{ $name }.*` e { $n } branch remoti (`refs/remotes/{ $name }/...`)
engine-remote-remove-upstreams = l'upstream dei branch { $branches } (`branch.<b>.remote`/`merge`)
engine-remote-remove-push-remote = il `pushRemote` dei branch { $branches }
engine-remote-remove-push-default = `remote.pushDefault`
engine-remote-remove-confirm = rimuovere il remoto `{ $name }` toglierebbe { $summary }. Il remoto vero non cambia; i commit che restano solo nei branch remoti si salvano prima in `refs/project/backup/remote-...`, e `edit.undo` rimette tutto
engine-remote-remove-promisor = `{ $remote }` è il remoto promisor di un clone parziale (`remote.{ $name }.promisor` o `extensions.partialClone`): gli oggetti che mancano in locale si scaricano da lì, e senza il remoto il repo non li troverebbe più (checkout, log e diff fallirebbero). Niente modificato
engine-remote-remove-partly-elsewhere = il remoto `{ $name }` è definito anche fuori dal file del repo: { $elsewhere }. Rimuovendolo da qui ne resterebbe una parte (git lo vedrebbe ancora): niente modificato, togliere prima quelle voci
engine-remote-remove-elsewhere = il remoto `{ $name }` è configurato fuori dal file del repo (configurazione globale, `include` o `.git/remotes/`): niente modificato, toglierlo da dove è definito
engine-remote-remove-unknown = non esiste un remoto `{ $name }` ({ $known }); niente modificato
engine-remote-add-refs-exist = ci sono già branch remoti `refs/remotes/{ $name }/...` ({ $n }) rimasti da un remoto tolto a mano: niente modificato (scegliere un altro nome)
engine-remote-add-config-exists = la configurazione del repo ha già voci `remote.{ $name }.*`: niente modificato (scegliere un altro nome o toglierle)
engine-remote-add-exists = esiste già un remoto `{ $name }`, niente modificato
engine-remote-no-such-branch = il remoto `{ $remote }` non ha il branch chiesto, niente scaricato: { $message }
engine-remote-git-rejects-name = git non accetta il nome del remoto `{ $name }`: { $stderr }
engine-remote-config-not-utf8 = il valore di `{ $key }` non è UTF-8: non si può salvare per l'undo, niente modificato (correggerlo con `git config`)
engine-remote-ref-not-utf8 = il remoto `{ $name }` ha un branch remoto con un nome non UTF-8: non si può salvare per l'undo, niente modificato
engine-remote-head-not-removed = `{ $head_ref }` (verso `{ $target }`) non si è tolto: { $error }
engine-remote-config-not-removed = la configurazione `remote.{ $name }.*` non si è tolta: { $error }
engine-remote-key-not-removed = `{ $key }` non si è tolto: { $error }
engine-remote-key-not-written = `{ $key }` non è stato riscritto: { $error }
engine-remote-head-not-put-back = `{ $head_ref }` non è stato rimesso (verso `{ $target }`): { $error }
engine-remote-refs-not-put-back = i branch remoti di `{ $name }` non sono stati rimessi ({ $error }): i commit sono nei backup e nella voce del diario
engine-remote-upstreams-not-put-back = upstream e pushRemote non rimessi: { $error }
engine-remote-upstream-taken = il branch `{ $branch }` ha già { $what ->
    [push-remote] il pushRemote
   *[other] l'upstream
} nuovo: { $what ->
    [push-remote] il pushRemote
   *[other] l'upstream
} verso `{ $name }` non è stato rimesso
engine-remote-upstream-branch-gone = il branch `{ $branch }` non c'è più: { $what ->
    [push-remote] il pushRemote
   *[other] l'upstream
} verso `{ $name }` non è stato rimesso
engine-remote-push-default-taken = `remote.pushDefault` ha già un altro valore: non è stato rimesso a `{ $name }`
engine-remote-memory-no-merge = upstream di `{ $branch }` senza `merge`: non riscritto nel repo in memoria
engine-remote-fetch-unexpected-ref = il fetch ha cambiato `{ $name }` (da { $before } a { $after }) per un refspec configurato in `remote.<nome>.fetch`: il valore di prima è nel reflog del ref
engine-remote-url-credentials = l'URL contiene credenziali: git le ha salvate in chiaro nella configurazione del repo (`remote.{ $name }.url`); meglio un credential helper
