# Scartare le modifiche

Parte di [ARCHITETTURA-git.md](../ARCHITETTURA-git.md): principi (§1), regole per ogni chiamata a
git (§5), comandi lenti (§6) e diario (§7) sono lì; qui il contratto dei comandi, il `CliBackend` e
undo e backup di questa funzione. "§n" rimanda sempre al documento principale.

## Comandi

### `worktree.discard`

Parametri: `{"paths", "confirm"}`.

Con backup (sotto).

## Passi e garanzie

`worktree.discard {"paths", "confirm"}` riporta il worktree dei file `paths` (file o cartelle,
alla lettera) alla versione in stage, come `git restore --worktree`. Scelte:

- **Solo il worktree.** L'indice non si tocca: un file `MM` torna `M.`. Per togliere anche lo
  stage c'è `index.unstage` (che ha il suo backup).
- **`"confirm": true` obbligatorio.** Senza, `ConfirmRequired` con l'elenco dei file che si
  perderebbero e niente cambia; i rifiuti qui sotto vengono prima della conferma.
- **Solo file tracciati con modifiche fuori stage** (Y ∉ {`.`, `?`, `!`}: modificati, eliminati,
  cambiati di tipo, anche nel modo). I **non tracciati non si toccano mai**: chiesti per nome
  sono un errore (scartarli vorrebbe dire cancellarli, e `git stash create` non li salva, quindi
  non ci sarebbe un backup); dentro una cartella restano dove sono. Niente opzione per
  cancellarli: se servirà sarà un comando a parte (`git stash create` non ha `-u`, servirebbe un
  altro backup). I sottomoduli chiesti per nome sono rifiutati, dentro una cartella saltati; i
  file `--assume-unchanged`/`--skip-worktree` hanno Y = `.` e non si toccano.
- **Quando il backup non si può fare, niente si scarta**: branch senza commit ("You do not have
  the initial commit yet"), file in conflitto nel repo ("Cannot save the current index state"),
  file `intent-to-add` nel repo ("not uptodate. Cannot merge", anche con git 2.56). I controlli
  sono in `discard_plan`, comuni ai backend.

- **Niente al posto delle cartelle.** Un non tracciato o ignorato al posto di una cartella del
  percorso di un file da scartare (`top/d/x` tracciato, la cartella cancellata e un file
  `top/d` creato) o al posto del file stesso (una cartella `x/`) verrebbe cancellato da `git
  restore` per ricreare il file, e il backup non lo contiene: `discard_plan` rifiuta se lo stato
  lo mostra, e il `CliBackend` controlla anche il file system (`symlink_metadata` di ogni
  cartella del percorso: deve essere una cartella vera, non un file o un link; il file non deve
  essere una cartella), così sono coperti anche ignorati e link. Il controllo si ripete prima
  dell'undo e del redo.
- **Il piano confermato.** `GitBackend::discard` riceve anche il piano calcolato dal comando
  (quello che la conferma elenca): se il piano letto dal backend prima di scrivere è diverso
  (un file in più o in meno, un'altra versione in stage), `NotAllowed` e niente cambia. Il
  piano è calcolato dentro lo stesso comando: fra quello che la UI ha mostrato e il comando
  la garanzia c'è solo se la UI passa i file esatti, non le cartelle.

Il `CliBackend`, in quest'ordine: stato senza fsmonitor, `discard_plan` e confronto con il
piano confermato; indice (`ls-files -s`) uguale allo stato per quei file; controllo delle
cartelle; `index.lock` → `IndexLocked` subito; `git -c core.fsmonitor=false stash create
<messaggio fisso>` con l'identità fissa dei backup (un commit con il worktree dei file tracciati;
non tocca indice, worktree né `refs/stash`; se fallisce per l'indice bloccato, `IndexLocked`);
**backup byte per byte**: per ogni file si confrontano i byte del worktree con quelli che `git
restore` ricreerebbe dal commit (`cat-file --filters --path=<file>`; per un link la
destinazione); se differiscono (es. CRLF con `core.autocrlf=input`, che il filtro `clean` toglie
e lo `smudge` non rimette, o un filtro non reversibile) i byte si salvano senza filtri
(`hash-object -w --no-filters`) in un commit con solo quei file, figlio di quello di `stash
create` (campo `raw` della voce); `update-ref refs/project/backup/discard-<secondi>-<nanosecondi>
<raw o stash> ""`; ricontrollo dell'indice e delle cartelle; **ultima verifica** che il backup
abbia esattamente il worktree attuale (`git diff --name-only -z <stash> -- <file>` vuoto, per il
modo, senza `diff.autoRefreshIndex=false`, che farebbe comparire i file cambiati solo nei dati
di `stat`, con `--no-optional-locks`; e i byte come sopra, o quelli di `raw`); infine `git
restore --worktree --no-overlay` dei soli file del piano (percorsi su stdin). Resta solo
l'istante fra l'ultima verifica e `restore` in cui un editor che salva un file perderebbe quella
modifica.

La voce del diario ha `"worktree": {"stash", "files": [{"path", "change", "mode", "oid"}],
"raw"}` (il modo e l'id della voce dell'indice a cui il file è stato riportato; `raw` è `null`
se non serviva) e `"backup"` con il ref. `Journal::undo`/`redo` chiamano
`GitBackend::restore_worktree` invece di `restore_refs`, e prima controllano HEAD: deve essere
quella registrata (`head.after` per l'undo, `head.before` per il redo, uguali per uno scarto),
altrimenti `UndoConflict` (un checkout o un commit da terminale) e niente cambia.

- **undo**: ogni file deve avere ancora nell'indice la voce registrata e il worktree uguale
  all'indice (nessuna modifica fuori stage, nessun flag che la nasconderebbe); altrimenti
  `UndoConflict` con una riga per file e niente cambia. Poi `git restore --source=<stash>
  --worktree --no-overlay` (un file che nel commit non c'è, perché era eliminato, si cancella di
  nuovo) e i file di `raw` riscritti con i loro byte. Il worktree sovrascritto è uguale
  all'indice: niente si perde.
- **redo**: stessa voce dell'indice e worktree uguale al backup (stessi byte e modo, come sopra:
  anche un cambio dei soli fine riga lo blocca), poi di nuovo `restore --worktree`. Nessun
  backup nuovo: il backup ha già quel contenuto.
- Dopo la scrittura si rilegge (byte e modo per l'undo, uguale all'indice per il redo): un file
  non come atteso va in `warnings`.

Il `MemoryBackend` registra i backup in `MemoryBackend::worktree_backups`
(`refs/project/backup/discard-<n>`, con la voce dello stato e il contenuto di ogni file).
