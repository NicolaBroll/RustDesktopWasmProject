# Stage e unstage (file e righe)

Parte di [ARCHITETTURA-git.md](../ARCHITETTURA-git.md): principi (§1), regole per ogni chiamata a
git (§5), comandi lenti (§6) e diario (§7) sono lì; qui il contratto dei comandi, il `CliBackend` e
undo e backup di questa funzione. "§n" rimanda sempre al documento principale.

## Comandi

### `index.stage` / `index.unstage`

Parametri: `{"paths"}`.

`paths`: lista non vuota di `RepoPath` validati come in `diff.*`, presi alla lettera (una cartella
comprende i suoi file). Risultato: `{"status": Status, "backup": string | null}`, con il nuovo stato
(come `repo.status`) e il ref del backup dell'indice salvato prima di scrivere, se c'erano versioni
in stage che esistevano solo nell'indice (vedi "Backup dell'indice"). `stage` = `git add` (modifiche, file nuovi,
cancellazioni; una rinomina fatta nel worktree si mette in stage con i due percorsi; una
cancellazione già in stage si salta); prima una prova a vuoto (`--dry-run`), così un file ignorato
chiesto per nome (`NotAllowed`, mai `-f`) o un percorso inesistente non lasciano niente a metà.
Stage di un file in conflitto = risoluzione, solo se il file è chiesto per nome: dentro una cartella
richiesta è rifiutato (`NotAllowed`). `unstage` = `git restore --staged --no-overlay
--source=<HEAD>` (albero vuoto senza commit): solo l'indice, mai il worktree; un file in conflitto è
rifiutato (`NotAllowed`), perché perderebbe gli stadi del conflitto. Indice bloccato da un altro
git: `IndexLocked`, senza riprovare né cancellare il lock.

### `index.stage_lines` / `index.unstage_lines`

Parametri: `{"path", "lines", "digest"}`.

