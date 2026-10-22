# Client git grafico: architettura

Obiettivo: un client git desktop per Linux (poi Windows e macOS), open source, nello stile di
GitKraken: grafo dei commit, diff con stage per hunk e per riga, branch, remoti, conflitti,
rebase interattivo. Si costruisce **dentro questo workspace**, trasformando l'esempio "lista di
voci" passo dopo passo. Gli step sono in [ROADMAP-git.md](ROADMAP-git.md).

Restano valide le regole generali di [ARCHITETTURA-rust.md](ARCHITETTURA-rust.md) e di
[AGENTS.md](../AGENTS.md): livelli con dipendenze solo verso il basso, ogni modifica è un comando
del `Registry`, niente logica nei callback grafici, `cargo xtask ci` verde prima di consegnare.

Le versioni delle dipendenze citate (`gix` 0.89, `git2` 0.21, `tempfile` 3, `insta` 1, git ≥ 2.30)
sono quelle disponibili quando è stato scritto il documento: **verificale quando aggiungi la
dipendenza**.

## 1. Principi

1. **Non perdere mai dati dell'utente.** Ogni operazione che può cancellare lavoro (scartare
   modifiche, reset, eliminare branch non uniti, drop di uno stash) chiede `"confirm": true` nei
   parametri e prima salva un backup recuperabile (§7). Vale per UI, CLI e MCP allo stesso modo.
   Le operazioni che la UX di git fa senza conferma ma che possono riscrivere l'unica copia di
   qualcosa (stage e unstage di una versione in stage che non è né in HEAD né nel worktree)
   non chiedono `confirm`: salvano da sole un backup prima di scrivere e, se il backup non
   riesce, non scrivono (§7).
2. **git di sistema per scrivere, parser puri per leggere.** Le operazioni passano dal comando
   `git` installato: così funzionano subito chiavi SSH, `ssh-agent`, credential helper, hook,
   firma dei commit e la configurazione dell'utente. L'output di git (formati porcelain, `-z`) si
   legge con parser puri nel dominio, testabili senza processi. `gix` entra più avanti solo dove
   serve velocità (lettura del log su repo enormi, step S36).
3. **Il motore non sa da dove arrivano i dati.** Il backend git è un trait (§4). Il desktop usa
   il backend vero, i test e la versione web usano un backend in memoria con un repo di esempio.
4. **Mai bloccare la UI.** I comandi lenti (fetch, push, log grandi) girano fuori dal thread
   della UI (§6).
5. **Stesso catalogo per persone e agenti.** UI, CLI e MCP usano gli stessi id di comando.
   Un agente può preparare un commit e l'utente vede il risultato nel grafo.

## 2. Livelli

| Livello | Crate | Contenuto |
| --- | --- | --- |
| L0 | `project-core` | errori, parsing `id=<json>` (invariato) |
| L2 | `project-domain` | tipi git (`Oid`, `Commit`, `RefInfo`, `Status`, `Diff`), parser dell'output di git, algoritmo delle corsie del grafo, costruttore di patch |
| L5 | `project-engine` | `Session` del repo, `Registry` dei comandi, trait `GitBackend` e `RepoOpener`, `MemoryBackend`, trait `Platform` |
| L6 | `project-ui` | interfaccia egui (nessun `cfg(target_arch)`) |
| L6 | `project-mcp` | server MCP su stdio |
| L6 | `project-git` | **nuovo**: `CliBackend`, esegue `git` e usa i parser del dominio |
| L7 | `project`, `project-cli`, `project-web` | avvio e collegamento |

`project-git` sta a L6 perché implementa un trait del motore (L5). Nessun altro crate L6 lo
importa: lo collegano le app L7, come succede già per `Platform`. Va dichiarato in
`xtask/src/layers.rs` (step S06).

Chi usa cosa:

- **`project`** (desktop): `CliBackend` + `NativePlatform`.
- **`project-cli`**: `CliBackend`; `--repo <percorso>` apre il repo, `mcp --root <dir>` confina
  i percorsi.
- **`project-web`**: `MemoryBackend` con un repo dimostrativo. Il browser non può accedere ai
  repo locali, ma la build web resta utile per sviluppare e provare la UI (anche con Playwright
  nelle sessioni cloud) e mantiene vero il controllo wasm della CI.

