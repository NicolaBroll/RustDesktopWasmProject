# Refs column and ref menu (view/refs.rs).

refs-local-branches = Branch locali
refs-tags = Tag
refs-section = { $title } ({ $n })
refs-tooltip-head =
    { $name }
    { $target }
    Clic: seleziona il commit
    Clic destro: altre azioni
refs-tooltip-local =
    { $name }
    { $target }
    Clic: seleziona il commit
    Doppio clic: checkout
    Clic destro: altre azioni
refs-tooltip-other =
    { $name }
    { $target }
    Clic: seleziona il commit
    Clic destro: nuovo branch da qui, unisci nel branch attivo
refs-tooltip-stash =
    { $name }
    { $target }
    Clic: seleziona il commit
refs-checkout = Checkout
refs-checkout-hint = Passa a «{ $name }»: le modifiche locali restano, se non toccano file diversi nel branch (altrimenti niente cambia)
refs-already-active = È già il branch attivo
refs-new-branch = Nuovo branch da qui…
refs-new-branch-hint = Crea un branch su { $id } senza passarci
refs-new-tag = Nuovo tag qui…
refs-new-tag-hint = Crea un tag su { $id }
refs-rename = Rinomina…
refs-pull-no-upstream = «{ $name }» non ha un upstream: fare prima Push, che lo imposta al primo push
refs-pull-head-hint = Scarica { $upstream } e lo integra: fast-forward, merge o rebase secondo la configurazione
refs-pull-other-hint = Scarica { $upstream } e porta avanti «{ $name }» senza passarci (solo fast-forward: se è divergente bisogna passarci)
refs-pull = Pull
refs-push = Push
refs-push-hint = Pubblica «{ $name }» su { $upstream }. Mai forzato
refs-push-first-hint = Primo push di «{ $name }»: lo pubblica e imposta l'upstream
refs-delete = Elimina
refs-delete-hint = Elimina il branch. Se è unito si elimina subito; se ha commit che non sono altrove chiede conferma. In ogni caso il commit resta in un backup (refs/project/backup/delete-…) e «Annulla» ricrea il branch
refs-delete-head = È il branch attivo: passare prima a un altro branch per eliminarlo
refs-delete-remote = Elimina dal remoto…
refs-delete-remote-hint = Elimina il branch «{ $branch }» dal remoto «{ $remote }», per tutti quelli che lo usano. Prima chiede conferma; il commit resta in un backup (refs/project/backup/push-delete-…). Se qualcuno lo ha aggiornato dopo il tuo ultimo Fetch, l'eliminazione è rifiutata
refs-current-branch = branch attivo
refs-merge-item = Unisci in «{ $into }»
refs-merge-hint = Unisce «{ $what }» in «{ $into }»: fast-forward se «{ $into }» non ha commit propri, altrimenti un commit di merge. Con conflitti il merge resta in corso (si risolve o si annulla dal riquadro in alto). «Annulla» annulla un merge concluso
refs-merge-no-ff-item = Unisci in «{ $into }» con commit di merge
refs-merge-no-ff-hint = Unisce «{ $what }» in «{ $into }» sempre con un commit di merge, anche dove basterebbe il fast-forward (--no-ff)
refs-filter-hint = Filtra branch e tag
refs-filter-clear = Svuota il filtro (Esc)
refs-none = Nessuno
refs-tag-push = Push
refs-tag-no-remote = Nessun remoto configurato: aggiungerne uno in «Remoti»
refs-tag-push-to = Push su «{ $remote }»
refs-tag-push-hint = Pubblica solo il tag «{ $name }» su «{ $remote }». Mai forzato: se il remoto ha questo tag su un altro valore, non cambia niente
refs-tag-delete = Elimina…
refs-tag-delete-hint = Elimina il tag «{ $name }» qui, dopo una conferma; i remoti non vengono toccati. Viene salvato un backup e «Annulla» lo ricrea
refs-tag-delete-remote = Elimina da «{ $remote }»…
refs-tag-delete-remote-hint = Elimina il tag «{ $name }» dal remoto «{ $remote }», per tutti quelli che lo usano. Prima lo legge dal remoto e chiede conferma; il valore resta in un backup (refs/project/backup/push-delete-…)
