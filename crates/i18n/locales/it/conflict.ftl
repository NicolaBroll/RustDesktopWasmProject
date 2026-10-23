# Editor dei conflitti (conflict.rs, view/conflict_view.rs).
# $command: id del comando; $path: percorso del file; $backup: ref o percorso del backup.

conflict-worktree-not-text = il file del worktree non è testo o non c'è
conflict-no-blocks = il risultato non ha blocchi da scegliere
conflict-unexpected-result = { $command }: risultato inatteso: { $error }
conflict-label-use-ours = Risoluzione di { $path } con la versione nostra
conflict-label-use-theirs = Risoluzione di { $path } con la versione loro
conflict-label-delete = Eliminazione di { $path } per risolvere il conflitto
conflict-label-resolve = Risoluzione del conflitto di { $path }
conflict-resolved-path = Conflitto di { $path } risolto e in stage
conflict-resolved = Conflitto risolto e in stage
conflict-resolved-backup = ; il file com'era è salvato in { $backup }
conflict-resolved-index-backup = ; backup dell'indice in { $backup }
conflict-stale = Il file è cambiato dopo la lettura: riletto, rifare le scelte
conflict-resolve-failed = Risoluzione non riuscita: { $error }
conflict-title = Conflitto
conflict-close = Chiudi
conflict-close-hint = Torna alla lista: le scelte non segnate come risolte si perdono
conflict-restart = Ricomincia
conflict-restart-hint = Rilegge il file dal disco: le scelte fatte qui si perdono
conflict-loading = Lettura delle versioni…
conflict-load-failed = Versioni non lette: { $error }
conflict-retry = Riprova
conflict-incoming = in ingresso
conflict-column-base = Base (antenato comune)
conflict-column-ours = Nostra ({ $label })
conflict-column-theirs = Loro ({ $label })
conflict-result = Risultato
conflict-blocks = { $n ->
    [0] nessun blocco di conflitto
    [one] 1 blocco di conflitto
   *[other] { $n } blocchi di conflitto
}
conflict-all-theirs = Tutto loro
conflict-all-theirs-hint = Ogni blocco con le righe loro
conflict-all-ours = Tutto nostra
conflict-all-ours-hint = Ogni blocco con le righe nostre
conflict-markers-unreadable = I marcatori non si leggono più ({ $error }): niente scelte per blocco, il testo resta modificabile. «Ricomincia» rilegge il file
conflict-missing = (assente)
conflict-binary = (binario, { $size } byte)
conflict-block = Blocco { $n } (riga { $line }):
conflict-choice-ours = Nostra
conflict-choice-ours-hint = Tiene le righe nostre
conflict-choice-theirs = Loro
conflict-choice-theirs-hint = Tiene le righe loro
conflict-choice-both = Entrambe
conflict-choice-both-hint = Le righe nostre e poi le loro
conflict-choice-base = Base
conflict-choice-base-hint = Le righe dell'antenato comune
conflict-sides-missing = Il file non c'è nel worktree. Si può risolvere solo con una versione intera (il file attuale si salva prima in un backup).
conflict-sides-not-text = Il file non è testo. Si può risolvere solo con una versione intera (il file attuale si salva prima in un backup).
conflict-sides-unreadable = I marcatori di conflitto del file non si leggono. Si può risolvere solo con una versione intera (il file attuale si salva prima in un backup).
conflict-use-ours = Usa nostra
conflict-use-theirs = Usa loro
conflict-use-side-hint = Scrive quella versione nel file e la mette in stage
conflict-wait = Aspettare l'operazione in corso
conflict-side-unavailable = Quella versione manca o non è testo
conflict-confirm-markers = { $n ->
    [one] Contiene ancora marcatori (1 blocco): risolvere comunque? Il file va in stage così com'è
   *[other] Contiene ancora marcatori ({ $n } blocchi): risolvere comunque? Il file va in stage così com'è
}
conflict-confirm-markers-button = Risolvi comunque
conflict-confirm-delete = Il file si elimina dal worktree e l'eliminazione va in stage. Il file attuale si salva prima in un backup
conflict-confirm-delete-button = Elimina il file
conflict-cancel = Annulla
conflict-mark-resolved = Segna come risolto
conflict-mark-resolved-hint = Scrive il risultato nel file e lo mette in stage. Il file com'era si salva prima in un backup
conflict-wait-dialog = Aspettare l'operazione in corso (o chiudere il dialogo)
conflict-delete = Elimina il file…
conflict-delete-hint = Risolve il conflitto eliminando il file: chiede conferma
conflict-resolving = Risoluzione…
