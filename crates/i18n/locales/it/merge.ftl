# Merge, cherry-pick, revert and the operation-in-progress panel (merge.rs, pick.rs, merge_view.rs).
# $name: merge, cherry-pick, revert, rebase (lowercase); $title: the same, capitalized.

merge-noun-merge-commit = commit di merge
merge-noun-commit = commit
merge-terminal-merge = Si conclude da terminale: `git merge --continue` o `git merge --abort`
merge-terminal-squash = Si conclude da terminale: `git commit` o `git reset --merge`
merge-terminal-rebase = Si conclude da terminale: `git rebase --continue` o `git rebase --abort`
merge-terminal-cherry-pick = Si conclude da terminale: `git cherry-pick --continue` o `git cherry-pick --abort`
merge-terminal-revert = Si conclude da terminale: `git revert --continue` o `git revert --abort`
merge-terminal-bisect = Si conclude da terminale: `git bisect reset` quando la ricerca è finita
merge-terminal-am = Si conclude da terminale: `git am --continue` o `git am --abort`
merge-incoming = «{ $label }»
merge-incoming-summary = «{ $label }» ({ $summary })
merge-title = { $name } in corso
merge-title-rebase-branch = { $name } in corso di «{ $branch }»
merge-title-rebase-onto = { $name } in corso su { $onto }
merge-title-rebase-branch-onto = { $name } in corso di «{ $branch }» su { $onto }
merge-title-merge-incoming = { $name } in corso: entra { $incoming }
merge-title-incoming = { $name } in corso: { $incoming }
merge-gate-unborn = Il branch attivo non ha ancora commit
merge-gate-detached = HEAD è staccata: serve un branch attivo. Passare prima a un branch
merge-gate-operation = C'è già un'operazione in corso: concluderla o annullarla dal riquadro in alto
merge-gate-conflicts = Ci sono file in conflitto: risolverli prima
merge-start-label = Merge di «{ $what }» in «{ $into }»
merge-squash-label = Squash di «{ $what }» in «{ $into }»
merge-pick-label = { $title } di «{ $what }» su «{ $into }»
merge-continue-label = Conclusione { $name ->
        [squash] dello
       *[other] del
    } { $name }
merge-abort-check-label = Controllo per l'annullamento { $name ->
        [squash] dello
       *[other] del
    } { $name }
merge-abort-label = Annullamento { $name ->
        [squash] dello
       *[other] del
    } { $name }
merge-refused-title = { $title } non possibile
merge-failed-title = { $title } non riuscito
merge-refused = { $what } non possibile: { $error }
merge-failed = { $what } non riuscito: { $error }. Il repo si rilegge per mostrare com'è adesso
merge-done = { $title } fatto
merge-concluded = { $title } concluso
merge-concluded-but = { $title } concluso, ma: { $error }
merge-concluded-notice = { $title } concluso: { $noun } { $id } creato. «Annulla» lo annulla
merge-concluded-notice-branch = { $title } concluso: { $noun } { $id } creato su «{ $branch }». «Annulla» lo annulla
merge-abort-refused = Annullamento { $name ->
        [squash] dello
       *[other] del
    } { $name } non possibile: { $error }
merge-abort-failed = Annullamento { $name ->
        [squash] dello
       *[other] del
    } { $name } non riuscito: { $error }
merge-aborted = { $title } annullato
merge-aborted-notice = { $title } annullato: indice e worktree salvati prima in { $backup }{ $n ->
        [0] {""}
        [one] {" "}(1 file)
       *[other] {" "}({ $n } file)
    }; si rimettono con `git stash apply { $backup }`. Non si annulla con «Annulla»
