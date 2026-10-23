# Cherry-pick e revert

Parte di [ARCHITETTURA-git.md](../ARCHITETTURA-git.md) (§1, §5, §7); il modello è il merge di
[merge.md](merge.md), di cui qui si dice solo cosa cambia.

Tipi e controlli comuni ai backend in `crates/engine/src/git/pick.rs` (`PickKind`, `PickRequest`,
`pick_parent`, `pick_message`, `check_pick_state`, `check_picking`, `pick_empty_error`); comandi
in `crates/engine/src/builtin/pick.rs`; `CliBackend` in `crates/git/src/backend/pick.rs` (l'abort
in `backend/merge.rs`, comune al merge); `MemoryBackend` in `crates/engine/src/git/memory/pick.rs`.

## Comandi

### `commit.cherry_pick`, `commit.revert`

Parametri: `{"rev", "mainline"?: n}`.

Scrivono (`writes`), entrano nel diario quando l'operazione si conclude. Un commit alla volta:
il cherry-pick applica le modifiche di `rev` sopra il branch di HEAD con un commit nuovo
(**autore e messaggio originali**, senza `-x`); il revert ne applica le modifiche inverse
(messaggio di git: `Revert "<oggetto>"` + riga vuota + `This reverts commit <id>.`; per un commit
di merge `This reverts commit <id>, reversing\nchanges made to <genitore>.`; il revert di un
revert è `Reapply "..."`, come git dalla 2.43).

