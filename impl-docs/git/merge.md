# Merge e operazione in corso

Parte di [ARCHITETTURA-git.md](../ARCHITETTURA-git.md): principi (§1), regole per ogni chiamata a
git (§5), comandi lenti (§6) e diario (§7) sono lì; qui il contratto dei comandi, il `CliBackend` e
undo e backup di questa funzione. "§n" rimanda sempre al documento principale.

Tipi e controlli comuni ai backend in `crates/engine/src/git/merge.rs` (`MergeRequest`,
`MergeMode`, `MergeOutcome`, `MergeAborted`, `OperationState`, `check_merge_state`,
`check_merging`, `merge_source`, `clean_merge_message`); comandi in
`crates/engine/src/builtin/merge.rs`; `CliBackend` in `crates/git/src/backend/merge.rs`;
`MemoryBackend` in `crates/engine/src/git/memory/merge.rs`.

## Comandi

### `merge.start`

Parametri: `{"rev", "no_ff"?, "ff_only"?, "message"?}`.

Scrive (`writes`), entra nel diario quando il merge si conclude (§7). Unisce `rev` nel branch di
HEAD. `rev`: un branch locale o remoto, un tag, un id (anche abbreviato) o un'altra rev; si risolve
nel commit con `GitBackend::log` (una rev che non c'è → `UnknownRev`) e **a git arriva solo l'id**,
dopo `--end-of-options` (un branch `-x` non diventa un'opzione). Vuota o con NUL → `InvalidParams`.

- **Modo** (da `unmerged_count`): niente da fare se il branch contiene già il commit
  (`up_to_date`: niente scritto, niente backup né diario); fast-forward se il branch non ha commit
  che `rev` non ha, a meno di `no_ff`; altrimenti un commit di merge. `ff_only` con branch
  divergenti → `NotAllowed`; `no_ff` e `ff_only` insieme → `InvalidParams`.
- **Messaggio** del commit di merge: `message` (non vuoto, senza NUL, altrimenti `InvalidParams`),
  altrimenti come `git merge` (`merge_source`): `Merge branch 'x'`, `Merge remote-tracking branch
  'origin/x'`, `Merge tag 'v1'`, `Merge commit '<id>'`, con ` into <branch>` fuori da `main`/
  `master`. Il nome si cerca nell'ordine di git (`refs/<rev>`, `refs/tags/`, `refs/heads/`,
  `refs/remotes/`) e conta solo se punta al commit risolto. Con il fast-forward non serve.
- **Rifiuti** (`NotAllowed`, niente cambia), prima di chiamare il backend e di nuovo nel backend
  sullo stato letto subito prima di scrivere (`check_merge_state`): HEAD staccata (scelta: il
  merge va fatto su un branch; il messaggio suggerisce `branch.checkout` o `branch.create`),
  branch senza commit, un'operazione in corso (anche un altro merge), file in conflitto.
- **Modifiche locali** (scelta, come `remote.pull`): per un commit di merge **qualunque** modifica
  ai file tracciati (in stage o no) → `NotAllowed` che chiede di committarle o metterle da parte
  (`stash.push`); mai `--autostash`. Motivo: un merge fermo in conflitto mescolerebbe le modifiche
  locali con quelle del merge, e `git merge --abort` non sa sempre ricostruirle. I non tracciati
  vanno bene. Il fast-forward accetta modifiche che non toccano i file che cambiano: git rifiuta da
  sé, prima di scrivere, di sovrascrivere le altre (`NotAllowed` con i file, il backup si toglie).
  Sempre rifiutati: file che il merge porta al posto di un non tracciato o **ignorato** (git li
  sovrascriverebbe senza dirlo) e file `--assume-unchanged` diversi fra i due commit.
- **Backup**: prima di scrivere `refs/project/backup/merge-...` sul branch com'era (tolto se poi
  niente cambia).
