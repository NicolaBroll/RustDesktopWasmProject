# Sottomoduli: la sezione della colonna dei ref, il suo menu, l'aggiornamento e la casella del clone
# (submodule.rs, view/submodule_view.rs, view/clone_view.rs).
# $path: il percorso del sottomodulo nel repository; $id: un id di commit breve.

submodule-section = Sottomoduli ({ $n })
submodule-list-error = Sottomoduli non letti: { $error }
submodule-update-all-hint = Aggiorna tutti: in ogni sottomodulo porta fuori il commit registrato in questo repository (quelli non ancora presenti vengono prima clonati). I sottomoduli con modifiche o con commit su nessun branch restano come sono.
submodule-menu-hint = Aggiorna, apri in una scheda, mostra la cartella

submodule-state-conflict = conflitto
submodule-state-uninitialized = non presente
submodule-state-local-changes = modifiche
submodule-state-different-commit = altro commit
submodule-state-in-sync = aggiornato

submodule-tooltip-path = Sottomodulo “{ $path }”
submodule-tooltip-url = URL: { $url }
submodule-tooltip-no-url = URL: nessuno
submodule-tooltip-recorded = Registrato in questo repository: { $id }
submodule-tooltip-recorded-conflict = Registrato in questo repository: in conflitto
submodule-tooltip-head-detached = Presente: { $id } (HEAD staccata)
submodule-tooltip-head-branch = Presente: { $id } (su un branch)
submodule-tooltip-not-checked-out = Presente: ancora nulla
submodule-tooltip-changes = Ha modifiche a file tracciati
submodule-tooltip-untracked = Ha file non tracciati
submodule-tooltip-operation = { $operation } in corso al suo interno
submodule-tooltip-unreferenced = Il commit presente ha commit su nessun branch, tag o branch remoto: crea prima un branch lì, altrimenti un aggiornamento li lascerebbe indietro
submodule-tooltip-ignored = File ignorati si trovano dove il commit registrato aggiunge file: un aggiornamento li sovrascriverebbe, spostali prima
submodule-tooltip-no-mapping = Assente da .gitmodules: git non può aggiornarlo

submodule-explain-conflict = In conflitto in questo repository: risolvi prima il conflitto
submodule-explain-uninitialized = Non ancora presente: “Inizializza e aggiorna” lo clona
submodule-explain-local-changes = Modifiche dentro il sottomodulo: fanne il commit o lo stash lì prima di aggiornare
submodule-explain-different-commit = Il commit presente non è quello registrato qui: “Aggiorna” porta fuori quello registrato
submodule-explain-in-sync = Già sul commit registrato
submodule-blocked-operation = Un'operazione è in corso dentro il sottomodulo: completala o annullala prima

submodule-update = Aggiorna
submodule-update-hint = Porta fuori il commit registrato in questo repository (HEAD staccata); il precedente è salvato in un ref di backup dentro il sottomodulo
submodule-init = Inizializza e aggiorna
submodule-init-hint = Clona il sottomodulo e porta fuori il commit registrato in questo repository
submodule-open-tab = Apri in una nuova scheda
submodule-open-tab-hint = Apri il sottomodulo come repository nella sua scheda
submodule-show-folder = Mostra nel file manager
submodule-show-folder-hint = Mostra la cartella del sottomodulo nel file manager di sistema
submodule-not-checked-out = Non ancora presente

submodule-update-label = Aggiornamento del sottomodulo { $path }
submodule-init-label = Inizializzazione del sottomodulo { $path }
submodule-update-all-label = Aggiornamento dei sottomoduli
submodule-stop-hint = Ferma l'aggiornamento: i sottomoduli già portati fuori restano come sono
submodule-update-cancelled = Annullato: { $label }
submodule-update-done = Sottomoduli: { $updated } aggiornati, { $unchanged } già aggiornati
submodule-update-refused = { $n ->
    [one] 1 non aggiornato
   *[other] { $n } non aggiornati
}: { $list }

submodule-clone = Con i sottomoduli
submodule-clone-hint = Clona anche i sottomoduli e porta fuori i commit registrati (git clone --recurse-submodules)
