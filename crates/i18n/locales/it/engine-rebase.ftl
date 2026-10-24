# Rebase interattivo (`rebase.plan`, `rebase.start`, `rebase.continue`, `rebase.abort`, S32).
engine-rebase-what = il rebase
engine-rebase-base-empty = `base` deve essere un id di commit, un branch o un tag, non vuoto
engine-rebase-unborn = il branch `{ $name }` non ha ancora commit: `{ $id }` non ha niente da riscrivere, nulla è cambiato
engine-rebase-detached = HEAD è staccata: `{ $id }` lavora su un branch, nulla è cambiato. Passa a un branch (`branch.checkout`) e riprova
engine-rebase-base-is-head = { $base } è HEAD: non ci sono commit da riscrivere sopra di esso (`{ $id }` vuole come `base` un commit precedente)
engine-rebase-not-ancestor = { $base } non è un antenato di HEAD: `{ $id }` riscrive i commit del branch sopra uno dei suoi commit precedenti (per portare dentro altri commit usa `merge.start` o `remote.pull`)
engine-rebase-merges = i commit da riscrivere comprendono commit di merge ({ $commits }): `{ $id }` supporta solo una storia lineare, nulla è cambiato
engine-rebase-too-many = più di { $max } commit da riscrivere: `{ $id }` rifiuta, scegli una `base` più recente
engine-rebase-conflicts = ci sono file in conflitto ({ $files }): risolvili prima di `rebase.start`; nulla è cambiato
engine-rebase-local-changes = ci sono modifiche locali a file tracciati ({ $files }): fai commit o mettile da parte (`stash.push`) e riprova `rebase.start` (niente autostash); nulla è cambiato
engine-rebase-noop = il piano tiene ogni commit com'è, nello stesso ordine: `rebase.start` non cambierebbe niente
engine-rebase-published = il piano riscrive commit già pubblicati ({ $commits }, raggiungibili da branch remoti): chi li ha già scaricati troverebbe una storia diversa e il prossimo push andrebbe forzato. Ripeti con "confirm": true per riscriverli
engine-rebase-head-moved = HEAD è cambiata mentre `rebase.start` veniva preparato: nulla è cambiato, riprova
engine-rebase-range-changed = i commit da riscrivere sono cambiati mentre `rebase.start` veniva preparato: nulla è cambiato, rileggi il piano
engine-rebase-untracked = il rebase scriverebbe file al posto di file non tracciati o ignorati ({ $files }): spostali o rimuovili e riprova; nulla è cambiato
engine-rebase-continue-conflicts = restano file in conflitto ({ $files }): risolvili e mettili in stage (`index.stage`), poi ripeti `rebase.continue`
engine-rebase-terminal = il rebase non è stato avviato dall'app (o i suoi dati mancano o non corrispondono): è concluso, ma non entra nel giornale
engine-rebase-unexpected-result = il branch è a { $after } ma non è il risultato atteso del rebase (HEAD non sul branch, o il branch non si è mosso): l'operazione non entra nel giornale, il branch com'era è in `{ $backup }`
engine-rebase-count-mismatch = il rebase ha prodotto { $found } commit invece dei { $expected } del piano (git scarta un passo risolto in niente): controlla la storia; il branch com'era è in `{ $backup }`
engine-rebase-not-checked = non è stato possibile controllare il risultato del rebase ({ $error })
engine-rebase-dropped-empty = il commit { $commit }, risolto com'era HEAD, è stato scartato (niente da committare), come fa git
engine-rebase-memory-no-state = il rebase in corso non ha dati nel repo in memoria (`set_operation`): non si può continuare
engine-rebase-todo-not-written = non è stato possibile scrivere il piano del rebase ({ $error }): nulla è cambiato
engine-rebase-record-not-written = non è stato possibile salvare i dati del rebase fermo ({ $error }): `rebase.continue` lo concluderà senza voce nel giornale; il branch com'era è in `{ $backup }`
engine-rebase-branch-not-moved = il rebase è finito ma `{ $full }` non si è mosso ({ $text }): controlla la storia; il branch com'era è in `{ $backup }`
engine-rebase-branch-gone = dopo il rebase `{ $full }` non esiste più; il branch com'era è in `{ $backup }`
engine-rebase-messages-not-applied = il rebase è concluso ma { $n } { $n ->
    [one] messaggio
   *[other] messaggi
} del piano non sono stati applicati (è stato continuato fuori dall'app?): controlla la storia; il branch com'era è in `{ $backup }`
engine-rebase-message-skipped = la pausa dopo { $commit } non seguiva il passo dopo cui il piano la metteva: il suo messaggio non è stato applicato, controlla la storia
engine-rebase-abort-untracked-refused = l'annullamento riporterebbe file che ora esistono come non tracciati ({ $files }): git si fermerebbe a metà. Spostali o rimuovili e riprova; nulla è cambiato
engine-rebase-abort-untracked = l'annullamento ha riportato file che erano ignorati ({ $files }): git li ha sovrascritti, sono nel backup
# Errori del piano (`InvalidParams`): `engine-rebase-plan-step` avvolge gli altri con il passo (da 1).
engine-rebase-plan-step = passo { $step }: { $reason }
engine-rebase-plan-unknown-commit = il commit { $commit } non è nell'intervallo da riscrivere
engine-rebase-plan-duplicate-commit = il commit { $commit } compare più di una volta
engine-rebase-plan-missing-commit = manca dal piano il commit { $commit } dell'intervallo (usa `drop` per toglierlo)
engine-rebase-plan-all-dropped = ogni commit è scartato (per toglierli tutti sposta invece il branch)
engine-rebase-plan-nothing-to-combine = il primo commit tenuto non può essere `squash` o `fixup`: non c'è niente con cui combinarlo
engine-rebase-plan-missing-message = `reword` vuole un `message`
engine-rebase-plan-unexpected-message = un `message` è ammesso solo su `reword` e sull'ultimo `squash`/`fixup` di un gruppo
engine-rebase-plan-reword-in-group = `reword` non può aprire un gruppo con `squash`/`fixup`: usa `pick` e metti il messaggio sull'ultimo passo del gruppo
engine-rebase-plan-empty-message = il messaggio è vuoto
engine-rebase-plan-nul-in-message = il messaggio contiene un byte NUL