I nomi `project-*` restano provvisori: rinominarli è una decisione separata, da fare in un solo
step dedicato se serve.

## 3. Dominio (L2)

Solo dati e funzioni pure, con `serde` dove servono parametri e risultati dei comandi.

```rust
pub struct Oid([u8; 20]);          // SHA-1; SHA-256 più avanti, se serve
pub struct Signature { pub name: String, pub email: String, pub time: i64, pub offset_min: i32 }
pub struct Commit {
    pub id: Oid,
    pub parents: Vec<Oid>,
    pub author: Signature,
    pub committer: Signature,
    pub summary: String,           // %s: primo paragrafo su una riga
    pub body: String,              // %b: il resto, senza a capo finali
}
pub enum RefKind { LocalBranch, RemoteBranch, Tag, Stash }
pub struct RefInfo { pub name: String, pub kind: RefKind, pub target: Oid, pub tag_object: Option<Oid>, pub upstream: Option<Upstream> }
// target: il commit (per un tag annotato quello puntato dal tag); tag_object: l'id del tag annotato
pub enum Head { Branch { name: String, target: Option<Oid> }, Detached(Oid) }
pub struct Status { pub head: Head, pub upstream: Option<Upstream>, pub entries: Vec<StatusEntry>, pub operation: Option<Operation> }
pub enum Operation { Merge, Rebase, CherryPick, Revert, Bisect }   // stato "in corso"
pub struct Diff { pub files: Vec<FileDiff> }                        // FileDiff → Hunk → Line
// FileDiff: percorsi, Change, modi, id, binario, hunk; Line: tipo, numeri di riga, contenuto in byte, no_newline
```

### Parser dell'output di git

Ogni comando git ha un formato scelto apposta per essere letto senza ambiguità:

