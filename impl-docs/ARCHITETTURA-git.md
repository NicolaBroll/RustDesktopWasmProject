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
| Diff | `git -c core.quotePath=false diff --no-color --no-ext-diff --no-textconv --find-renames --full-index --src-prefix=a/ --dst-prefix=b/ --submodule=short --no-relative` (costante `DIFF_ARGS`, più `--cached` o le revisioni) | formato unified diff, letto da `parse_diff` in `Diff` → `FileDiff` → `Hunk` → `Line` (righe in byte, con numeri di riga vecchio/nuovo e il marker "no newline" per riga). Gli argomenti neutralizzano la configurazione che cambierebbe il formato: colori, programmi esterni e `textconv`, `diff.noprefix`/`diff.mnemonicPrefix` (prefissi fissi `a/` `b/`), `diff.renames=false`, `diff.submodule=log` (un sottomodulo resta una riga `Subproject commit <id>`), `diff.relative`; `--full-index` dà id completi. Nomi tra virgolette con escape C (git li quota per `"`, `\`, a capo, tab... anche con `core.quotePath=false`) decodificati in byte; il numero di righe di ogni hunk viene dall'intestazione, così righe come `--- x` dentro un hunk non sono scambiate per intestazioni. I file in conflitto (`diff --cc`, `* Unmerged path`) sono riportati senza hunk. `Diff::to_patch` riscrive l'output identico, base del costruttore di patch |

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
(`to_patch()` dà i byte) da applicare con `git apply` più gli argomenti di `APPLY_ARGS`
(`--cached --recount --whitespace=nowarn --no-ignore-whitespace`), patch su stdin; `--check` in
più per provarla. Le due flag sugli spazi neutralizzano `apply.whitespace=error` (rifiuterebbe
righe con spazi finali) e `apply.ignoreWhitespace` (accetterebbe un indice diverso negli spazi).

- **Direzioni.** `git apply --cached` confronta il preimage della patch (contesto e righe `-`)
  con l'indice. La patch è sempre costruita *in avanti* rispetto all'indice, così le due
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
  con `git add -N` (`git apply --cached` accetta di creare un file `intent-to-add`); un file non
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
    // poi commit, checkout, ...
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
| `index.stage_lines` / `index.unstage_lines` | `{"path", "lines"}` | usa il costruttore di patch (§3): `lines` è una lista di `LineRef` (`{"old": n}` / `{"new": n}`) con i numeri di riga di `diff.worktree` (stage) o `diff.index` (unstage) per `path`; la patch si applica con `git apply` + `APPLY_ARGS` |
| `worktree.discard` | `{"paths", "confirm"}` | con backup (§7) |
| `commit.create` | `{"message", "amend"?}` | |
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
- Messaggi di commit via stdin (`git commit -F -`), mai come argomento.
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

Se un passo fallisce l'indice non si tocca e il comando restituisce l'errore. Il risultato del
comando riporta il ref (`"backup"`), la UI lo mostra nella barra di stato; il contenuto si
recupera con `git show <ref>:<percorso>`. Lo stage di un file in conflitto non fa backup: git
tiene le versioni del conflitto nell'estensione REUC dell'indice (`git checkout -m` rifà il
conflitto), e l'unstage di un conflitto è rifiutato. Il `MemoryBackend` registra i backup in
memoria (`MemoryBackend::backups`).

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