merge-warning = attenzione: { $warning }
merge-status-not-reloaded = lo stato non si rilegge: { $error }
merge-refs-not-reloaded = i ref non si rileggono: { $error }
merge-done-but = { $what }, ma { $problems }
merge-commits = { $n } commit
merge-conflict-files = { $n ->
    [one] 1 file in conflitto
   *[other] { $n } file in conflitto
}
merge-stopped-before-commit = Merge di «{ $name }» in «{ $branch }» fermo prima del commit (un hook?): concluderlo con «Continua il merge…» nel riquadro in alto, o annullarlo
merge-stopped = Merge di «{ $name }» in «{ $branch }» fermo: { $files }. Risolverli e segnarli come risolti nell'area di stage, poi «Continua il merge…» nel riquadro in alto (o «Annulla il merge…»)
merge-up-to-date = «{ $branch }» contiene già «{ $name }»: niente da unire
merge-fast-forward = Fast-forward: «{ $branch }» portato avanti di { $commits } fino a «{ $name }» ({ $id }). «Annulla» lo riporta indietro
merge-rebase-stopped = Rebase fermo di nuovo sul commit successivo: { $files }. Risolverli e segnarli come risolti nell'area di stage, poi «Continua il rebase…» nel riquadro in alto (o «Annulla il rebase…»)
merge-rebase-stopped-before-commit = Rebase fermo di nuovo prima di un commit (un hook o la firma?): concluderlo con «Continua il rebase…» nel riquadro in alto, o annullarlo
merge-rebase-stopped-short = Rebase fermo di nuovo
merge-rebase-finished = Rebase concluso: HEAD su { $id }
merge-rebase-finished-branch = Rebase di «{ $branch }» concluso: il branch è su { $id }
merge-rebase-backup = ; il branch com'era è in { $backup }
merge-merged = «{ $name }» unito in «{ $branch }» ({ $commits }): commit di merge { $id }. «Annulla» lo annulla
merge-pick-stopped-before-commit = { $title } di «{ $what }» su «{ $branch }» fermo prima del commit (un hook o la firma?): concluderlo con «Continua { $name ->
        [squash] lo
       *[other] il
    } { $name }…» nel riquadro in alto, o annullarlo
merge-pick-stopped = { $title } di «{ $what }» su «{ $branch }» fermo: { $files }. Risolverli e segnarli come risolti nell'area di stage, poi «Continua { $name ->
        [squash] lo
       *[other] il
    } { $name }…» nel riquadro in alto (o «Annulla { $name ->
        [squash] lo
       *[other] il
    } { $name }…»)
merge-cherry-picked = «{ $what }» applicato su «{ $branch }»: commit { $id }. «Annulla» lo annulla
merge-reverted = Modifiche di «{ $what }» annullate su «{ $branch }» con il commit { $id }. «Annulla» lo toglie

# Operation-in-progress panel.
merge-info-error = Dettagli dell'operazione non letti: { $error }
merge-onto = Su { $onto }
merge-sequence-hint = È una sequenza di più commit avviata da terminale: va conclusa da lì (un commit da qui lascerebbe la sequenza a metà, l'annullamento toglierebbe anche i commit già applicati)
merge-not-supported-hint = L'app non sa concludere o annullare questa operazione: va fatto da terminale
merge-abort-button = Annulla { $name ->
        [squash] lo
       *[other] il
    } { $name }…
merge-abort-hint = Torna a com'era prima { $name ->
        [squash] dello
       *[other] del
    } { $name }: chiede conferma e spiega cosa si perde. Prima salva indice e worktree in un backup ({ $backup })
merge-wait = Aspettare l'operazione in corso (o chiudere il dialogo)
merge-continue-button = Continua { $name ->
        [squash] lo
       *[other] il
    } { $name }…
merge-continue-hint = Crea il { $noun }: si può cambiare il messaggio
merge-continue-hint-rebase = Prosegue con i commit rimanenti del rebase: può fermarsi di nuovo al conflitto successivo
merge-continue-conflicts = Restano file in conflitto: risolverli e segnarli come risolti (stage) nell'area delle modifiche
merge-no-conflicts-continue = Nessun file in conflitto: si può concludere con «Continua { $name ->
        [squash] lo
       *[other] il
    } { $name }…»
merge-no-conflicts = Nessun file in conflitto
merge-conflicts-label = { $n ->
    [one] 1 file in conflitto:
   *[other] { $n } file in conflitto:
}
merge-more-files = e altri { $n }…
merge-more-files-hint = Apre l'area di stage, dove ci sono tutti
merge-click-to-resolve = — un clic apre l'editor per risolverli
merge-file-hint = Apre l'editor dei conflitti del file

# Dialogs: Continue, abort, parent of a merge commit.
merge-finish-heading = Concludi { $name ->
        [squash] lo
       *[other] il
    } { $name }
