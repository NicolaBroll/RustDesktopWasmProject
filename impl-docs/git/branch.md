# Branch

Parte di [ARCHITETTURA-git.md](../ARCHITETTURA-git.md): principi (§1), regole per ogni chiamata a
git (§5), comandi lenti (§6) e diario (§7) sono lì; qui il contratto dei comandi, il `CliBackend` e
undo e backup di questa funzione. "§n" rimanda sempre al documento principale.

## Comandi

### `branch.create`

Parametri: `{"name", "start"?}`.

Crea `refs/heads/<name>` su `start` (una rev; HEAD se manca, errore `NotAllowed` se HEAD non ha
commit) senza cambiare branch e senza upstream; mai sopra un ref esistente (`NotAllowed`). Nome
nuovo controllato (`check_new_branch_name` e, nel `CliBackend`, `git check-ref-format --branch`, che
deve restituire il nome identico): niente `-` iniziale, `HEAD`, `@`, nomi completi `refs/...` →
`InvalidParams`. Risultato: `{"branch", "target", "refs", "can_undo", "can_redo", "next_undo",
"next_redo"}` (vedi "Undo e backup" per tutti i `branch.*`).

### `branch.checkout`

Parametri: `{"name"}`.

Passa al branch locale `name` (`git switch`, mai forzato). Risultato: `{"branch", "previous":
HeadState, "changed", "warnings", "status", ...diario}`. Modifiche locali che verrebbero
sovrascritte → `CheckoutConflict { target, files, stderr }` e niente cambia; operazione in corso o
file in conflitto → `NotAllowed`; già sul branch: `changed: false`.

### `branch.delete`

Parametri: `{"name", "confirm"?, "expected"?}`.