| Dato | Comando | Note |
| --- | --- | --- |
| Log | `git log -z --date=raw --encoding=UTF-8 --no-show-signature --no-color --format=tformat:%H%x00%P%x00%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%s%x00%b` (costante `LOG_ARGS`) | 10 campi per record, **ognuno terminato da NUL** (il decimo NUL lo mette `-z`, anche dopo l'ultimo record): un messaggio git non può contenere NUL, quindi contare i campi è univoco. `%ad`/`%cd` con `--date=raw` danno `<secondi> <+hhmm>`, cioè anche il fuso orario che `%at`/`%ct` non hanno. `%s` è il primo paragrafo unito su una riga, `%b` il resto (a capo finali tolti dal parser) |
| Ref | `git for-each-ref --format=%(refname)%00%(objectname)%00%(*objectname)%00%(upstream)%00%(upstream:track,nobracket)%00%(symref)%00 refs/heads/ refs/remotes/ refs/tags/ refs/stash` (costante `REFS_ARGS`) | 6 campi per ref, **ognuno terminato da NUL**; git aggiunge un a capo dopo ogni record (non c'è `-z`), quindi il parser divide solo sui NUL e toglie l'a capo all'inizio del record seguente: i nomi dei ref non possono contenere NUL né a capo, ma `%(upstream)` viene dalla config e può contenere un a capo. Ref simbolici (`origin/HEAD`), con nome non UTF-8 o fuori dai quattro tipi: ignorati. Tag annotati: `target` dal `%(*objectname)` (risalendo i tag di tag presenti nella lista), `tag_object` dal `%(objectname)`. Ahead/behind: vuoto ⇒ 0/0, `gone` ⇒ `None` |
| Stato | `git status --porcelain=v2 -z --branch --untracked-files=all` | formato stabile per definizione |
| File di un commit | `git diff-tree -z -r --raw --no-abbrev -M --no-commit-id <primo genitore> <commit>` (costante `DIFF_TREE_ARGS`; `--root <commit>` per un commit radice) | un record per file: `:<modi> <id> <stato>` e uno o due percorsi (origine e destinazione per `R`/`C`), **ognuno terminato da NUL**. I merge si confrontano con il primo genitore (i file portati nel branch), come fa `commit.show` |
| Diff | `git -c core.quotePath=false diff --no-color --no-ext-diff --no-textconv --find-renames --full-index --src-prefix=a/ --dst-prefix=b/ --submodule=short --no-relative --unified=3` (costante `DIFF_ARGS`, più `--cached` o le revisioni) | formato unified diff, letto da `parse_diff` in `Diff` → `FileDiff` → `Hunk` → `Line` (righe in byte, con numeri di riga vecchio/nuovo e il marker "no newline" per riga). Gli argomenti neutralizzano la configurazione che cambierebbe il formato: colori, programmi esterni e `textconv`, `diff.noprefix`/`diff.mnemonicPrefix` (prefissi fissi `a/` `b/`), `diff.renames=false`, `diff.submodule=log` (un sottomodulo resta una riga `Subproject commit <id>`), `diff.relative`, `diff.context` (sempre tre righe di contesto: il diff mostrato, il suo digest e le patch per righe hanno lo stesso contesto, e un hunk senza contesto in testa o in coda tocca per forza l'inizio o la fine del file); `--full-index` dà id completi. Nomi tra virgolette con escape C (git li quota per `"`, `\`, a capo, tab... anche con `core.quotePath=false`) decodificati in byte; il numero di righe di ogni hunk viene dall'intestazione, così righe come `--- x` dentro un hunk non sono scambiate per intestazioni. I file in conflitto (`diff --cc`, `* Unmerged path`) sono riportati senza hunk. `Diff::to_patch` riscrive l'output identico, base del costruttore di patch |

I parser ricevono `&[u8]`, non `&str`: nomi di file e messaggi possono non essere UTF-8.
Si convertono con `String::from_utf8_lossy` solo per la visualizzazione, mai per i percorsi
passati di nuovo a git (tenerli come byte o `OsString`).

### Corsie del grafo

Funzione pura: commit in ordine topologico (dal più recente), risultato una riga per commit con
la colonna del commit e i segmenti verso la riga successiva.

1. Si tiene un vettore di corsie attive; ogni corsia "aspetta" un `Oid`.
2. Per ogni commit: la sua colonna è la prima corsia che lo aspetta, oppure una corsia libera.
   Le altre corsie che aspettavano lo stesso commit si chiudono (convergono qui).
3. La corsia del commit passa ad aspettare il primo genitore. Ogni altro genitore va su una
   corsia che lo aspetta già, oppure su una corsia libera (si apre una diramazione).

Invarianti da verificare con test di proprietà: ogni arco genitore–figlio è disegnato una sola
volta; nessun commit occupa due colonne; le corsie libere vengono riusate (larghezza limitata).

### Costruttore di patch

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
del file) sul blob dell'indice da cui il diff è stato calcolato, e lo scrive (§7). `APPLY_ARGS`
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

## 4. Motore (L5)

### Backend

```rust
pub trait GitBackend: Send {
    fn head(&self) -> Result<Head, EngineError>;
    fn refs(&self) -> Result<Vec<RefInfo>, EngineError>;
    fn log(&self, query: &LogQuery) -> Result<Vec<Commit>, EngineError>;
    fn status(&self) -> Result<Status, EngineError>;
    fn diff(&self, spec: &DiffSpec) -> Result<Diff, EngineError>;   // DiffSpec: Worktree | Index | Commit { rev }, ognuno con `path` facoltativo
    // Scritture (`&mut self`: il MemoryBackend tiene lo stato in sé), una per step:
    // Restituiscono il ref del backup dell'indice, se ne hanno salvato uno (§7).
    fn stage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError>;
    fn unstage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError>;
    // Stage/unstage di righe (`LineSelection`: path, lines, digest, direction).
    fn apply_lines(&mut self, selection: &LineSelection) -> Result<Option<String>, EngineError>;
    // Commit (`CommitRequest`: message, amend, confirm; `CommitOutcome`: commit, previous,
    // branch, backup).
    fn commit(&mut self, request: &CommitRequest) -> Result<CommitOutcome, EngineError>;
    // poi checkout, ...
}

pub trait RepoOpener: Send + Sync {
    fn open(&self, path: &str) -> Result<Box<dyn GitBackend>, EngineError>;
}
```

- `MemoryBackend` (nel motore): repo finto costruito in codice. Serve ai test dei comandi e alla
  versione web. Le scritture modificano lo stato in memoria.
- `CliBackend` (in `project-git`): esegue `git` (§5).

### Sessione

`Session` diventa la sessione di un repo: contiene il `Box<dyn GitBackend>` aperto (o nessuno),
una cache di quello che la UI mostra (stato, ref, prima pagina del log) e il **diario delle
operazioni** per l'undo (§7). La `Session` attuale con gli snapshot del documento si rimuove
quando l'esempio "lista di voci" viene tolto (step S08).

### Comandi

Ogni operazione è un comando con id stabile e parametri JSON. Le letture restituiscono i dati
come JSON, così CLI e MCP li usano direttamente. Catalogo iniziale:

| Id | Parametri | Note |
| --- | --- | --- |
| `repo.open` | `{"path"}` | usa il `RepoOpener` iniettato |
| `repo.status` | `{}` | |
| `repo.refs` | `{}` | |
| `repo.log` | `{"skip"?, "limit"?, "rev"?}` | paginato |
| `commit.show` | `{"id"}` | metadati e file cambiati rispetto al primo genitore |
| `diff.worktree` / `diff.index` / `diff.commit` | `{"path"?}` / `{"path"?}` / `{"id", "path"?}` | worktree rispetto all'indice (senza i non tracciati), indice rispetto a HEAD (albero vuoto se non ci sono commit), commit rispetto al primo genitore (albero vuoto per un commit radice, come `commit.show`). Risultato: il `Diff` (`{"files": [...]}`). `path` (stringa o array di byte, come `RepoPath`) limita al file con quel percorso esatto, vecchio o nuovo: una rinomina resta tale (il backend cerca prima l'altro percorso con `--diff-filter=R`) e i percorsi vanno a git dopo `--` con `--literal-pathspecs`. Nessun limite di dimensione |
| `index.stage` / `index.unstage` | `{"paths"}` | `paths`: lista non vuota di `RepoPath` validati come in `diff.*`, presi alla lettera (una cartella comprende i suoi file). Risultato: `{"status": Status, "backup": string \| null}`, con il nuovo stato (come `repo.status`) e il ref del backup dell'indice salvato prima di scrivere, se c'erano versioni in stage che esistevano solo nell'indice (§7). `stage` = `git add` (modifiche, file nuovi, cancellazioni; una rinomina fatta nel worktree si mette in stage con i due percorsi; una cancellazione già in stage si salta); prima una prova a vuoto (`--dry-run`), così un file ignorato chiesto per nome (`NotAllowed`, mai `-f`) o un percorso inesistente non lasciano niente a metà. Stage di un file in conflitto = risoluzione, solo se il file è chiesto per nome: dentro una cartella richiesta è rifiutato (`NotAllowed`). `unstage` = `git restore --staged --no-overlay --source=<HEAD>` (albero vuoto senza commit): solo l'indice, mai il worktree; un file in conflitto è rifiutato (`NotAllowed`), perché perderebbe gli stadi del conflitto. Indice bloccato da un altro git: `IndexLocked`, senza riprovare né cancellare il lock |
| `index.stage_lines` / `index.unstage_lines` | `{"path", "lines", "digest"}` | usa il costruttore di patch (§3) e l'applicatore rigoroso: `lines` è una lista **non vuota** di `LineRef` (`{"old": n}` / `{"new": n}`) con i numeri di riga di `diff.worktree` (stage) o `diff.index` (unstage) per `path`; `digest` (obbligatorio) è il campo `digest` di quel file nello stesso diff, che identifica il diff visto (vedi sotto). Il backend rilegge il diff del file, sceglie il `FileDiff` con `new_path == path` (altrimenti `old_path == path`: una rinomina chiesta con il vecchio nome), controlla il digest, costruisce la patch con `build_patch` (senza `--reverse`: la patch è sempre in avanti rispetto all'indice), rifiuta (`NotAllowed`) un hunk senza contesto in testa che non parte dall'inizio del file o senza contesto in coda che non è l'ultimo, calcola il contenuto nuovo con `apply_patch` sul blob in stage e lo scrive (`CliBackend`: `hash-object -w --stdin --no-filters` e `update-index -z --index-info`, §7; `MemoryBackend`: nei suoi blob). Errori, sempre senza scrivere: file senza più modifiche o digest diverso → `StaleDiff` (rileggere il diff); due `FileDiff` sullo stesso percorso (cambio di tipo) → `NotAllowed`; `UnsupportedPatch` del dominio (binari, sottomoduli, link, conflitti, copie, rinomine nello stage) → `NotAllowed`; `InvalidLineSelection` o `lines` vuota → `InvalidParams`; indice bloccato → `IndexLocked`. Backup come `index.stage` (§7). Risultato: `{"status": Status, "backup": string \| null}` |

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
| `worktree.discard` | `{"paths", "confirm"}` | con backup (§7) |
| `commit.create` | `{"message", "amend"?, "confirm"?}` | `git commit -F -` (§5). Controlli comuni ai backend (`check_commit_state`, `CommitRequest::check_message`, `check_amend_published`), sempre prima di scrivere: messaggio vuoto o di soli spazi (o con NUL) → `InvalidParams`; file in conflitto → `NotAllowed`; rebase, cherry-pick o revert in corso → `NotAllowed` (un commit da qui li concluderebbe o cambierebbe in modo poco visibile: un cherry-pick prenderebbe l'autore originale); niente in stage (indice uguale a HEAD, file `intent-to-add` esclusi) senza `amend` e senza merge → `NothingToCommit`, mai un commit vuoto. Merge in corso: il commit è il commit di merge (genitori HEAD e `MERGE_HEAD`), anche senza modifiche. Bisect: nessun controllo. Primo commit di un branch e HEAD staccata funzionano (con HEAD staccata si sposta solo HEAD). Identità mancante: l'errore di git, nessun default. `amend`: stessi genitori e autore, albero dall'indice, messaggio nuovo (anche solo il messaggio); rifiutato senza commit, durante un merge e se renderebbe vuoto il commit (errore di git); un merge si riscrive tenendo i due genitori. Se il commit riscritto è raggiungibile da un branch remoto (`for-each-ref --contains <HEAD> refs/remotes/`, senza i ref simbolici) serve `"confirm": true`, altrimenti `ConfirmRequired` che elenca i branch; un commit non pubblicato si riscrive senza conferma (è nel reflog e nel backup). Prima di scrivere, con `amend`, un ref di backup sul commit riscritto (§7). Risultato: `{"commit": Commit, "previous": oid \| null, "branch": string \| null, "backup": string \| null, "status": Status}`: il commit come in `repo.log`, HEAD prima, il branch spostato con il nome completo (`refs/heads/main`; `null` con HEAD staccata), il ref di backup (solo `amend`) e il nuovo stato |
| `branch.create` / `branch.checkout` / `branch.delete` | `{"name", ...}` | `delete` di un branch non unito chiede `confirm` |
| `remote.fetch` / `remote.pull` / `remote.push` | `{"remote"?, "branch"?}` | lenti: vedi §6 |
| `edit.undo` / `edit.redo` | `{}` | sul diario delle operazioni |

Il catalogo cresce con gli step. Gli id, una volta pubblicati, non si rinominano.

## 5. Backend `git` (L6, `project-git`)

Regole per ogni chiamata a git:

- **Mai una shell.** `std::process::Command` con argomenti separati.
- Sempre `git -C <repo> -c core.quotePath=false -c color.ui=false ...`.
- Prima di valori che vengono dall'utente (nomi di branch, rev): `--end-of-options`, così un nome
  che inizia con `-` non diventa un'opzione. Prima dei percorsi: `--`. I percorsi si passano
  alla lettera (`--literal-pathspecs`: niente glob né pathspec magici come `:(top)`), e le
  variabili `GIT_*_PATHSPECS` ereditate si tolgono.
- Ambiente: `GIT_TERMINAL_PROMPT=0` (git non deve mai aspettare input da un terminale che non
  c'è), `LC_ALL=C` per i messaggi di errore da riconoscere. Il resto dell'ambiente si eredita
  (SSH agent, credential helper).
- Messaggi di commit via stdin (`git commit -F -`), mai come argomento. `commit.create` esegue
  `git -c i18n.commitEncoding=UTF-8 commit --no-edit -F - [--amend]` con `GIT_EDITOR=:`:
  - hook (`pre-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit`, `post-rewrite`) e
    firma (`commit.gpgSign`, `gpg.format`, `user.signingKey`) seguono la configurazione: mai
    `--no-verify`, `--no-gpg-sign`, `--allow-empty` o `--allow-empty-message`;
  - nessun editor (`--no-edit` e `GIT_EDITOR=:`, il valore con cui git non ne avvia): così
    `core.editor`, `commit.verbose`, `commit.template` (con `-F` comunque ignorato) e
    `commit.status` non possono aprire un editor che aspetterebbe un terminale;
  - la pulizia del messaggio è quella di `commit.cleanup`; senza configurazione, con `-F` git
    usa `whitespace` (le righe `#` restano: `#123 corregge` è un messaggio valido). Se la
    pulizia lo rende vuoto (es. `strip` e solo commenti) git rifiuta: `InvalidParams`;
  - il messaggio arriva da JSON ed è UTF-8: `i18n.commitEncoding=UTF-8` evita che una
    configurazione diversa lo etichetti con un'altra codifica;
  - un hook che rifiuta o una firma che non riesce: `EngineError::Git` con stderr e stdout di
    git (git manda su stderr anche l'output degli hook), nessun commit, indice invariato
    (tranne quello che l'hook stesso ha cambiato: un `pre-commit` che fa `git add` cambia
    l'indice, e se riesce quello che viene committato, come con `git commit`). Indice
    bloccato: `IndexLocked`;
  - hook e firma possono essere lenti: il comando resta sincrono (§6), stdout e stderr si
    leggono insieme e stdin si scrive da un altro thread, quindi un hook che stampa molto non
    blocca. Nessun timeout: una pinentry grafica di gpg o la conferma di una chiave ssh
    aspettano legittimamente l'utente; senza terminale né pinentry grafica gpg fallisce subito.
    Se l'app è avviata da un terminale, una pinentry testuale può usare quel terminale;
  - dopo il commit il backend rilegge HEAD e controlla che sia un commit nuovo sopra il
    `previous` letto prima (con `amend`: con gli stessi genitori); se un altro processo o un
    hook `post-commit` lo ha spostato, errore che lo dice (il commit resta).
- Le liste di percorsi delle scritture vanno via stdin (`--pathspec-from-file=-
  --pathspec-file-nul`, con `--literal-pathspecs`): nessun limite della riga di comando e nessun
  percorso letto come opzione. Una lista vuota non arriva mai a git (per molti comandi vorrebbe
  dire "tutto il repo").
- Le letture non scrivono l'indice: `--no-optional-locks`, e per `git diff` anche
  `-c diff.autoRefreshIndex=false` (altrimenti riscrive l'indice per aggiornare i dati di `stat`).
- Variabili d'ambiente tolte sempre, oltre a quelle del repo (`GIT_DIR`, `GIT_WORK_TREE`,
  `GIT_INDEX_FILE`, ...) e dei pathspec: la configurazione ereditata `GIT_CONFIG_PARAMETERS`,
  `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_*`/`GIT_CONFIG_VALUE_*` (un `core.worktree` iniettato
  sposterebbe le scritture).
- Limiti noti, voluti: la configurazione del repo e dell'utente può far eseguire comandi a git
  (filtri `clean`/`smudge`, `core.fsmonitor`, hook come `reference-transaction`): è la
  configurazione dell'utente e si rispetta, come in ogni client che usa git di sistema; aprire
  un repo non fidato è come eseguire git in quel repo. `index.stage` di una cartella che è un
  repo annidato aggiunge un gitlink (come `git add`, che lo segnala solo con un avviso).
- Un comando che modifica il repo è segnato in `CommandSpec::writes` (CLI `commands`, MCP
  `list_commands`), così la UI lo disabilita durante un job e un agente sa cosa scrive.
- All'apertura del repo si verifica la versione di git (minimo 2.30) e si dà un errore chiaro se
  è più vecchia.
- Gli errori di git diventano varianti di `EngineError` con il testo di stderr, così UI e agenti
  vedono il motivo vero.

## 6. Comandi lenti e UI non bloccante

`Registry::run` resta sincrono: è semplice da testare e da usare da CLI e MCP.

La UI non lo chiama direttamente sul suo thread per i comandi lenti. `Platform` ottiene un
metodo per eseguire lavoro in background:

```rust
fn spawn(&self, job: Box<dyn FnOnce() + Send>);
```

- Desktop: un thread (o un piccolo pool).
- Web: esegue subito (il `MemoryBackend` è istantaneo).

La sessione sta dietro un `Arc<Mutex<...>>` condiviso con il job; il risultato torna alla UI con
un canale e la UI chiede un nuovo frame (`ctx.request_repaint()`). Durante un job la UI mostra
l'avanzamento e disabilita i comandi che modificano il repo. L'avanzamento di fetch e push si
legge da stderr con `--progress`.

## 7. Undo e backup

Il diario delle operazioni registra, per ogni comando che sposta ref, i valori prima e dopo:
`HEAD`, il branch coinvolto, l'oid. `edit.undo` riporta i ref ai valori precedenti, solo se nel
frattempo non sono cambiati (altrimenti errore chiaro, nessuna sovrascrittura).

Prima di scartare modifiche non committate si crea un backup con `git stash create` e se ne
salva l'oid nel diario (con un ref `refs/project/backup/<data>` perché il garbage collector di git
non lo elimini). Così anche "scarta" si può annullare.

### Backup dell'indice (stage e unstage)

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

### Backup del commit riscritto (`amend`)

`commit.create` con `amend` sostituisce HEAD con un commit nuovo: il vecchio resta solo nel
reflog, che scade. Prima di eseguire `git commit --amend` il backend crea
`refs/project/backup/amend-<secondi>-<nanosecondi>` sul commit riscritto (`update-ref <ref>
<id> ""`, solo se non esiste, come per l'indice) e lo restituisce in `"backup"`; se il ref non
si crea non si committa. Se poi `git commit` fallisce e HEAD è ancora il commit letto prima, il
ref si toglie (`update-ref -d <ref> <id>`): non c'è niente da recuperare. Un commit nuovo non
salva backup: il commit precedente è il suo genitore. Il `MemoryBackend` registra i backup in
`MemoryBackend::commit_backups`.

Per il diario (S22) `commit.create` restituisce già i valori prima e dopo del ref spostato:
`branch` (o `null` con HEAD staccata), `previous` e `commit.id`.

L'undo non copre le operazioni sui remoti: un push non si annulla da qui.

## 8. Interfaccia (L6)

Layout come GitKraken:

- **Barra in alto**: repo aperto, pulsanti fetch/pull/push, undo/redo, stato del job in corso.
- **Colonna sinistra**: branch locali, remoti, tag, stash.
- **Centro**: grafo e lista dei commit, con riga "modifiche non committate" in cima. Lista
  virtualizzata (`ScrollArea::show_rows`): si disegnano solo le righe visibili, il log si carica
  a pagine mentre si scorre.
- **Destra**: dettagli del commit selezionato, oppure area di stage (file non in stage e in stage,
  messaggio di commit) quando è selezionata la riga delle modifiche.
- **Diff**: sotto o al posto del grafo, con selezione di hunk e righe.

Ogni interazione produce un `Action` che passa da `Registry::run`, come oggi.

## 9. Test

- **Dominio**: test unitari dei parser con output reali di git salvati come file di esempio
  (anche con nomi non UTF-8, spazi, rinomine), test di proprietà per le corsie del grafo,
  snapshot (`insta`) per le patch generate.
- **Motore**: comandi eseguiti sul `MemoryBackend`.
- **`project-git`**: test di integrazione su repo veri creati in cartelle temporanee
  (`tempfile`) con un helper di scenari. Ambiente deterministico: `GIT_CONFIG_GLOBAL=/dev/null`,
  `GIT_CONFIG_NOSYSTEM=1`, autore e date fissi (`GIT_AUTHOR_DATE`, `GIT_COMMITTER_DATE`).
  Scenari da coprire nel tempo: repo vuoto, HEAD staccata, merge in conflitto, rebase interrotto,
  submodule, worktree, file binari, fine riga CRLF, nomi non UTF-8.
- **CLI**: test end-to-end come `apps/project-cli/tests/cli.rs`.
- **UI**: build web con il `MemoryBackend` e screenshot con Playwright per controllare il
  layout.

## 10. Agenti

Definiti in `.claude/agents/`. Ognuno lavora sul proprio livello e chiude con `cargo xtask ci`.

| Agente | Lavora su | Quando usarlo |
| --- | --- | --- |
| `git-domain` | `crates/core`, `crates/domain` | tipi, parser, grafo, patch: codice puro |
| `git-engine` | `crates/engine`, `crates/git`, `crates/mcp`, `apps/project-cli` | backend, comandi, sessione, CLI e MCP |
| `egui-ui` | `crates/ui`, `apps/project`, `apps/project-web` | interfaccia e collegamento nelle app |
| `git-scenarios` | test in `crates/git/tests` | scenari su repo veri e casi limite |
| `safety-reviewer` | sola lettura | revisione di ogni step che scrive nel repo dell'utente |
