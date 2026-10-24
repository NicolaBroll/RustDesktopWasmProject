# L'avviso del commit-graph sotto la barra e repo.optimize (optimize.rs, view/optimize_view.rs).
# $seconds: quanto ha impiegato la prima pagina della cronologia.

optimize-label = Ottimizzazione del repository
optimize-slow = La cronologia ha impiegato { $seconds } s a caricarsi: questo repository non ha un commit-graph, quindi git legge tutti i commit prima di mostrare i primi.
optimize-running = Scrittura del commit-graph: su un repository grande richiede un po', l'app resta utilizzabile.
optimize-explanation = Ottimizza scrive il commit-graph una volta (git commit-graph write --reachable): una cache ricavata dai commit che fa caricare la cronologia in millisecondi. Nessun branch, file o commit cambia.
optimize-button = Ottimizza
optimize-not-now = Non ora
optimize-not-now-hint = Nasconde questo avviso fino alla prossima apertura del repository
optimize-after-operation = Dopo l'operazione in corso
optimize-stop-hint = Interrompe la scrittura del commit-graph: il repository resta com'era
optimize-done = Commit-graph scritto: ora la cronologia si carica velocemente
optimize-cancelled = Ottimizzazione interrotta: non è cambiato nulla
optimize-failed = Ottimizzazione non riuscita: { $error }
optimize-unexpected-response = risposta inattesa da `{ $command }`