- **Conflitti** (scelta): il merge **resta in corso** (`MERGE_HEAD`), come in `remote.pull` con
  strategia merge ([sync.md](sync.md); il rebase del pull invece si annulla): non è un errore, il risultato ha `in_progress: true`, i file in `conflicts` (quelli ancora
  non uniti e quelli che git dice in conflitto, anche se `rerere` li ha già risolti) e il testo di
  git in `message`. Il branch non si è mosso (`after == before`) e il backup resta. Si esce con
  `merge.continue` (dopo aver risolto e messo in stage i file con `index.stage`) o con
  `merge.abort`. Anche un merge fermo per un hook `pre-merge-commit` è `in_progress` (senza
  conflitti).

Risultato: `{"rev", "source": {"kind": "branch" | "remote_branch" | "tag" | "commit", "name",
"full": string | null}, "target", "branch", "strategy": "fast_forward" | "merge" | "up_to_date",
"before", "after", "ahead", "behind", "in_progress", "conflicts": [percorso], "message", "backup":
string | null, "warnings", "status" | null, "refs" | null, "can_undo", "can_redo", "next_undo",
"next_redo"}`.

### `merge.continue`

Parametri: `{"message"?}`.

Scrive (`writes`), entra nel diario. Conclude il merge in corso con il commit di merge (genitori
HEAD e `MERGE_HEAD`): passa da `GitBackend::commit` (`git commit -F -`, come `commit.create`: hook
e firma secondo la configurazione, errori uguali, `PostCommit` dopo un commit riuscito). Rifiuti
(`NotAllowed`, niente committato): nessun merge in corso, un'altra operazione in corso, **file
ancora in conflitto** (l'errore li elenca: vanno risolti e messi in stage con `index.stage`).
Messaggio: `message` (non vuoto, senza NUL) o quello preparato da git, `MERGE_MSG`, senza il blocco
`# Conflicts:` / `#\t<file>` che git aggiunge in fondo (`clean_merge_message`: `git commit -F` con
la pulizia `whitespace` non toglie i commenti, che finirebbero nel commit; vale con qualunque
`core.commentChar`); senza `MERGE_MSG`, `Merge commit '<id>'`. Funziona anche per un merge avviato
da terminale. `commit.create` con un merge in corso resta possibile e fa lo stesso commit, ma entra
nel diario come un commit (undo = `reset --soft`); `merge.continue` è il comando da usare.

Risultato: `{"commit": Commit, "previous", "branch", "backup", "also_on", "message", "status" |
null, "refs" | null, "can_undo", "can_redo", "next_undo", "next_redo"}`.

### `merge.abort`

Parametri: `{"confirm": true}`.

Scrive (`writes`), **non** entra nel diario. Annulla il merge in corso (`git merge --abort`):
indice e worktree tornano come HEAD per i file del merge. Le risoluzioni fatte nei file in
conflitto, le modifiche in stage e quelle fatte dopo l'inizio del merge andrebbero perse: chiede
**sempre** `"confirm": true`; senza, `ConfirmRequired` che elenca i file in conflitto e quelli
cambiati, e niente cambia. Prima di annullare il backend salva indice e worktree in
`refs/project/backup/merge-abort-...` (se il backup non riesce non si annulla niente; vedi sotto).
Rifiuti (`NotAllowed`): nessun merge in corso, un'altra operazione (rebase, cherry-pick...), un
file in stage con anche modifiche fuori stage (`reset --merge` lo rifiuterebbe: si controlla prima,
senza backup né scritture, e il messaggio suggerisce di metterle in stage o scartarle; uguale nei
due backend). Se `git merge --abort` (cioè `reset --merge`) fallisce, l'errore è `Git` con il
testo di git e **nomina il backup, che resta sempre**: anche quando git rifiuta, può aver già
riscritto una parte dei file (un file che non si cancella, una cartella in sola lettura, un file
aperto su Windows) con `MERGE_HEAD` e HEAD invariati. Se lo stato cambia tra il controllo e git
(`not uptodate`), l'errore ha in più lo stesso suggerimento.

Risultato: `{"backup", "files": [percorso], "warnings", "status" | null, "refs" | null,
"can_undo", "can_redo", "next_undo", "next_redo"}` (`files`: i file salvati perché diversi da
HEAD).

### `repo.operation`

Parametri: `{}`. Sola lettura: quello che serve alla UI per il banner di un'operazione in corso.

Risultato: `{"operation": "Merge" | "Rebase" | "CherryPick" | "Revert" | "Bisect" | "Am" | null,
"head": Head, "conflicts": [percorso], "incoming": [{"id", "label", "summary": string | null,
"refs": [nome completo]}], "message": string | null, "onto": oid | null, "rebase_branch": string |
null, "ready": bool, "continue": string | null, "abort": string | null}`.

- `operation`: come `Status::operation` (`repo.status`), dai file della cartella git.
- `conflicts`: i file in conflitto dello stato (voci `u`).
- `incoming`: i commit in ingresso (`MERGE_HEAD`, anche più d'uno per un octopus,
  `CHERRY_PICK_HEAD`, `REVERT_HEAD`; vuoto per rebase, bisect, `git am`): `label` il nome breve del
  primo branch locale, branch remoto o tag che ci punta (altrimenti l'id abbreviato), `summary`
  l'oggetto del commit, `refs` i ref (nomi completi) che ci puntano.
- `message`: il messaggio che `merge.continue` userebbe (`MERGE_MSG` senza il blocco dei
  conflitti; anche per cherry-pick e revert, dove git lo prepara).
- `onto`, `rebase_branch`: per un rebase, il commit su cui si riapplica e il branch riscritto
  (`rebase-merge/` o `rebase-apply/`: `onto` e `head-name`; `null` se partito da HEAD staccata).
- `ready`: c'è un'operazione e non restano file in conflitto.
- `continue` / `abort`: gli id dei comandi che concludono o annullano l'operazione da qui:
  `merge.continue` / `merge.abort` per il merge, `cherry_pick.continue` / `cherry_pick.abort` per
  un cherry-pick, `revert.continue` / `revert.abort` per un revert ([cherry-pick.md](cherry-pick.md));
  `null` per le altre (rebase in S32: per ora da terminale) e per una sequenza di cherry-pick o
  revert di più commit avviata da terminale (`.git/sequencer`, campo `sequencer` di
  `OperationState`), che resta al terminale.

Il banner della UI: `operation` non `null` → banner con il tipo, `incoming[].label`, il numero di
`conflicts` (e i file, che si aprono come le modifiche), e i pulsanti `continue` (attivo se `ready`)
e `abort` (in due tempi: senza `confirm` il motore risponde `ConfirmRequired` con il motivo da
mostrare). Rileggere dopo ogni comando che scrive (in particolare `merge.start` con `in_progress`,
`index.stage`, `merge.continue`, `merge.abort`).

## CliBackend

**Merge** (`merge.start`): letto lo stato senza fsmonitor (`check_merge_state`), HEAD sul branch
con il valore atteso, i file `--assume-unchanged` e i file in arrivo al posto di un non tracciato o
ignorato (`diff-tree --diff-filter=A HEAD <oid>` e `symlink_metadata`, come il pull), poi il backup
e, con `GIT_REFLOG_ACTION=project: merge.start` e `GIT_EDITOR=:`:

- fast-forward: `git -c core.fsmonitor=false merge --ff-only --no-squash --commit --no-autostash
  --no-overwrite-ignore --no-edit --end-of-options <oid>` (`--no-squash`/`--commit` contro
  `branch.<nome>.mergeOptions=--squash`, che farebbe uno squash senza spostare il branch);
- commit di merge: `git -c core.fsmonitor=false merge --no-autostash --no-overwrite-ignore
  --no-edit --commit --no-squash --ff|--no-ff -F <file> --end-of-options <oid>`, il messaggio da un
  file temporaneo nella cartella git (git non legge `-F -` da stdin per `merge`; il percorso come
  `OsStr`). `--ff`/`--no-ff`, `--commit`, `--no-squash` espliciti contro `merge.ff` e
  `branch.<nome>.mergeOptions`; `--no-autostash` contro `merge.autoStash`. Hook
  (`pre-merge-commit`, `commit-msg`, `post-merge`) e firma seguono la configurazione: mai
  `--no-verify`.

Dopo: riuscito e senza operazione in corso, ma con il branch ancora sul valore di prima → errore
`Git` che nomina il backup (git ha fatto altro dal merge chiesto); riuscito → il branch dopo
(`ref_value`), con `warnings` se HEAD
non è più sul branch o il branch non contiene il commit unito (un hook o un altro processo); merge
in corso (`MERGE_HEAD`) → `in_progress` con i file non uniti dello stato e quelli dell'output
(`Merge conflict in <file>`, e con `rerere` `Resolved '<file>' using previous resolution`), nessun
`--abort`; altrimenti niente è partito: il backup si toglie (solo se il branch vale ancora il
valore di prima) e l'errore è `NotAllowed` con i file se git ha rifiutato per non sovrascrivere
(`would be overwritten`, `would be removed`, `would lose untracked files`), altrimenti `Git` con
stdout e stderr.

**Abort** (`merge.abort`): stato senza fsmonitor e `check_merging`, indice non bloccato
(`IndexLocked`), poi il backup come uno stash, costruito in indici temporanei (l'indice vero, con i
conflitti, non si tocca; niente split index, fsmonitor o indice sparso):

1. dall'indice vero (`ls-files -s -z`) le voci di stadio 0 e, per i file in conflitto, la versione
   nostra (stadio 2), con `update-index -z --index-info` in un indice vuoto, `write-tree` e
   `commit-tree -p HEAD` (identità fissa dei backup, `--no-gpg-sign`): il commit dell'indice;
2. `read-tree` di quell'albero in un altro indice temporaneo, poi `git --literal-pathspecs add -A
   -f --pathspec-from-file=- --pathspec-file-nul` (`-f`: un conflitto su un percorso ignorato,
   es. un `*.log` tracciato, cancellato da noi e modificato da loro, non blocca il backup) dei file
   in conflitto o cambiati (in stage o no)
   che esistono su disco o nell'indice (i cancellati si tolgono), `write-tree`, `commit-tree -p HEAD
   -p <commit dell'indice>`: il commit del worktree, con i marcatori e le risoluzioni com'erano;
3. `update-ref` del ref nuovo `refs/project/backup/merge-abort-<s>-<ns>` (solo se non esiste).

Si recupera con `git stash apply [--index] <ref>` (ha la forma di uno stash) o file per file con
`git show <ref>:<percorso>`. Poi `git -c core.fsmonitor=false merge --abort` con
`GIT_REFLOG_ACTION=project: merge.abort`; se fallisce (o il merge resta in corso) → `Git` che
nomina il backup, che non si toglie mai: `reset --merge` può aver già riscritto una parte dei file
anche con `MERGE_HEAD` e HEAD invariati. Limite: i non tracciati non sono nel backup (`git merge --abort` non
li tocca); fra il backup e `merge --abort` resta l'istante in cui un altro processo potrebbe
cambiare un file (che `reset --merge` riporterebbe a HEAD senza che sia nel backup).

**Lettura** (`repo.operation`): l'operazione come per `repo.status` (`MERGE_HEAD`,
`rebase-apply/applying`, `rebase-merge`/`rebase-apply`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`,
`sequencer/`, `BISECT_LOG`), poi dalla cartella git di questo worktree: gli id di `MERGE_HEAD` /
`CHERRY_PICK_HEAD` / `REVERT_HEAD` (uno per riga), `MERGE_MSG`, e per il rebase `onto` e
`head-name` di `rebase-merge/` o `rebase-apply/` (`head-name` vale solo se inizia con `refs/`).

**Continue**: `GitBackend::commit` (vedi [commit.md](commit.md)); con `MERGE_HEAD` git crea il
commit di merge e toglie lo stato del merge.

## Undo e backup

- **Merge concluso** (`merge.start` con fast-forward o commit di merge, `merge.continue`): voce
  `JournalEntry::merge` con il branch di HEAD da `before` ad `after`, HEAD uguale prima e dopo, il
  backup `refs/project/backup/merge-...` (solo `merge.start`; per `merge.continue` il commit di
  prima è il primo genitore del merge) e la descrizione `Merge di <nome> in `<branch>`
  (fast-forward | commit di merge: da a1b2c3d a e4f5a6b)`. Undo e redo come il pull del branch
  attivo (vedi [sync.md](sync.md)): `GitBackend::reset_branch`, cioè `git reset --keep` con gli
  stessi controlli (HEAD e branch come li ha lasciati il merge, altrimenti `UndoConflict`; file da
  ripristinare con modifiche locali → `NotAllowed`, la voce resta) e il backup del commit di merge
  abbandonato in `refs/project/backup/undo-...`. `merge.continue` con HEAD staccata (merge avviato
  da terminale) entra nel diario come un commit (`JournalEntry::commit`).
- **Solo un risultato atteso**: `merge.start` registra la voce solo se il branch dopo è il
  commit unito (fast-forward) o un commit di merge che ha per primo genitore il branch di prima;
  altrimenti (un hook `post-merge` o un altro processo l'ha spostato nel frattempo) il merge è
  fatto ma non entra nel diario, con un avviso in `warnings` che nomina il backup: un undo
  riporterebbe indietro anche il lavoro di altri.
- Con un'operazione in corso anche undo e redo di `worktree.discard` sono rifiutati
  (`NotAllowed`), come quelli che spostano ref: riscriverebbero file del merge.
- **Merge fermo in conflitto: niente nel diario** (scelta). Il branch non si è mosso; finché il
  merge è in corso `edit.undo` ed `edit.redo` sono comunque rifiutati (`NotAllowed`, operazione in
  corso), e la voce arriva con `merge.continue`. Così un undo non può mai cadere a metà di un
  merge, e la via d'uscita dal merge in corso è una sola: `merge.abort`.
- **`merge.abort` non entra nel diario** (scelta): rifare un merge annullato vorrebbe dire rifare
  il merge con i conflitti e rimettere le risoluzioni, non uno spostamento di ref. Il recupero è il
  backup `refs/project/backup/merge-abort-...` (sopra). `can_undo` dopo un abort riguarda la voce
  precedente.
- `refs/project/backup/merge-...` di un merge rimasto in corso resta anche dopo `merge.abort`: è
  il branch prima del merge, uguale a HEAD; una futura pulizia dei backup può toglierlo. Lo stesso
  per `refs/project/backup/pull-...` di un pull in merge rimasto in corso.
- **Merge avviato da `remote.pull`**: `merge.continue` e `merge.abort` funzionano allo stesso modo
  (il merge in corso è un normale `MERGE_HEAD`); il messaggio di `merge.continue` è quello del pull
  (`Merge remote-tracking branch 'origin/main'`) e la voce del diario `Merge di origin/main in
  `main``.

Il `MemoryBackend` fa lo stesso merge file per file del pull in memoria (un file cambiato in modo
diverso dai due lati è un conflitto), ma **lascia il merge in corso**: i file uniti in stage, i
conflitti come voci `u` (`BothModified`, `BothAdded`, `DeletedByUs`, `DeletedByThem`) con i
marcatori `<<<<<<< HEAD` / `=======` / `>>>>>>> <nome>` nel worktree se i contenuti sono noti, e
`MERGE_HEAD`/`MERGE_MSG` (con il blocco `# Conflicts:`) in memoria. Il commit con un merge in corso
fa il commit di merge (genitori HEAD e `MERGE_HEAD`, anche senza modifiche); un merge impostato con
`set_operation` non ha `MERGE_HEAD` e il commit resta rifiutato. L'abort salva un commit di backup
(`refs/project/backup/merge-abort-<n>`) con la stessa forma di uno stash (genitori HEAD e un commit
con l'indice; il worktree dei file cambiati) e
riporta tutti i file tracciati a HEAD (più severo di git, che tiene le modifiche fuori stage dei
file che il merge non tocca: qui sono solo nel backup); rifiuta come git un file in stage con
anche modifiche fuori stage. Backup dei merge in `commit_backups` (`refs/project/backup/merge-<n>`).