`expected`: id **completo** (40 cifre, altrimenti `InvalidParams`) del commit su cui il branch è
stato mostrato; se il branch punta altrove → `NotAllowed` ("si è spostato da quando è stato visto:
... ora punta a ...") prima della conferma e dell'eliminazione, niente cambia: la conferma vale solo
per il commit visto (senza `expected` nessun controllo in più). Branch non unito (commit non
raggiungibili né da HEAD né dal suo upstream) → `ConfirmRequired` senza `confirm`; branch di HEAD,
aperto in un altro worktree, operazione in corso → `NotAllowed`. Backup sempre. Risultato:
`{"branch", "target", "unmerged", "backup", "removed_config": [{"key", "value"}], "warnings",
"refs", ...diario}`.

### `branch.rename`

Parametri: `{"name", "new_name"}`.

`git branch -m` (mai `-M`): reflog, upstream e HEAD seguono; nome nuovo controllato come in
`create`, mai sopra un branch esistente (`NotAllowed`). Risultato: `{"branch", "previous", "target",
"head", "warnings", "refs", ...diario}`.

## Undo e backup

I comandi `branch.*` ricevono nomi **brevi** (`feature/x`); il nome completo è sempre
`refs/heads/<nome>`. Un branch esistente si indica anche con un nome che git non accetterebbe
più per un branch nuovo (es. `-x`, creato con `update-ref`; basta un nome di ref valido), così
si può ancora passarci, rinominarlo o eliminarlo. Al `CliBackend` i nomi arrivano solo come
`refs/heads/<nome>` (`update-ref`) o dopo `--end-of-options` (`switch`, `branch -m`): un
branch che si chiama `--force` o `-d` non diventa mai un'opzione (test con nomi così).

- **`branch.create`**: `update-ref --no-deref -m "project: branch.create ..." refs/heads/<nome>
  <id> ""` (valore vecchio vuoto: si crea solo se non esiste, anche se un altro processo l'ha
  creato nell'istante prima; `--no-deref`: un ref simbolico `refs/heads/<nome>`, anche verso un
  ref inesistente, non si segue, quindi non si crea un altro ref). Nome già preso, ref simbolico
  o un ref che lo contiene o ne è contenuto (`a` per `a/b`: "exists; cannot create") →
  `NotAllowed` con il messaggio di git. Anche `update-ref -d` di `branch.delete` usa
  `--no-deref`. Nessun upstream (`branch.autoSetupMerge` non si applica). Voce del
  diario: il branch da assente a `<id>`; undo = eliminazione con il valore atteso, redo =
  creazione solo se ancora assente. Nessun backup: il commit esisteva già.
- **`branch.checkout`**: `git -c core.fsmonitor=false switch --no-guess --no-overwrite-ignore
  --end-of-options <nome>` (con HEAD staccata `--detach <id>`, solo per l'undo). Mai `--force`,
  `--discard-changes`, `--merge`. Git controlla prima di scrivere: un file modificato (in stage
  o no) diverso nella destinazione, un non tracciato o un ignorato che la destinazione contiene
  (`--no-overwrite-ignore`: di suo git sovrascrive gli ignorati senza dirlo) fanno fallire il
  comando senza cambiare niente → `CheckoutConflict` con i file elencati da git (righe con il
  tab nello stderr, `LC_ALL=C`). Riconosciuti: "would be overwritten by" (checkout, e merge per
  i file dei sottomoduli con `submodule.recurse`), "would be removed by", "would lose untracked
  files" (una cartella con non tracciati che nella destinazione è un file), "would be lost". Le altre modifiche restano, come in git. Rifiutati prima: merge,
  rebase, cherry-pick, revert, bisect, `git am` in corso, file in conflitto, e file
  `--assume-unchanged` diversi fra HEAD e la destinazione (git li sovrascriverebbe senza
  controllarli). `core.fsmonitor=false` perché un fsmonitor non aggiornato farebbe sembrare
  pulito un file modificato. Dopo, HEAD si rilegge: se non è quella chiesta (hook
  `post-checkout`), `warnings`.
- **Il checkout entra nel diario** (scelta): GitKraken e il nostro diario trattano il cambio di
  branch come un'operazione, e senza la voce l'undo di un commit fatto prima di un checkout
  sarebbe rifiutato finché non si torna a mano sul branch. La voce non ha ref (nessun ref si
  sposta), solo HEAD prima e dopo. L'undo **non** usa `update-ref` (`RefRestore::check`
  continua a rifiutare un cambio di branch di HEAD: lascerebbe il worktree del branch di
  prima), ma un altro checkout altrettanto sicuro (`GitBackend::checkout` verso HEAD prima,
  branch o commit staccato), solo se HEAD è ancora quella lasciata dal checkout
  (`UndoConflict` altrimenti); se le modifiche locali lo impedirebbero, `CheckoutConflict` e la
  voce resta. Il redo allo stesso modo. Non entra nel diario il checkout che lascia un branch
  senza commit: tornarci vorrebbe dire svuotare indice e worktree (`switch --orphan`). Il
  checkout del branch già attivo non fa niente e non entra.
- **`branch.delete`**: con `expected` (id completo) il branch deve puntare ancora a quel commit,
  controllato prima di calcolare la conferma e di eliminare (`NotAllowed` altrimenti): la UI lo
  passa con il commit mostrato, così `confirm` non elimina un branch spostato da un altro
  processo nel frattempo. Il backend poi elimina solo se il ref vale ancora quell'id
  (`update-ref -d <ref> <id>`). Rifiutato per il branch di HEAD, per un branch aperto in un altro
  worktree (`git worktree list --porcelain`) e durante un'operazione in corso. "Unito" come per
  `git branch -d`: nessun commit fuori da HEAD e dall'upstream, se il ref dell'upstream c'è
  (`rev-list --count <tip> ^HEAD ^<upstream>`, senza replace object); altrimenti serve
  `confirm` (`ConfirmRequired` con il numero di commit). Sempre, anche se unito, prima un
  backup `refs/project/backup/delete-<secondi>-<nanosecondi>` (creato solo se non esiste, come
  gli altri): l'eliminazione cancella anche il reflog del branch, e il backup tiene il commit
  per l'undo anche dopo un riavvio o un `gc`. Poi `update-ref -d refs/heads/<nome> <id>` (solo
  se vale ancora `<id>`; se fallisce il backup si toglie) e `git config --remove-section
  branch.<nome>` come `git branch -d` (upstream e descrizione; un errore diverso da "no such
  section" va in `warnings`). Prima di toglierla la si legge (`git config --local -z
  --get-regexp '^branch\.<nome con escape>\.[^.]+$'`: solo il file del repo, niente
  configurazione globale né `include`; un valore non UTF-8 non si salva e lo dice un avviso):
  il risultato la riporta in `removed_config` e la voce del diario in `branch_config`
  (`[{"key", "value"}]`, `value` `null` per una chiave senza valore). Voce: il branch da `<id>`
  ad assente, con il backup; undo = lo ricrea su `<id>` solo se nessuno ha creato nel frattempo
  un branch con quel nome (`UndoConflict`), poi riscrive la configurazione (`git config --local
  --add --end-of-options <chiave> <valore>`, una chiave senza valore come `true`) solo se il
  repo non ha già voci `branch.<nome>.*`, altrimenti la lascia e lo dice in `warnings`; redo =
  eliminazione con il valore atteso (nessun nuovo backup: c'è già) e poi la sezione tolta solo
  se è esattamente quella salvata (una diversa resta, con un avviso). La configurazione non è
  sotto lock come i ref: un problema lì dà avvisi, non annulla lo spostamento del ref.
  L'eliminazione cancella anche il **reflog** del branch (come `git branch -d`) e l'undo non lo
  rimette: il backup tiene solo l'ultimo commit (e quindi la sua storia), non le posizioni
  precedenti del branch; resta la voce nel reflog di HEAD, se il branch era stato attivo. `restore_refs` del
  `CliBackend` rifiuta (`NotAllowed`) di cancellare un branch aperto in un altro worktree.
  Limite: un branch fermo in un rebase o bisect di un altro worktree (lì HEAD è staccata) non
  risulta aperto; il commit resta comunque nel backup.
- **`branch.rename`**: `git branch -m --end-of-options <vecchio> <nuovo>` dopo aver controllato
  che il vecchio valga l'id letto e che il nuovo non esista (mai `-M`): a differenza di
  `update-ref`, `git branch -m` sposta anche reflog e sezione `branch.<nome>` e aggiorna HEAD
  (anche di altri worktree). Voce: vecchio da `<id>` ad assente, nuovo da assente a `<id>`, HEAD
  prima e dopo (cambia se era il branch attivo). Undo e redo non passano da `restore_refs` ma
  da `GitBackend::rename_branch` all'inverso, dopo gli stessi controlli (operazione in corso,
  HEAD e i due ref come li ha lasciati l'operazione, altrimenti `UndoConflict`). Limite: l'undo
  della rinomina di un branch dal nome che git non accetta più (es. `-x` → `y`) è rifiutato,
  perché il nome nuovo dell'undo (`-x`) non passa il controllo dei nomi nuovi.

Il `MemoryBackend` fa gli stessi rifiuti (il checkout è più severo di git: rifiuta qualsiasi
modifica a un file diverso fra HEAD e la destinazione, e un non tracciato o ignorato che è il
file, una cartella di un file o un file al posto di una cartella della destinazione); come
configurazione ha solo l'upstream (`branch.<nome>.remote`/`merge`), e registra i backup delle eliminazioni in
`commit_backups` (`refs/project/backup/delete-<n>`).
