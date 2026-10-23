# Editor dei conflitti

Parte di [ARCHITETTURA-git.md](../ARCHITETTURA-git.md): principi (§1), regole per ogni chiamata a
git (§5), comandi lenti (§6) e diario (§7) sono lì; qui il contratto dei comandi, il `CliBackend` e
undo e backup di questa funzione. "§n" rimanda sempre al documento principale.

Parser dei marcatori in `crates/domain/src/git/conflict.rs`; tipi e controlli comuni ai backend in
`crates/engine/src/git/conflict.rs`; comandi in `crates/engine/src/builtin/conflict.rs`;
`CliBackend` in `crates/git/src/backend/conflict.rs`; UI in `crates/ui/src/conflict.rs` e
`crates/ui/src/view/conflict_view.rs`.

## Parser dei marcatori (dominio)

Funzioni pure, sui byte del file del worktree:

- `parse_conflicts(text: &[u8], marker_size: usize) -> Result<ConflictText, DomainError>`:
  `ConflictText { segments: Vec<Segment> }`, `Segment::Common(Vec<u8>)` (byte fuori dai blocchi,
  a capo compresi) o `Segment::Conflict(ConflictBlock)` con `ours`, `base: Option<Vec<u8>>`,
  `theirs` (byte delle righe, a capo compresi), le etichette dopo i marcatori (`ours_label`,
  `base_label`, `theirs_label`, `Vec<u8>` senza lo spazio iniziale, vuote se mancano) e `range`,
  l'intervallo di byte del blocco nel testo, marcatori e loro a capo compresi.
- Marcatori: a inizio riga, esattamente `marker_size` caratteri (default 7, attributo
  `conflict-marker-size`) e poi fine riga o spazio: `<<<<<<<`, `|||||||` (solo con
  `merge.conflictStyle=diff3`/`zdiff3`), `=======`, `>>>>>>>`. Una riga con più caratteri
  (`<<<<<<<<`) è testo. A capo `\n` o `\r\n`, conservati com'erano; l'ultima riga può non avere a
  capo.
- Testo fuori da un blocco che sembra un marcatore di mezzo (`=======`, `|||||||`, `>>>>>>>` senza
  `<<<<<<<` aperto) è testo comune (può essere un titolo setext o una riga di un file vero). Un
  blocco aperto e non chiuso, o con le parti nell'ordine sbagliato → `DomainError::InvalidConflict`
  (l'editor mostra le tre versioni e il file com'è, senza blocchi).
- `Choice`: `Ours`, `Theirs`, `Base` (solo se il blocco ha la base; altrimenti errore),
  `OursThenTheirs`, `TheirsThenOurs`. `ConflictBlock::resolved(&self, choice) -> Result<Vec<u8>, _>`
  i byte che sostituiscono il blocco; `resolve_block(text, &ConflictText, index, choice) ->
  Result<Vec<u8>, _>` il testo con solo quel blocco sostituito (gli altri, risolti o no, restano
  come sono); `resolve_all(text, &ConflictText, choice)` tutti i blocchi con la stessa scelta.
- Byte, non stringhe: nessuna perdita per testo non UTF-8 (la UI però modifica solo UTF-8, sotto).

Test: stile merge e diff3, etichette, CRLF, ultima riga senza a capo, blocchi vuoti da un lato,
marcatori più lunghi, `=======` isolato, blocco non chiuso, `marker_size` diverso,
`resolve_block` e `resolve_all` per ogni `Choice`, `range` che delimita esattamente il blocco, e che
concatenare i segmenti con i blocchi ricostruiti dai `range` ridia il file.

## Comandi

### `conflict.show`

Parametri: `{"path"}`. Sola lettura.

`path`: `RepoPath` validato come in `diff.*`; deve essere un file in conflitto dello stato (voce
`u`), altrimenti `NotAllowed`. Risultato:

```json
{"path": "...", "kind": "BothModified" | "BothAdded" | "DeletedByUs" | "DeletedByThem" | ...,
 "base": Version | null, "ours": Version | null, "theirs": Version | null,
 "worktree": Version | null, "marker_size": 7, "digest": "<32 hex>"}
```

