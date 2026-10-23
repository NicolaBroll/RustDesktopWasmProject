# Stash

Parte di [ARCHITETTURA-git.md](../ARCHITETTURA-git.md): principi (§1), regole per ogni chiamata a
git (§5), comandi lenti (§6) e diario (§7) sono lì; qui il contratto dei comandi, il `CliBackend` e
undo e backup di questa funzione. "§n" rimanda sempre al documento principale.

## Comandi

### `stash.list`

Parametri: `{}`.

Sola lettura. Risultato: `{"stashes": [{"index": n, "name": "stash@{n}", "id", "base", "message",
"branch": string | null, "time", "untracked": bool}]}`, dal più recente, dal reflog di `refs/stash`
(vuoto se non c'è). `base`: il primo genitore, il commit di HEAD quando lo stash è stato creato (la
UI disegna lo stash accanto a questo commit; con `repo.log {"all": true, "stashes": true}` è sempre
nel grafo); `message`: il messaggio del reflog come in `git stash list` (`On <branch>: <msg>` o `WIP
on <branch>: <id> <oggetto>`); `branch` letto da lì (`null` con HEAD staccata o un altro formato);
`time`: data del commit dello stash; `untracked`: ha il terzo genitore con i non tracciati. I file
tracciati di uno stash si leggono con `commit.show`/`diff.commit` sull'`id` (confronto con `base`:
il worktree salvato, in stage o no); i non tracciati non ci sono. `refs/stash` resta anche in
`repo.refs` (`kind: "Stash"`, lo stash più recente).

### `stash.push`

Parametri: `{"message"?, "include_untracked"?, "keep_index"?, "paths"?}`.

Scrive (`writes`), entra nel diario (§7). `git stash push` (vedi "CliBackend"): salva le modifiche ai file
tracciati (in stage e no) e, con `include_untracked`, i non tracciati (mai gli ignorati: niente
`--all`), poi riporta indice e worktree come HEAD (con `keep_index` l'indice resta e il worktree
diventa come l'indice). `paths`: lista non vuota di percorsi alla lettera, come `index.stage` (**git
mette comunque nello stash tutto l'indice**: le modifiche in stage degli altri file restano nel
worktree ma sono anche nello stash, e applicarlo le riporta). `message`: una riga, senza caratteri
di controllo (`InvalidParams`); vuoto = assente (git scrive `WIP on ...`). Rifiutati prima di git
(`check_stash_push`, `NotAllowed`, niente cambia): operazione in corso, file in conflitto, branch
senza commit, file `intent-to-add` (git non riesce a salvarli), file `--assume-unchanged` fra quelli
scelti (solo `CliBackend`, vedi sotto: git non ne salverebbe le modifiche e poi le cancellerebbe con il
reset del worktree), niente da mettere da parte fra i file scelti (anche se git non crea lo stash:
`refs/stash` invariato). Risultato: `{"stash": StashEntry, "stashes": [StashEntry], "status",
...diario}`.

### `stash.apply` / `stash.pop`

Parametri: `{"index"?, "expected"?, "restore_index"?}`.

