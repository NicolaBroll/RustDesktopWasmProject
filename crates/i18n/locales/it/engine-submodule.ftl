# Sottomoduli (`submodule.list`, `submodule.update`, impl-docs/git/submodules.md).

engine-submodule-paths-empty = `paths` deve elencare almeno un sottomodulo, senza percorsi vuoti
engine-submodule-unknown = `{ $path }` non è un sottomodulo né una cartella che ne contiene uno, nulla aggiornato
engine-submodule-not-initialized = `{ $path }` non è inizializzato: aggiornalo con "init"
engine-submodule-no-mapping = `{ $path }` non ha una voce in `.gitmodules`: git non può aggiornarlo
engine-submodule-suspicious-url = l'URL di `{ $path }` inizia con `-`: non viene usato
engine-submodule-conflict = `{ $path }` è in conflitto in questo repository: risolvi prima il conflitto
engine-submodule-operation = c'è un'operazione in corso dentro `{ $path }`: completala o annullala prima
engine-submodule-local-changes = `{ $path }` ha modifiche: fai prima commit o stash dentro il sottomodulo
engine-submodule-untracked = `{ $path }` ha file non tracciati: fai commit, spostali o rimuovili prima
engine-submodule-unreferenced = il commit attivo di `{ $path }` ha commit che non sono su nessun branch, tag o branch remoto: crea prima un branch lì, altrimenti l'aggiornamento li lascerebbe indietro
engine-submodule-not-at-recorded = dopo l'aggiornamento `{ $path }` è su { $head }, non sul commit registrato { $recorded }
engine-submodule-update-failed = { $error } (gli HEAD precedenti dei sottomoduli sono salvati in: { $backups })