- `rev` si risolve con `GitBackend::log` (non c'è → `UnknownRev`); a git arriva solo l'id dopo
  `--end-of-options`. Vuota o con NUL → `InvalidParams`.
- `mainline`: un commit di merge lo vuole (da 1 al numero dei genitori; senza → `InvalidParams`
  che lo chiede); su un commit non di merge (anche se git lo accetterebbe) o fuori range →
  `InvalidParams`.
- **Rifiuti** (`NotAllowed`, niente cambia) prima del backend e di nuovo nel backend sullo stato
  letto subito prima di scrivere (`check_pick_state`): HEAD staccata, branch senza commit,
  operazione in corso, file in conflitto, **qualunque** modifica ai file tracciati (in stage o
  no: committarle o `stash.push`, mai autostash, come il commit di merge). Sempre rifiutati: file
  che la patch scrive al posto di un non tracciato o ignorato e file `--assume-unchanged` che la
  patch cambia.
- **Già applicato / risultato vuoto** → `NotAllowed` ("non cambierebbe niente"): nessun commit,
  niente in corso (nessun `CHERRY_PICK_HEAD`), backup tolto.
- **Backup**: prima di scrivere `refs/project/backup/cherry-pick-...` / `revert-...` sul branch
  com'era (tolto se niente cambia).
- **Conflitti**: l'operazione **resta in corso** (`CHERRY_PICK_HEAD` / `REVERT_HEAD`), risultato
  con `in_progress: true`, `conflicts`, `message` (testo di git); il branch non si è mosso,
  niente diario, il backup resta. Anche un commit fallito per firma o hook resta in corso (senza
  conflitti, le modifiche in stage).

Risultato: `{"rev", "commit", "summary", "mainline", "branch", "before", "after", "in_progress",
"conflicts": [percorso], "message", "backup", "warnings", "status" | null, "refs" | null,
"can_undo", "can_redo", "next_undo", "next_redo"}`.

### `cherry_pick.continue`, `revert.continue`

Parametri: `{"message"?}`. Come `merge.continue`: `GitBackend::commit` con
`CommitRequest::concluding(op)` (l'unico modo di passare `check_commit_state` con un cherry-pick
o revert in corso; `commit.create` resta rifiutato). Con `CHERRY_PICK_HEAD` git tiene l'autore
originale. Messaggio: `message`, altrimenti `MERGE_MSG` senza il blocco `# Conflicts:`,
altrimenti `pick_message` del commit in ingresso. Rifiuti (`NotAllowed`): nessuna operazione di
quel tipo in corso (anche durante un merge o l'altro tipo), sequenza da terminale (sotto), file
ancora in conflitto, risultato uguale a HEAD (risolto tenendo la versione nostra: niente da
committare, l'operazione resta in corso e il messaggio indica `*.abort`). Risultato come
`merge.continue`.

### `cherry_pick.abort`, `revert.abort`

Parametri: `{"confirm": true}`. Come `merge.abort`: sempre `confirm` (`ConfirmRequired` con i
file), poi backup di indice e worktree a forma di stash in
`refs/project/backup/cherry-pick-abort-...` / `revert-abort-...` (stesso codice del merge), poi
`git cherry-pick --abort` / `git revert --abort`; se fallisce l'errore nomina il backup, che
resta. Non entra nel diario. Stessi rifiuti di `*.continue`.

### Sequenza di più commit avviata da terminale (scelta)

`git cherry-pick a b c` crea `.git/sequencer/`; un cherry-pick di un commit solo no. Con
`.git/sequencer` presente `*.continue` e `*.abort` sono `NotAllowed` (`check_picking`) e
`repo.operation` dà `continue`/`abort` `null`: `git commit` lascerebbe la sequenza a metà e
`git cherry-pick --abort` riporterebbe indietro anche i commit già applicati, fuori dal diario.
Si conclude da terminale. `OperationState::sequencer` lo dice.

## CliBackend

Letto lo stato senza fsmonitor (`check_pick_state`), HEAD sul branch con il valore atteso, i file
`--assume-unchanged` cambiati dalla patch (`diff-tree from to`, con `from`/`to` = genitore →
commit per il cherry-pick e al contrario per il revert; albero vuoto per un commit radice) e i
file che la patch scrive (`--diff-filter=AMT`) che HEAD non ha e che esistono su disco; poi il
backup e, con `GIT_REFLOG_ACTION=project: <id>` e `GIT_EDITOR=:`:

`git -c core.fsmonitor=false -c revert.reference=false <cherry-pick|revert> --no-edit
--no-rerere-autoupdate [-m <n>] --end-of-options <oid>`

`--no-edit` (niente editor), `revert.reference=false` (il messaggio resta quello sopra),
`--no-rerere-autoupdate` (contro `rerere.autoUpdate`: un file risolto da rerere resta in
conflitto da confermare). Hook e firma secondo la configurazione, mai `--no-verify`, mai
`--no-commit`, `--allow-empty` o `--keep-redundant-commits`.

Dopo: riuscito senza operazione in corso ma con il branch fermo → `Git` che nomina il backup;
riuscito → il branch dopo (`warnings` se HEAD non è più sul branch); in corso: con conflitti (dallo
stato e dall'output) o senza (firma, hook) → `in_progress`; in corso e vuoto (`is now empty`) →
`git <cmd> --skip`, backup tolto, `NotAllowed`; niente in corso: vuoto (il revert di un commit già
annullato esce così) → `NotAllowed`, altrimenti `NotAllowed` con i file (`would be overwritten`) o
`Git` con il testo di git; il backup si toglie solo se il branch vale ancora il valore di prima.
Un `MERGE_MSG` lasciato da git senza operazione in corso (e che prima non c'era) si toglie.

## Undo e backup

- **Concluso** (subito o con `*.continue`): `JournalEntry::pick` con il branch da `before` ad
  `after`, HEAD uguale, il backup (solo il comando di avvio) e la descrizione `Cherry-pick di
  <nome> su `<branch>` (da a1b2c3d a e4f5a6b)` / `Revert di ...`. Undo e redo come il merge:
  `reset_branch` (`git reset --keep`) con gli stessi controlli. Nel diario solo se il commit dopo
  ha per unico genitore il branch di prima, altrimenti avviso in `warnings` che nomina il backup.
  `*.continue` con HEAD staccata (avviato da terminale) entra come un commit.
- **Fermo in conflitto**: niente diario; undo e redo rifiutati finché è in corso.
- **Abort**: non entra nel diario; il recupero è il backup `...-abort-...`.

Il `MemoryBackend` applica la patch file per file (base = genitore, loro = commit; al contrario
per il revert): un file cambiato in modo diverso da HEAD è un conflitto (voci `u`, marcatori con
etichetta `<id> (<oggetto>)` / `parent of ...`), lo stato in corso in `MemoryMerge` (il commit e
`MERGE_MSG` con `# Conflicts:`); il commit che conclude prende l'autore del commit originale per
il cherry-pick. Risultato uguale a HEAD → `NotAllowed`. Backup in `commit_backups`
(`refs/project/backup/cherry-pick-<n>`, `revert-<n>`, `...-abort-<n>`); niente sequencer.