`Version`: `{"text": string | null, "binary": bool, "size": n}`; `text` è il contenuto se è
UTF-8 valido e senza NUL, altrimenti `null` con `binary: true`. `base`/`ours`/`theirs` dagli stadi
1/2/3 dell'indice (`ls-files -s -z` per gli id, poi `cat-file blob <id>`: byte dell'indice, senza
filtri), `null` se lo stadio manca (es. `DeletedByUs` non ha `ours`); `worktree` i byte del file
(`null` se non c'è; un link o una cartella → `NotAllowed`). `marker_size` da `git check-attr
conflict-marker-size -z --stdin` (7 se non impostato o non valido). `digest`: FNV-1a a 128 bit
(come `file_digest`) dei byte del worktree, con un valore fisso per "file assente": identifica la
versione mostrata per `conflict.resolve`. Sottomodulo in conflitto (modo 160000) → `NotAllowed`.

### `conflict.resolve`

Parametri: `{"path", "digest", "content"?: string, "delete"?: bool}`. Scrive (`writes`).

Esattamente uno fra `content` e `delete: true` (altrimenti `InvalidParams`). Risolve il conflitto
di `path`: scrive `content` nel file del worktree (byte UTF-8 così come arrivano, senza
conversioni di a capo: la UI parte dai byte del worktree) o lo cancella con `delete`, e lo mette
in stage come `index.stage` di quel solo percorso (`git add` o, con `delete`, `git rm --cached`
del percorso, `--literal-pathspecs`). Il contenuto **non** viene controllato per marcatori
rimasti: è un testo libero dell'utente; l'avviso è compito della UI.

Controlli, prima di scrivere, sempre `NotAllowed`/`StaleDiff` senza cambiare niente:

- `path` non in conflitto (voce `u`), sottomodulo, link o cartella → `NotAllowed`.
- `digest` diverso da quello calcolato ora sul worktree → `StaleDiff` (il file è cambiato dopo
  `conflict.show`: rileggere).
- Indice bloccato → `IndexLocked`.

**Backup** (§1, regole: si riscrive l'unica copia di qualcosa senza che git chieda conferma): il
file del worktree con i marcatori e le modifiche a mano dell'utente esiste solo lì. Prima di
scrivere si salvano i suoi byte (`hash-object -w --no-filters --stdin`) in un commit (identità
fissa dei backup, nessun genitore) con un albero che contiene solo quel file al suo percorso, in
`refs/project/backup/conflict-<secondi>-<nanosecondi>`. Se il file non c'è (es. `DeletedByUs`
con il file cancellato) niente backup del worktree. Le versioni dei tre stadi non servono: vengono
dai commit del merge e da `git checkout -m <path>` si rigenera il file con i marcatori. Se il
backup non riesce, niente si scrive. Se la scrittura o lo stage falliscono dopo il backup, il
backup resta e l'errore lo nomina.

La scrittura nel worktree: file temporaneo nella stessa cartella e `rename` sopra il file (con i
permessi del file attuale); poi si rilegge il digest per controllare che nel frattempo
nessuno l'abbia cambiato (se il file prima della rinomina non è più quello del digest, `StaleDiff`
e il temporaneo si cancella). Limite noto: fra quest'ultimo controllo e il `rename` resta una
finestra di poche chiamate di sistema in cui un salvataggio da un editor andrebbe perso senza
backup. Lo stage usa lo stesso percorso di `index.stage` (con il suo backup
dell'indice, se serve).

Risultato: `{"status": Status, "backup": string | null, "index_backup": string | null}`
(`backup`: il ref del worktree salvato; `index_backup`: quello dell'indice di `index.stage`).

Undo: non sposta ref, quindi niente diario, come lo stage; il recupero è il backup (`git show
<backup>:<path>`) o `git checkout -m <path>` finché il merge è in corso.

Scelte di implementazione (dove il contratto non diceva):

- Gli id degli stadi vengono dal record `u` di `git status --porcelain=v2` (sono gli stessi di
  `ls-files -s`), letto comunque per il controllo "in conflitto"; i byte con `cat-file blob`.
- `digest`: FNV-1a a 128 bit di un byte di tipo (`0` file assente, `1` presente) seguito dai byte
  del worktree (`worktree_digest`), così nessun contenuto ha il digest del file assente.
- Un link simbolico anche fra le **cartelle** del percorso → `NotAllowed` (si scriverebbe fuori
  dal repo o in un altro file); con il file assente le cartelle mancanti si creano.
- Il temporaneo (`.project-conflict-<pid>-<nanos>.tmp` nella cartella del file) prende tutti i
  permessi del file che sostituisce (un file 0600 resta 0600); un file che non c'era nasce con i
  permessi della `umask`, eseguibile se lo è uno degli stadi `ours`/`theirs` (es. `DeletedByUs`
  con il lato loro 100755). Le cartelle mancanti si creano solo dopo il ricontrollo del digest. Il backup registra il modo del file (`100644`/`100755`).
- Un errore dopo il backup (scrittura, digest ricontrollato, stage) conserva il tipo quando è
  `Git`, `NotAllowed` o `StaleDiff` e aggiunge il nome del backup al testo; gli altri diventano
  `Git` con il testo originale e il backup.
- `marker_size`: un numero positivo dall'attributo, altrimenti 7; il `MemoryBackend` dà sempre 7.
- `delete` passa anch'esso dallo stage di `index.stage` (`git add` di un percorso cancellato
  registra la cancellazione e toglie gli stadi), senza un `git rm --cached` a parte.

`MemoryBackend`: le voci `u` di `merge_three_way` con i tre stadi; `conflict.show` li legge,
`conflict.resolve` scrive il file e lo mette in stage come `stage`; backup nel suo store dei ref.

## UI

Dal banner dell'operazione in corso (`repo.operation`), un file in conflitto si apre nell'editor
dei conflitti (al posto della vista del diff), che legge `conflict.show` e analizza `worktree.text`
con `parse_conflicts(.., marker_size)`:

- **Tre colonne in alto**, sola lettura: base, nostra (`ours_label`, o "HEAD"), loro
  (`theirs_label`, o la `label` di `incoming`), ciascuna con il testo del suo stadio; `null` →
  "(assente)", binario → "(binario)".
- **Risultato in basso**, modificabile (`TextEdit` multiriga, monospazio), che parte dal testo del
  worktree. Per ogni blocco del parser una riga di pulsanti: "Nostra", "Loro", "Entrambe",
  "Base" (se c'è). I blocchi sono quelli del testo **attuale** del risultato (si rianalizza a ogni
  modifica): la scelta sostituisce quel blocco con `resolve_block`, il resto (anche le modifiche a
  mano) resta. Pulsanti globali "Tutto nostra", "Tutto loro" (`resolve_all`) e "Ricomincia" (rilegge
  `conflict.show`). Se il testo attuale non si analizza (marcatori rotti a mano) niente pulsanti
  per blocco, il testo resta modificabile.
- **"Segna come risolto"** → `conflict.resolve {path, digest, content}`. Se il risultato contiene
  ancora marcatori (`parse_conflicts` trova blocchi) chiede conferma in due tempi ("Contiene ancora
  marcatori: risolvere comunque?"). Dopo: rilegge `repo.operation` e lo stato; il file sparisce da
  `conflicts`; con `ready` il pulsante "Concludi merge" si attiva. `StaleDiff` → messaggio e
  rilettura.
- File binario, `worktree.text` `null` o parser che fallisce: niente blocchi né risultato
  modificabile; solo "Usa nostra" / "Usa loro" (con `content` = testo di quel lato; solo se è
  testo) e, per `DeletedByUs`/`DeletedByThem`, "Elimina il file" (`delete: true`).
- Niente logica di modifica nei callback: i pulsanti preparano `content` (puro, `resolve_block`) e
  invocano `Registry::run`.

Fatto quando: test del parser; un test della UI (`egui_kittest` o il modello della vista, come gli
altri) che apre un conflitto del `MemoryBackend`, sceglie per blocco, segna come risolto e trova
`ready`; scenario di integrazione `CliBackend` (`crates/git/tests`) con merge in conflitto,
`conflict.show`, `conflict.resolve`, `merge.continue`; CI verde.
