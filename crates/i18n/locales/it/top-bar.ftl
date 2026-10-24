# Top bar, status bar and welcome screen (view/top_bar.rs).

top-bar-repository = repository
top-bar-branch = branch
top-bar-refresh = Aggiorna
top-bar-refresh-hint = Rilegge ref, stato e log
top-bar-after-operation = Dopo l'operazione in corso
top-bar-stash = Stash…
top-bar-undo-in-operation = { $hint } (non durante un'operazione in corso: concluderla o annullarla dal riquadro in alto)
top-bar-undo-after-operation = { $hint } (dopo l'operazione in corso)
top-bar-changed-files = { $n ->
    [one] 1 file modificato
   *[other] { $n } file modificati
}
top-bar-head-unborn = { $name } (senza commit)
top-bar-head-detached = HEAD staccata su { $id }
top-bar-fetch = Fetch
top-bar-fetch-hint = Scarica commit, branch remoti e tag dal remoto dell'upstream del branch attivo (altrimenti origin, o l'unico remoto). Non tocca branch locali, indice né file
top-bar-fetch-from = Fetch da «{ $remote }»
top-bar-fetch-from-hint = Sceglie il remoto da cui fare il fetch
top-bar-pull = Pull
top-bar-push = Push
top-bar-detached-hint = HEAD è staccata (o il branch non ha commit): passare a un branch
top-bar-pull-hint = Scarica { $upstream } e lo integra in «{ $branch }»: fast-forward se può, altrimenti merge o rebase secondo pull.rebase (senza configurazione: merge). Con modifiche locali il merge non parte. «Annulla» riporta il branch dov'era
top-bar-push-hint = Pubblica «{ $branch }» su { $upstream }. Mai forzato: se il remoto ha commit che non hai il push è rifiutato e si propone il Pull. Un push non si annulla
top-bar-pull-no-upstream-hint = «{ $branch }» non ha un upstream: il pull spiega come impostarlo (oppure fare Push, che lo imposta)
top-bar-push-first-hint = Primo push di «{ $branch }»: lo pubblica su origin (o sull'unico remoto) con lo stesso nome e imposta l'upstream. Un push non si annulla
top-bar-connecting = Connessione al remoto…
top-bar-stop = Interrompi
top-bar-stopping = Interruzione in corso…
top-bar-running = { $what } in corso…
top-bar-running-queued = { $what } in corso… (+{ $n } in coda)
top-bar-opening = Apertura del repository…
top-bar-no-repo = Nessun repository aperto
top-bar-open-repo = Apri un repository…
top-bar-recent = Apri recenti
top-bar-recent-hint = Repository aperti di recente: uno si apre in una scheda, o mostra la sua scheda se è già aperto
top-bar-recent-none = Ancora nessun repository aperto
top-bar-recent-item = { $name } — { $path }
top-bar-recent-heading = Repository recenti
top-bar-recent-missing-hint = La cartella { $path } non esiste più
top-bar-recent-forget = Rimuovi mancanti
top-bar-recent-forget-hint = Toglie dall'elenco dei recenti i repository la cui cartella non esiste più (le cartelle non vengono toccate)