merge-into-branch = su «{ $branch }»
merge-into-detached = su HEAD staccata ({ $id })
merge-finish-text = Crea il { $noun } { $into } con quello che è in stage. «Annulla» nella barra lo annulla, finché l'app resta aperta.
merge-message-label = Messaggio del { $noun }
merge-create-button = Crea il { $noun }
merge-committing-hint = Commit in corso
merge-write-message = Scrivi il messaggio
merge-close = Chiudi
merge-rebase-continue-text = Git prosegue con i commit rimanenti del rebase: il commit su cui si è fermato si crea con quello che è in stage e tiene il suo messaggio. Può fermarsi di nuovo al conflitto successivo.
merge-rebase-continue-text-branch = Git prosegue con i commit rimanenti del rebase di «{ $branch }»: il commit su cui si è fermato si crea con quello che è in stage e tiene il suo messaggio. Può fermarsi di nuovo al conflitto successivo; a rebase concluso, «Annulla» nella barra riporta indietro il branch.
merge-rebase-continue-button = Continua { $name ->
        [squash] lo
       *[other] il
    } { $name }
merge-continuing = Prosecuzione…
merge-committing = Commit…
merge-abort-heading = Annullare { $name ->
        [squash] lo
       *[other] il
    } { $name }?
merge-abort-text-rebase = Il rebase in corso si annulla: branch, indice e file tornano come prima del rebase; i commit riscritti finora si scartano (il backup li mantiene raggiungibili).
merge-abort-text = { $name ->
        [squash] Lo
       *[other] Il
    } { $name } in corso si annulla: indice e file tornano come prima { $name ->
        [squash] dello
       *[other] del
    } { $name }, il branch non si muove.
merge-what-happens = Cosa succede:
merge-abort-backup = «Annulla» nella barra in alto non lo ripristina: per riavere le risoluzioni fatte resta il backup { $backup } (git stash apply <backup>).
merge-abort-confirm = Annulla { $name ->
        [squash] lo
       *[other] il
    } { $name }
merge-dont-abort = Non annullare
merge-mainline-heading = { $title } di un commit di merge
merge-mainline-text-cherry-pick = «{ $what }» ha { $n } genitori. Si applica su «{ $into }» le modifiche che il merge ha portato rispetto al genitore scelto: di solito il primo (il branch in cui si è unito).
merge-mainline-text-revert = «{ $what }» ha { $n } genitori. Si annulla su «{ $into }» le modifiche che il merge ha portato rispetto al genitore scelto: di solito il primo (il branch in cui si è unito).
merge-mainline-hint = Con il genitore { $n } (mainline)

# Squash merge (S58): messaggi della barra di stato.
merge-squash-stopped-before-commit = Squash di «{ $name }» in «{ $branch }» fermo prima del commit (un hook o la firma?): concluderlo con «Continua il squash…» nel riquadro in alto, o annullarlo
merge-squash-stopped = Squash di «{ $name }» in «{ $branch }» fermo: { $files }. Risolverli e segnarli come risolti nell'area di stage, poi «Continua il squash…» nel riquadro in alto (o «Annulla il squash…»)
merge-squash-nothing = Niente da unire con squash: le modifiche di «{ $name }» sono già su «{ $branch }». Non è stato scritto nulla
merge-squashed = «{ $name }» unito con squash in «{ $branch }» ({ $commits }): un solo commit { $id }. «{ $name }» non risulta unito. «Annulla» lo annulla

# Finestra del merge (S58): modo e messaggio dello squash.
merge-start-heading = Unisci «{ $what }» in «{ $into }»
merge-mode-auto = Merge (fast-forward se possibile)
merge-mode-merge-commit = Commit di merge
merge-mode-squash = Squash in un solo commit
merge-mode-ff-only = Solo fast-forward
merge-mode-auto-text = Fast-forward se «{ $into }» non ha commit propri, altrimenti un commit di merge.
merge-mode-merge-commit-text = Crea sempre un commit di merge con due genitori, anche dove basterebbe il fast-forward (--no-ff).
merge-mode-squash-text = Le modifiche di «{ $what }» diventano un solo commit ordinario su «{ $into }» (--squash).
merge-mode-ff-only-text = Porta avanti «{ $into }» fino a «{ $what }»; rifiutato se «{ $into }» ha commit propri (--ff-only).
merge-squash-not-merged = «{ $what }» non risulterà unito: unirlo di nuovo più avanti può dare conflitti sulle stesse modifiche.
merge-squash-message-label = Messaggio del commit
merge-squash-message-hint = Vuoto: «Squashed branch '{ $what }'» seguito dall'elenco dei commit uniti
merge-start-button = Unisci
merge-squash-button = Squash
merge-start-undo = Con conflitti resta in corso (si conclude o si annulla dal riquadro in alto). «Annulla» annulla un merge concluso.
