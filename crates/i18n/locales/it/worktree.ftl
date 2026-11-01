# Worktree collegati: la sezione della colonna dei ref, il suo menu, la finestra "Nuovo worktree" e
# la rimozione (worktree.rs, view/worktree_view.rs, view/refs.rs).
# $path: la cartella di un worktree; $branch, $name: il nome di un branch; $id: un id breve di commit.

worktree-section = Worktree ({ $n })
worktree-list-error = Worktree non letti: { $error }
worktree-new-hint = Nuovo worktree: un'altra cartella con un checkout di questo repository, su un altro branch
worktree-menu-hint = Apri in una nuova scheda, mostra la cartella, rimuovi
worktree-new-from-branch = Nuovo worktree da questo branch…
worktree-new-from-branch-hint = Fa il checkout di “{ $name }” in un'altra cartella, per lavorarci senza lasciare il branch aperto qui (un nuovo branch da esso se ha già un checkout)

worktree-badge-current = qui
worktree-badge-locked = bloccato
worktree-badge-prunable = mancante
worktree-bare = repository bare
worktree-detached = staccato su { $id }
worktree-no-commit = ancora nessun commit
worktree-tooltip-main = Worktree principale
worktree-tooltip-current = Aperto in questa scheda
worktree-tooltip-locked = Bloccato (git worktree lock): non si può rimuovere da qui
worktree-tooltip-locked-reason = Bloccato ({ $reason }): non si può rimuovere da qui
worktree-tooltip-prunable = La sua cartella non esiste più: ne resta solo la voce
worktree-bare-hint = Il repository bare non ha file da aprire

worktree-open-tab = Apri in una nuova scheda
worktree-open-tab-hint = Apre questo worktree in una scheda tutta sua
worktree-open-current = Apri in una nuova scheda
worktree-open-current-hint = Già aperto in questa scheda
worktree-show-folder = Mostra nel file manager
worktree-show-folder-hint = Mostra la cartella di questo worktree nel file manager del sistema
worktree-remove = Rimuovi…
worktree-remove-hint = Elimina la cartella di questo worktree, dopo una conferma; rifiutato se ha modifiche, file non tracciati o ignorati
worktree-remove-main = Il worktree principale non viene mai rimosso
worktree-remove-current = Questo worktree è aperto qui: aprine un altro per rimuoverlo

worktree-add-title = Nuovo worktree
worktree-add-explanation = Un worktree è un'altra cartella con un checkout di questo repository: commit, branch e stash sono condivisi, i file e il branch corrente sono suoi. Si apre in una nuova scheda.
worktree-add-path = Cartella
worktree-add-path-hint = /percorso/della/nuova/cartella
worktree-add-browse-hint = Scegli la cartella del nuovo worktree (assente o vuota)
worktree-add-checkout = Checkout di
worktree-mode-existing = Branch esistente
worktree-mode-existing-hint = Un branch locale che non ha il checkout in nessun worktree
worktree-mode-new = Nuovo branch
worktree-mode-new-hint = Un nuovo branch locale, creato al punto di partenza
worktree-mode-detached = Commit staccato
worktree-mode-detached-hint = Il punto di partenza su un HEAD staccato, senza branch
worktree-add-branch = Branch
worktree-add-branch-pick = Scegli un branch
worktree-add-branch-hint = Solo i branch senza checkout in un worktree
worktree-add-no-free-branch = Ogni branch locale ha il checkout in un worktree
worktree-add-new-branch = Nome
worktree-add-new-branch-hint = nome del nuovo branch
worktree-add-start = Punto di partenza
worktree-add-start-hint = HEAD (o un branch, tag, commit)
worktree-add-rule = La cartella non deve esistere o deve essere vuota; un percorso relativo parte dalla cartella di questo repository.
worktree-add-submit = Crea
worktree-add-running = Creazione del worktree…
worktree-add-need-path = Scegli la cartella
worktree-add-need-branch = Scegli il branch
worktree-add-need-name = Scrivi il nome del nuovo branch
worktree-add-label = Nuovo worktree
worktree-added-branch = Worktree “{ $path }” creato su { $branch }
worktree-added-new-branch = Worktree “{ $path }” creato sul nuovo branch { $branch }
worktree-added-detached = Worktree “{ $path }” creato, staccato su { $id }

worktree-remove-check-label = Verifica del worktree { $path }
worktree-remove-label = Rimozione del worktree { $path }
worktree-remove-title = Rimuovere questo worktree?
worktree-remove-body = La sua cartella viene eliminata e git dimentica il worktree. Il suo branch resta nel repository; si può fare di nuovo il checkout con un nuovo worktree.
worktree-remove-button = Rimuovi worktree
worktree-remove-refused-title = Il worktree non si può rimuovere
worktree-close = Chiudi
worktree-removed = Worktree “{ $path }” rimosso
worktree-removed-backup = Worktree “{ $path }” rimosso; i suoi commit su nessun branch sono salvati in { $backup }