Usa il costruttore di patch (sotto) e l'applicatore rigoroso: `lines` è una lista **non vuota** di
`LineRef` (`{"old": n}` / `{"new": n}`) con i numeri di riga di `diff.worktree` (stage) o
`diff.index` (unstage) per `path`; `digest` (obbligatorio) è il campo `digest` di quel file nello
stesso diff, che identifica il diff visto (vedi sotto). Il backend rilegge il diff del file, sceglie
il `FileDiff` con `new_path == path` (altrimenti `old_path == path`: una rinomina chiesta con il
vecchio nome), controlla il digest, costruisce la patch con `build_patch` (senza `--reverse`: la
patch è sempre in avanti rispetto all'indice), rifiuta (`NotAllowed`) un hunk senza contesto in
testa che non parte dall'inizio del file o senza contesto in coda che non è l'ultimo, calcola il
contenuto nuovo con `apply_patch` sul blob in stage e lo scrive (`CliBackend`: `hash-object -w
--stdin --no-filters` e `update-index -z --index-info`, vedi "Backup dell'indice"; `MemoryBackend`: nei suoi blob). Errori,
sempre senza scrivere: file senza più modifiche o digest diverso → `StaleDiff` (rileggere il diff);
due `FileDiff` sullo stesso percorso (cambio di tipo) → `NotAllowed`; `UnsupportedPatch` del dominio
(binari, sottomoduli, link, conflitti, copie, rinomine nello stage) → `NotAllowed`;
`InvalidLineSelection` o `lines` vuota → `InvalidParams`; indice bloccato → `IndexLocked`. Backup
come `index.stage` (vedi "Backup dell'indice"). Risultato: `{"status": Status, "backup": string | null}`.


`diff.worktree` e `diff.index` aggiungono a ogni file il campo `digest`: 32 cifre esadecimali,
un hash FNV-1a a 128 bit della serializzazione JSON del `FileDiff` (`project_engine::file_digest`;
un `FileDiff` riletto da quel JSON dà lo stesso digest). Copre percorsi, modi, id e ogni riga con
i suoi numeri: i numeri di riga di una selezione possono coincidere per caso con quelli di un
diff cambiato (stesso numero, contenuto diverso), e senza il digest la patch sposterebbe
nell'indice righe che l'utente non ha visto. Gli id da soli non bastano in modo uniforme: nel
diff del worktree il nuovo id lo calcola git, ma nel `MemoryBackend` è finto. Il parametro è
obbligatorio perché chi sceglie le righe ha comunque appena letto il diff da cui vengono i
numeri. Non è un hash crittografico: protegge da un file cambiato, non da chi costruisce apposta
due diff con lo stesso digest.

## Costruttore di patch (dominio)

Per lo stage e l'unstage di singole righe: `build_patch(file: &FileDiff, lines: &[LineRef],
direction: PatchDirection) -> Result<Option<FileDiff>, DomainError>` (modulo
`crates/domain/src/git/patch.rs`), funzione pura. La patch restituita è un `FileDiff`
(`to_patch()` dà i byte) che si può applicare con `git apply` più gli argomenti di `APPLY_ARGS`
(`--cached --recount --whitespace=nowarn --no-ignore-whitespace`), patch su stdin; `--check` in
più per provarla. Le due flag sugli spazi neutralizzano `apply.whitespace=error` (rifiuterebbe
righe con spazi finali) e `apply.ignoreWhitespace` (accetterebbe un indice diverso negli spazi).
I comandi `index.stage_lines`/`index.unstage_lines` però **non** usano `git apply`: `git apply`
cerca il preimage di un hunk anche lontano dalla posizione scritta e ancora a fine file un hunk
senza contesto in coda, quindi con un diff a contesto zero (`diff.context=0`) un inserimento in
mezzo al file finiva in fondo e il comando riusciva. Il motore calcola il contenuto nuovo con
un applicatore rigoroso (`project_engine::apply_patch`: ogni hunk solo alla sua posizione,
contesto e righe tolte byte per byte, hunk senza contesto ammessi solo all'inizio o alla fine
del file) sul blob dell'indice da cui il diff è stato calcolato, e lo scrive (vedi "Backup dell'indice"). `APPLY_ARGS`
resta per i test che verificano le patch con git (`crates/git/tests/patch.rs`).

- **Direzioni.** `git apply --cached` (con cui i test del costruttore verificano le patch; i
  comandi usano l'applicatore rigoroso, che si comporta allo stesso modo ma solo alla posizione
  scritta) confronta il preimage della patch (contesto e righe `-`) con l'indice. La patch è sempre costruita *in avanti* rispetto all'indice, così le due
  direzioni si applicano allo stesso modo, **senza `--reverse`**.
  `stage` lavora sul diff di `diff.worktree`, il cui lato vecchio è l'indice: aggiunte
  selezionate `+`, aggiunte non selezionate tolte, rimosse selezionate `-`, rimosse non
  selezionate contesto. `unstage` lavora sul diff di `diff.index`, in cui l'indice è il lato
  **nuovo**: il diff si inverte e si applica la stessa regola, che rispetto alle righe mostrate
  diventa: aggiunte selezionate `-` (escono dall'indice), aggiunte non selezionate contesto
  (restano), rimosse selezionate `+` (tornano), rimosse non selezionate tolte.
- **Ordine nel risultato.** In ogni gruppo di modifiche fra due righe di contesto, tolte e
  aggiunte si accoppiano per posizione (prima tolta con prima aggiunta, ...): di `-x -y +x' +y'`
  lo stage di `x → x'` dà `x' y`. L'unstage di una selezione dà lo stesso contenuto dello stage
  delle righe non selezionate partendo da HEAD.
- **Selezione** (`LineRef`, in JSON `{"old": n}` / `{"new": n}`): il numero di riga nel lato in
  cui la riga cambiata esiste, come nel `Line` del diff (`old_line` per le tolte, `new_line` per
  le aggiunte); `Line::line_ref`, `Hunk::line_refs` e `FileDiff::line_refs` danno i riferimenti
  di una riga, di un hunk intero e del file. Selezione vuota: `Ok(None)`. Un riferimento che non
  è una riga cambiata del diff (contesto, numero inesistente, lato sbagliato):
  `DomainError::InvalidLineSelection`, mai una patch su altre righe.
- **Fine file senza a capo**: una riga senza a capo che nel risultato non è più l'ultima prende
  l'a capo (se era nel preimage diventa `-riga` + `\ No newline` e `+riga`).
- **File nuovo**: patch `new file mode` con le sole righe scelte. Nel diff worktree compare solo
  con `git add -N` (nei test del costruttore `git apply --cached` accetta di creare un file
  `intent-to-add`; i comandi scrivono la voce con `update-index --index-info`, che sostituisce
  il segnaposto con una voce normale); un file non
  tracciato non è nel diff e va messo in stage intero. Nell'unstage è il caso di un file
  eliminato in stage: torna nell'indice con le righe scelte.
- **File eliminato** (dal worktree nello stage, file nuovo in stage nell'unstage): tutte le righe
  ⇒ `deleted file mode`, il file esce dall'indice; selezione parziale ⇒ modifica, il file resta
  con le righe non selezionate.
- **Rinomine**: le patch per righe non rinominano. Nell'unstage (rinomina in stage) la patch
  modifica il percorso nuovo, quello nell'indice; il vecchio non torna (lo fa `index.unstage`
  del file). Nello stage (rinomina vista con `git add -N`) è rifiutata.
- **Cambi di modo**: mai nella patch, che non ha righe `old mode`/`new mode` né `index` e lascia
  il modo dell'indice; vanno con lo stage del file intero.
- **Intestazioni**: scritte da `FileDiff::write_header` (nomi tra virgolette dove servono, byte
  esatti), senza `index`, `similarity`, `rename`; conteggi degli hunk ricalcolati, quindi la
  patch è valida anche senza `--recount`. Contenuto (CRLF, byte non UTF-8) copiato byte per byte.
- **Rifiutati** con `DomainError::UnsupportedPatch` (prima di guardare la selezione): binari,
  sottomoduli, link simbolici, conflitti, copie, rinomine nello stage.

Test: snapshot `insta` delle patch su output reali di git (`testdata/patch.sh`), test di
proprietà con un applicatore puro (righe scelte applicate e le altre no, selezione totale = diff
originale, unstage = stage del complemento, stage poi unstage = contenuto iniziale) e test in
`crates/git/tests/patch.rs` che applicano le patch con `git apply --check` e poi davvero.

## Backup dell'indice

`index.stage` e `index.unstage` riscrivono la versione in stage dei file. Se un file toccato
(anche dentro una cartella richiesta) ha nell'indice una versione diversa da HEAD e dal worktree
(codice XY di `git status` con X ∉ {`.`, `?`} e Y ≠ `.`: `MM`, `AM`, `AD`, `MD`, `RM`, ...), dopo
la scrittura quella versione resterebbe solo in un blob irraggiungibile. Prima di scrivere il
backend la salva:

1. un indice temporaneo nella cartella git (`GIT_INDEX_FILE`) con solo quelle versioni
   (`update-index --index-info`), così funziona anche con conflitti nell'indice vero, e
   `write-tree`;
2. `commit-tree` con genitore HEAD (se c'è), messaggio con l'operazione e i percorsi, identità
   fissa (il backup riesce anche senza `user.name`) e `--no-gpg-sign`;
3. `update-ref refs/project/backup/index-<secondi>-<nanosecondi> <commit> ""`: il ref si crea solo
   se non esiste, un backup non ne sovrascrive mai un altro.

Per i file `--assume-unchanged` o `--skip-worktree` (lettere da `git ls-files -v`) `git status`
dà Y = `.` anche se il worktree è diverso: per loro basta X ∉ {`.`, `?`}. Lo stato letto prima di
scrivere non usa `core.fsmonitor` (un fsmonitor non aggiornato nasconderebbe le modifiche).
L'indice temporaneo si scrive con `core.splitIndex=false`, `core.fsmonitor=false` e
`index.sparse=false`, così non lascia file in `.git`.

Fra la lettura dello stato e la scrittura c'è una finestra breve in cui un altro processo git
potrebbe cambiare l'indice. Subito prima di scrivere il backend rilegge l'indice
(`ls-files -s`) e controlla che le voci toccate concordino con lo stato e con la prima lettura;
se no, errore senza scrivere. Resta solo l'istante fra quel controllo e la scrittura, e solo con
scritture git esterne concorrenti. Il `MemoryBackend` non ha i flag `assume-unchanged` e
`skip-worktree` né processi concorrenti: usa solo il codice XY.

`index.stage_lines` e `index.unstage_lines` sostituiscono anch'essi la versione in stage del
file e usano lo stesso criterio (`versions_at_risk*`, X ∉ {`.`, `?`} e Y ≠ `.`) sui percorsi
toccati (quello richiesto e i due del `FileDiff`): righe di un file `MM`, `AM`, `MD`, `AD`, `RM`
→ prima il backup della versione in stage; con `.M`, `M.`, `A.`, `.A` (intent-to-add) no,
perché la versione sostituita è anche in HEAD o nel worktree. Il `CliBackend`, in quest'ordine: legge
stato (senza fsmonitor) e indice (`ls-files -s`), poi il diff, e controlla che la voce
dell'indice sia quella su cui il diff è calcolato (id del lato indice: il vecchio per lo stage,
il nuovo per l'unstage; un `intent-to-add` deve esserci, con il blob vuoto, un file eliminato in
stage no); legge quel blob (`cat-file blob <id>`; file assente o `intent-to-add`: niente) e
calcola il contenuto nuovo con l'applicatore rigoroso, che non scrive niente (una patch che non
si applica non lascia nemmeno un backup); se esiste `index.lock` dà subito `IndexLocked` senza
fare il backup; salva il contenuto come blob con `hash-object -w --stdin --no-filters` (è già
nella forma dell'indice: niente filtri `clean` né conversioni di fine riga; scrive solo nel
database degli oggetti); salva il backup; ricontrolla l'indice come sopra; scrive la voce con
`update-index -z --index-info` (modo dell'indice, o quello della patch per un file nuovo o
`intent-to-add`, che diventa una voce normale; modo `0` toglie la voce se il file esce
dall'indice); rimette i flag `--skip-worktree`/`--assume-unchanged` che la voce aveva (vedi
sotto); infine rilegge la voce e controlla che sia quella scritta (se un altro git l'ha cambiata
subito dopo, errore che rimanda al backup). Fra il ricontrollo e la scrittura resta solo
`update-index`: con la CLI di git non c'è un vero compare-and-swap sull'indice (git prende il
lock solo dentro `update-index`), quindi l'istante fra il ricontrollo e quel lock resta scoperto,
come per stage e unstage. Perché è
sicuro: la voce dell'indice è proprio il lato del diff da cui vengono i numeri di riga (stesso
id), il contesto è fisso (`--unified=3`), il digest garantisce che il diff è quello visto e
l'applicatore non cerca mai il preimage altrove. Il worktree non si legge né si scrive mai; la
configurazione che potrebbe cambiare il risultato non entra in gioco (`diff.context` è
neutralizzato in `DIFF_ARGS`; `apply.*` e `core.whitespace` non servono più, perché non si usa
`git apply`). Un file `intent-to-add` (`.A`) ha in `git status` modo e id zero ma nell'indice una
voce con il blob vuoto (nel formato del repo): il ricontrollo dell'indice lo accetta solo con
quell'id (vale anche per `index.stage`). Oggi `Oid` e i parser supportano solo SHA-1 e un repo
SHA-256 non si apre: l'id SHA-256 del blob vuoto (come quello dell'albero vuoto) c'è solo per
non dare per scontato il formato, e non viene usato.

**Flag `--skip-worktree` e `--assume-unchanged`.** `update-index --index-info` e `restore
--staged` ricreano la voce dell'indice e ne perdono i flag (verificato: `S d/f` diventa
`H d/f`). In uno sparse checkout un file fuori dal cono con una modifica in stage passerebbe da
`M ` a `MD`, e un `git add -A` o `commit -a` successivo metterebbe in stage una cancellazione mai
fatta. Dopo la scrittura `index.stage_lines`, `index.unstage_lines` e `index.unstage` rimettono i
flag (`update-index --skip-worktree` / `--assume-unchanged`, percorsi su stdin) alle voci toccate
che li avevano nella lettura iniziale (`ls-files -v`: `S`/`s` skip-worktree, minuscola
assume-unchanged) e che sono ancora nell'indice; se non riesce, errore che lo dice (la voce
scritta è comunque corretta). `index.stage` (`git add`) non ne ha bisogno: conserva
`--assume-unchanged` e rifiuta i file `--skip-worktree` (fuori dallo sparse checkout) con un
errore, senza scrivere niente.

Se un passo fallisce l'indice non si tocca e il comando restituisce l'errore. Il risultato del
comando riporta il ref (`"backup"`), la UI lo mostra nella barra di stato; il contenuto si
recupera con `git show <ref>:<percorso>`. Lo stage di un file in conflitto non fa backup: git
tiene le versioni del conflitto nell'estensione REUC dell'indice (`git checkout -m` rifà il
conflitto), e l'unstage di un conflitto è rifiutato. Il `MemoryBackend` registra i backup in
memoria (`MemoryBackend::backups`).
