engine-discard-changed = i file da scartare sono cambiati dalla conferma (confermati: { $confirmed }; ora: { $now }): niente scartato, rileggere lo stato e confermare di nuovo
engine-discard-nothing = nessuna modifica fuori stage da scartare in { $paths }
engine-discard-submodules = sottomoduli, niente scartato: le modifiche di un sottomodulo si scartano dentro il sottomodulo: { $files }
engine-discard-untracked = file non tracciati, niente scartato: scartarli vorrebbe dire cancellarli, e il backup del worktree (`git stash create`) non li salva: { $files }
engine-discard-in-the-way = file non tracciati o ignorati dove andrebbero i file da scartare: { $files } (ricreare i file li cancellerebbe, e il backup non li contiene). Niente scartato: spostarli o scartare altri percorsi
engine-discard-intent-to-add = ci sono file `intent-to-add` (`git add -N`: { $files }): il backup del worktree (`git stash create`) non si può creare, niente scartato. Metterli in stage (`index.stage`) o toglierli (`index.unstage`) e riprovare
engine-discard-conflicts = ci sono file in conflitto ({ $files }): il backup del worktree (`git stash create`) non si può creare con un indice in conflitto, niente scartato. Risolvere prima i conflitti
engine-discard-unborn = il branch non ha ancora commit: il backup del worktree (`git stash create`) non si può creare, niente scartato
engine-discard-no-paths = nessun percorso: niente da scartare
engine-discard-worktree-changed-redo = `{ $path }`: il file è cambiato dopo l'annullamento dello scarto
engine-discard-worktree-changed = `{ $path }`: il file è cambiato dopo lo scarto
engine-discard-unreadable = `{ $path }` non si legge: { $error }
engine-discard-nothing-saved = `git stash create` non ha trovato modifiche da salvare (il worktree è cambiato nel frattempo?): niente scartato, rileggere lo stato
engine-discard-backup-incomplete-stash = il backup del worktree (`git stash create`, { $stash }) non ha il contenuto attuale di { $files } (un file cambiato proprio adesso?): niente scartato, riprovare
engine-discard-backup-incomplete = il backup del worktree (`{ $backup }`) non ha il contenuto attuale di { $files } (un file cambiato proprio adesso?): niente scartato, riprovare
engine-discard-in-the-way-git = nel worktree { $files } non { $n ->
    [one] è
   *[other] sono
} come se lo aspetta git (un file o un link al posto di una cartella, o una cartella al posto di un file): ricreare i file lo cancellerebbe, e il backup non lo contiene. Niente scritto: spostarlo e ripetere
engine-discard-index-moved = l'indice è cambiato mentre si preparava lo scarto (un altro processo git?): niente scartato, riprovare
engine-discard-no-backup = il backup del worktree non si può creare, niente scartato
engine-discard-backup-saved = il worktree prima dello scarto è salvato in `{ $backup }`
engine-discard-backup-kept = il worktree prima dello scarto resta nel commit { $commit }
engine-discard-rewrite-failed = `{ $path }` non si riscrive ({ $error })
engine-discard-not-as-expected = dopo `{ $command }` { $files } non { $n ->
    [one] è
   *[other] sono
} come atteso (un filtro `clean`/`smudge` o un altro programma?); { $note }
engine-discard-lines-whole-file = `{ $path }` non ha modifiche di righe scartabili una per una (eliminato, rinominato, `intent-to-add`, cambio di tipo o solo cambio di modo): scarta il file intero (`worktree.discard`)
engine-discard-lines-filters = `{ $path }`: i byte sul disco non sono quelli che git scriverebbe per il suo contenuto (un filtro `clean`/`smudge` o una conversione dei fine riga che non torna uguale): nulla scritto, scarta il file intero (`worktree.discard`)
engine-discard-lines-not-a-file = `{ $path }` non è un file normale nel worktree: nulla scritto
engine-discard-lines-changed = il file è cambiato dopo la lettura del suo diff: nulla scritto, ricarica il diff
engine-discard-lines-write-failed = `{ $path }` non si può scrivere ({ $error }); il file prima dello scarto è salvato in `{ $backup }`
engine-discard-lines-not-as-expected = dopo `{ $command }` `{ $path }` non ha i byte scritti (un altro programma?); il file prima dello scarto è salvato in `{ $backup }`
