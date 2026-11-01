# Worktree collegati (`worktree.list`, `worktree.add`, `worktree.remove`, impl-docs/git/worktrees.md).

engine-worktree-path-newline = il percorso del nuovo worktree non deve contenere un a capo
engine-worktree-branch-and-new = indica `branch` (un branch esistente) o `new_branch` (uno nuovo), non entrambi
engine-worktree-branch-and-start = `start` va con `new_branch` o con un worktree in HEAD staccato, non con un `branch` esistente
engine-worktree-branch-in-use = `{ $branch }` è già attivo nel worktree `{ $path }`: nulla creato
engine-worktree-branch-not-rolled-back = impossibile creare il worktree ({ $error }) e impossibile eliminare di nuovo il branch `{ $name }` appena creato ({ $rollback }): eliminalo a mano se non serve
engine-worktree-unknown = `{ $path }` non è un worktree di questo repository, nulla rimosso
engine-worktree-main = `{ $path }` è il worktree principale: non viene mai rimosso
engine-worktree-current = `{ $path }` è il worktree aperto qui: apri un altro worktree per rimuoverlo
engine-worktree-locked = `{ $path }` è bloccato: sbloccalo prima con `git worktree unlock`
engine-worktree-locked-reason = `{ $path }` è bloccato ({ $reason }): sbloccalo prima con `git worktree unlock`
engine-worktree-operation = in `{ $path }` è in corso un'operazione ({ $operation }): completala o annullala lì prima
engine-worktree-changes = `{ $path }` ha { $count ->
    [one] un file modificato
   *[other] { $count } file modificati
} (`{ $first }`): fai commit o stash lì prima
engine-worktree-untracked = `{ $path }` ha { $count ->
    [one] un file non tracciato
   *[other] { $count } file non tracciati
} (`{ $first }`): fai commit, spostali o rimuovili lì prima
engine-worktree-ignored = `{ $path }` ha { $count ->
    [one] un file ignorato
   *[other] { $count } file ignorati
} (`{ $first }`), che git eliminerebbe senza backup: spostali o rimuovili prima
engine-worktree-hidden = `{ $path }` ha { $count ->
    [one] un file
   *[other] { $count } file
} marcati assume-unchanged o skip-worktree (`{ $first }`): git non sa dire se sono cambiati; togli prima il flag lì
engine-worktree-not-worktree = la cartella `{ $path }` non contiene più quel worktree (`.git` mancante o che punta altrove): nulla rimosso
engine-worktree-remove-confirm = il worktree `{ $path }` e la sua cartella verrebbero eliminati (non ha modifiche, file non tracciati o ignorati){ $backup ->
    [yes] ; il suo HEAD staccato ha commit su nessun branch, tag o branch remoto: verrebbero prima salvati in `refs/project/backup/worktree-...`
   *[other] {""}
}. Ripeti con "confirm": true
engine-worktree-changed = `{ $path }` è cambiato mentre si preparava la rimozione: controllalo e riprova
engine-worktree-remove-failed = { $error } (l'HEAD staccato del worktree è salvato in `{ $backup }`)
engine-worktree-add-hook = il worktree `{ $path }` è stato creato, ma git ha segnalato un errore dopo (un hook `post-checkout`?): { $message }
engine-worktree-add-unexpected = il nuovo worktree `{ $path }` non è su `{ $expected }` come richiesto ma su `{ $found }` (vuoto: non è elencato): un altro processo l'ha cambiato nel frattempo?
engine-worktree-path-registered = `{ $path }` è già registrato come worktree la cui cartella non esiste più: rimuovi prima quella voce (worktree.remove), nulla creato
engine-worktree-unreachable = la cartella di `{ $path }` non è leggibile ({ $reason }): non viene considerata sparita, nulla rimosso
