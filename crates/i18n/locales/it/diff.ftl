# Diff view and staging of hunks and lines (diff.rs, diff_view.rs, lines.rs).

diff-submodule-badge = [sottomodulo]
diff-note-mode = modo { $old } -> { $new }
diff-note-new-file = nuovo file, modo { $mode }
diff-note-deleted-file = file eliminato, modo { $mode }
diff-note-submodule = sottomodulo: il diff mostra il commit a cui punta
diff-reason-unmerged = file in conflitto: il diff non è disponibile finché il conflitto non è risolto
diff-reason-binary = file binario: il contenuto non si mostra
diff-reason-exact-rename = rinomina esatta: contenuto invariato
diff-reason-exact-copy = copia esatta: contenuto invariato
diff-reason-empty-file = file vuoto
diff-reason-empty-file-deleted = file vuoto eliminato
diff-reason-mode-only = solo cambio di modo: contenuto invariato
diff-reason-no-change = nessuna modifica al contenuto
diff-loading = Caricamento del diff di { $path }…
diff-retry = Riprova
diff-empty-worktree = Nessuna differenza rispetto all'indice. Un file non tracciato non ha diff finché non è in stage: dopo lo stage il contenuto compare tra le modifiche in stage.
diff-empty = Nessuna differenza per questo file.
diff-back = < Grafo
diff-back-hint = Chiudi il diff e torna al grafo (Esc)
diff-in-commit = nel commit { $id }
diff-unstaged = non in stage
diff-unstaged-hint = Modifiche nel worktree rispetto all'indice
diff-staged = in stage
diff-staged-hint = Modifiche nell'indice rispetto all'ultimo commit (HEAD)
diff-partial-unavailable = Stage per hunk e per riga non disponibile
diff-stage-selected = Stage righe selezionate ({ $n })
diff-unstage-selected = Unstage righe selezionate ({ $n })
diff-stage-selected-hint = Mette in stage solo le righe selezionate; il resto del file resta fuori
diff-unstage-selected-hint = Toglie dallo stage solo le righe selezionate; il worktree non cambia
diff-no-lines-selected = Nessuna riga selezionata: clic su una riga + o - per selezionarla
diff-wait-write = Attendere la fine della scrittura in corso
diff-clear = Deseleziona
diff-clear-hint = Deseleziona tutte le righe (Esc)
diff-select-hint = Clic su una riga + o - per selezionarla, Maiusc+clic per un intervallo, trascina per più righe
diff-stage-hunk = Stage hunk
diff-unstage-hunk = Unstage hunk
diff-stage-hunk-hint = Mette in stage tutte le righe cambiate di questo hunk
diff-unstage-hunk-hint = Toglie dallo stage tutte le righe cambiate di questo hunk; il worktree non cambia
diff-line-cut = {" "}… (altri { $n } caratteri)
diff-lines-type-changed = il file ha cambiato tipo (ad esempio da file a link): le righe non si possono scegliere, si sposta solo il file intero dall'area di stage
diff-lines-label-stage-hunk = Stage dell'hunk { $hunk } di { $path }
diff-lines-label-unstage-hunk = Unstage dell'hunk { $hunk } di { $path }
diff-lines-label-stage-lines = { $n ->
    [one] Stage di 1 riga di { $path }
   *[other] Stage di { $n } righe di { $path }
}
diff-lines-label-unstage-lines = { $n ->
    [one] Unstage di 1 riga di { $path }
   *[other] Unstage di { $n } righe di { $path }
}