Scrivono (`writes`), **non** entrano nel diario (§7). Lo stash è `stash@{index}` (0 se manca; non
c'è → `NotAllowed` che dice quanti sono); `expected`: l'id **completo** dello stash visto
(altrimenti `InvalidParams`), se l'elenco è cambiato → `NotAllowed` che dice dov'è ora (consigliato:
la UI lo passa sempre). Al backend arriva l'id. `restore_index`: rimette anche la divisione fra in
stage e no (`--index`); senza, tutto torna fuori stage tranne i file nuovi, che git lascia in stage.
**Mai sopra modifiche locali** (controlli prima di git, `NotAllowed` e niente cambia): operazione in
corso, file in conflitto, una modifica (in stage o no) a un file che lo stash cambia (rispetto alla
base, nel worktree o nell'indice salvati), un non tracciato o ignorato al posto di un file che lo
stash aggiunge o di un non tracciato che rimette (anche al posto di una sua cartella), file
`--assume-unchanged` che lo stash cambia. **Eccezione**: un file senza modifiche fuori stage la cui
voce dell'indice è la stessa nell'indice e nel worktree salvati dallo stash (`<id>^2` e `<id>`; modo
0: assente da tutti e due) ha già la versione dello stash e non blocca: è il caso dei file rimasti
in stage dopo un push con `keep_index`, o con `paths` (git salva comunque tutto l'indice); senza,
l'undo di quei push e il pop dopo un `keep_index` sarebbero sempre rifiutati. Un file `MM` rimasto
con `keep_index` resta bloccato (il worktree dello stash è diverso: git andrebbe in conflitto).
Servono perché `git stash apply` non controlla tutto prima di scrivere: con un non tracciato dello
stash già presente rimette i file tracciati e poi fallisce, e rifiutando per una modifica locale ha
già rimesso i non tracciati. `restore_index` che non si applica sopra l'indice attuale (HEAD
cambiato dalla base) → `NotAllowed` che suggerisce di riprovare senza. **Conflitti** (scelta): se
HEAD ha cambiato gli stessi file dalla base il merge si ferma e i file restano in conflitto come li
lascia git (`conflicts`; nessuna operazione in corso, solo voci `U` nell'indice); niente si perde,
perché quei file non avevano modifiche locali (controllo sopra) e lo stash resta nell'elenco: `pop`
non lo elimina (`dropped: false`, con un avviso). Si risolvono con `index.stage` del file (poi
l'editor dei conflitti, S30); finché ci sono, apply e push sono rifiutati. `pop` senza conflitti:
drop come `stash.drop` (backup `refs/project/backup/stash-...` prima). Un drop che fallisce **dopo**
l'apply non è un errore del pop (`StashPopped::drop_error`): l'apply è fatto e non va ripetuto, il
risultato ha `dropped: false` e un avviso "applicato ma non tolto dall'elenco". Risultato:
`{"stash": StashEntry, "conflicts": [percorso], "dropped": bool, "backup": string | null,
"warnings", "stashes", "status"}`.

### `stash.drop`

Parametri: `{"index"?, "expected", "confirm"}`.

Scrive (`writes`), entra nel diario (§7). Stash scelto come `stash.apply`. Chiede **sempre**
`"confirm": true` **con `"expected"`**, l'id completo dello stash: senza `confirm`,
`ConfirmRequired` con posizione, id, messaggio, numero di file tracciati e se ha non tracciati, e
l'id in `expected` (come per il force di `remote.push`); `confirm` senza `expected` →
`InvalidParams`; niente cambia. La conferma vale solo per lo stash visto (`expected` diverso da
`stash@{index}` → `NotAllowed`). Prima un backup del commit in `refs/project/backup/stash-<s>-<ns>`
(se non riesce non si elimina niente). Risultato: `{"stash": StashEntry, "backup": string,
"warnings", "stashes", ...diario}`.

## CliBackend

**Stash** (`stash.*`, S28). Elenco: `git log --walk-reflogs -z --no-show-signature
--no-color --format=tformat:%H%x00%P%x00%ct%x00%gs --end-of-options refs/stash --` (4 campi
per record, ognuno terminato da NUL; solo se `refs/stash` esiste). Push: prima `ls-files -v`: un file
`--assume-unchanged` (lettera minuscola) fra quelli scelti → `NotAllowed`, sempre, senza
guardare il contenuto (`git stash push` non ne vede le modifiche e poi riporta il worktree a
HEAD con un reset: le perderebbe), prima degli altri controlli. Poi `git -c
core.fsmonitor=false [--literal-pathspecs] stash push [--include-untracked]
--keep-index|--no-keep-index [--message=<msg>] [--pathspec-from-file=- --pathspec-file-nul]`
(percorsi su stdin). **Eccezione alla regola dei messaggi via stdin**: `git stash` non legge il
messaggio da stdin né da un file, quindi va in un solo argomento `--message=<testo>` (una riga
senza caratteri di controllo, controllata dal motore), che git non può leggere come un'altra
opzione. Lo stash nuovo si riconosce da `refs/stash` prima e dopo (git esce con 0 anche con
"No local changes to save"); se git fallisce dopo aver creato lo stash, l'errore lo nomina.
Apply: i file che lo stash tocca (`diff-tree -r -z --name-only --no-renames <base> <id>` e
`<base> <id>^2`) e i non tracciati che rimette (`ls-tree -r -z --name-only --full-tree
--end-of-options <id>^3`, **senza `--` finale**: `ls-tree` lo prenderebbe per un percorso)
confrontati con lo stato e con il file system (`symlink_metadata`: un file che lo stash
aggiunge non deve esistere, nemmeno ignorato, e le sue cartelle devono essere cartelle vere),
con l'eccezione dei file che hanno già la versione dello stash (voci di `ls-tree -r -z
--full-tree <id>^2` e `<id>` uguali alla voce dell'indice), poi `git -c core.fsmonitor=false
-c merge.directoryRenames=false stash apply [--index] --end-of-options <id>` (l'id, mai
`stash@{n}`). `merge.directoryRenames=false`: con la rilevazione delle cartelle rinominate un
file che lo stash aggiunge in `old/` finirebbe in `new/` se HEAD ha rinominato la cartella, e
sovrascriverebbe un ignorato che c'è lì, che i controlli non vedono (guardano `old/`). Uscita non zero con voci `U` nello stato o `CONFLICT` nell'output: i conflitti
(risultato, non errore); "conflicts in index" (`--index`): `NotAllowed`, niente modificato.
Solo se il repo cambia fra i controlli e git, che allora ha già scritto una parte: "already
exists, no checkout" → `NotAllowed` che dice che l'apply è **rimasto a metà** (git ha già fatto
il merge dei tracciati, i non tracciati elencati non sono stati rimessi); "would be
overwritten" → `NotAllowed` con i soli file del blocco subito dopo il messaggio (non il
riepilogo dello stato che git stampa dopo), "niente modificato" solo se lo stash non ha non
tracciati, altrimenti avvisa che git potrebbe averli già rimessi. I non tracciati da rimettere
si controllano sul file system sempre, anche se lo stato li mostra (es. `D.` con il file
ignorato nel worktree), e l'eccezione dei file che hanno già la versione dello stash vale solo
per i tracciati. Drop: backup
`update-ref <ref> <id> ""`, ricontrollo che `stash@{n}` sia ancora l'id, `git stash drop
--quiet --end-of-options stash@{n}` (se fallisce il backup si toglie); git non ha un drop con
il valore atteso, quindi resta l'istante fra il ricontrollo e `drop`, e dopo l'elenco si
confronta con quello atteso (diverso → `warnings`). Store (undo di un drop): `git stash store
--quiet --message=<msg> <id>` (`store` rifiuta `--end-of-options`; l'id è esadecimale), poi lo
stash dev'essere in cima.

## Undo e backup

`stash.push` e `stash.drop` entrano nel diario con lo stash (`JournalEntry::stash`, uno
`StashRecord`: `{"id", "message", "base", "push": StashPush | null}`); la voce non ha `refs`
(l'elenco degli stash è il reflog di `refs/stash`, e spostare quel ref non basterebbe).

- **Undo di `stash.push` = `pop` di quello stash con `restore_index`** (come `git stash pop
  --index`): solo se non c'è un'operazione in corso, HEAD è quella della voce **e sul commit
  `base`** (così il merge non può andare in conflitto: lo stash si riapplica sullo stesso
  commit) e lo stash è ancora nell'elenco (in qualunque posizione), altrimenti `UndoConflict` e
  niente cambia. Valgono i controlli di `stash.apply`: se nel frattempo un file dello stash ha
  modifiche locali, `NotAllowed` e la voce resta. Prima del drop il commit dello stash va in
  `refs/project/backup/undo-...` (in `backups`). Se l'apply riesce ma il drop no, l'undo dà un
  errore che dice che lo stash è stato applicato (non va ripetuto; la voce resta nel diario e lo
  stash va eliminato con `stash.drop`). **Limite**: dopo un push con `paths` mentre anche altri
  file erano in stage, git ha messo nello stash tutto l'indice, e `pop --index` non si applica
  sopra l'indice che ha ancora quei file ("conflicts in index"): l'undo è rifiutato (`NotAllowed`,
  niente cambia, la voce resta) con un messaggio suo che indica la via d'uscita, `stash.pop`
  senza `restore_index`. Redo = di nuovo `stash.push` con gli stessi
  parametri, con la stessa HEAD: lo stash nuovo ha di solito un altro id (la voce si aggiorna) e
  contiene quello che c'è da mettere da parte al momento del redo (di solito quello che l'undo ha
  rimesso; se nel frattempo sono cambiati altri file, anche quelli, che restano comunque nello
  stash).
- **Undo di `stash.drop` = `git stash store`** dello stesso commit (lo tiene il backup del drop),
  solo se non è già nell'elenco (`UndoConflict`): torna **in cima** (`stash@{0}`), non nella
  posizione di prima, perché `git stash store` non sa inserire altrove (scelta: riscrivere il
  reflog di `refs/stash` a mano sarebbe più fragile). Il messaggio è quello del reflog. Redo =
  di nuovo il drop, ovunque sia ora lo stash; il backup del drop di solito c'è già, ma se non
  punta più allo stash (tolto o spostato a mano) se ne fa uno nuovo, `refs/project/backup/redo-...`,
  prima di eliminarlo.
- **`stash.apply` e `stash.pop` non entrano nel diario** (scelta): cambiano solo indice e
  worktree, e annullarli vorrebbe dire togliere dai file quello che lo stash ha aggiunto,
  mescolato alle modifiche che c'erano già (apply e pop sono ammessi con modifiche ad altri file)
  o fatte dopo: non c'è un modo sicuro. Niente si perde comunque: dopo `apply` lo stash resta
  nell'elenco, dopo `pop` il suo commit è in `refs/project/backup/stash-...` (e l'undo di uno
  `stash.push` precedente, con lo stash ormai tolto, è rifiutato con `UndoConflict`). `can_undo`
  dopo un apply o un pop riguarda quindi la voce di prima, e la UI non deve presentare l'undo
  come annullamento dell'apply.
- Un conflitto lasciato da apply/pop non ha uscite automatiche per ora: si risolve file per file
  con `index.stage` (poi con l'editor dei conflitti, S30). Grazie ai controlli di `stash.apply`
  i file in conflitto non avevano modifiche locali: la versione "nostra" è quella di HEAD.
- `refs/stash` è una protezione **volatile** dei commit (vedi in §7 "Backup dei commit
  annullati"): i backup `refs/project/backup/stash-...` e `undo-...` di drop e pop sono invece
  ref veri e propri, che una futura pulizia dei backup deve trattare come gli altri.

Il `MemoryBackend` registra i backup degli stash in `commit_backups`
(`refs/project/backup/stash-<n>`, `undo-<n>`, `redo-<n>`) e li risolve come ref.

## UI (`crates/ui/src/stash.rs`)

**Stash (S28, `crates/ui/src/stash.rs`)**. La sezione "Stash" della colonna sinistra viene da
`stash.list` (letto con ref e stato, riletto dopo undo/redo e preso dalle risposte dei comandi),
non da `refs/stash`: una riga per stash (`stash@{n}`, messaggio senza il prefisso `On <branch>:`,
branch e base, "con non tracciati"). Clic: seleziona lo stash (per id, la posizione cambia) e la
sua base nel grafo; a destra, al posto dei dettagli del commit, il pannello dello stash con le
azioni e i file tracciati (`commit.show` sull'id; un file apre `diff.commit` sull'id, cioè il
confronto con la base); i non tracciati si segnalano solo, perché `commit.show` non li vede.
Menu contestuale e pannello: Applica, Pop, la casella "Rimetti anche lo stage"
(`restore_index`), Elimina…. Apply, pop e drop mandano sempre `index` ed `expected` dall'elenco
visto; un `NotAllowed` va nella barra di stato e l'elenco si rilegge. **Apply e pop non entrano
nel diario**: l'avviso dice "Non si annulla con «Annulla»" e il pulsante Annulla resta quello
della voce precedente (il suo suggerimento ne mostra la descrizione). `conflicts` non vuoto apre
una finestra (non modale) con i file e "Vai alle modifiche": si risolvono con lo stage del file. Un pop riuscito con `dropped: false` e senza conflitti (applicato, ma il drop non è
riuscito) si segnala in rosso con l'avviso del motore, e per quello stash Pop resta spento (le
modifiche sono già nei file): resta Elimina.
Elimina va in due tempi come la rimozione di un remoto: senza `confirm` il motore risponde con
`ConfirmRequired` e il motivo, il dialogo rosso lo mostra, e solo il clic manda `"confirm":
true`. "Stash…" (barra in alto, testa dell'area di stage, sezione Stash; attivo solo con
modifiche) apre un dialogo con messaggio facoltativo (una riga: gli a capo diventano spazi),
"Includi i file non tracciati" e "Tieni lo stage"; un rifiuto resta nel dialogo. Nel grafo ogni
stash è un'etichetta sulla riga della base, dopo branch e tag, con solo il bordo grigio: il log
della lista chiede `"all": true, "stashes": true`, e dopo push, drop e pop riuscito si rilegge.
