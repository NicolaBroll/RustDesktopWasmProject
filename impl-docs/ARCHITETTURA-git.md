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
    pub summary: String,
    pub body: String,
}
pub enum RefKind { LocalBranch, RemoteBranch, Tag, Stash }
pub struct RefInfo { pub name: String, pub kind: RefKind, pub target: Oid, pub upstream: Option<Upstream> }
pub enum Head { Branch { name: String, target: Option<Oid> }, Detached(Oid) }
pub struct Status { pub head: Head, pub entries: Vec<StatusEntry>, pub operation: Option<Operation> }
pub enum Operation { Merge, Rebase, CherryPick, Revert, Bisect }   // stato "in corso"
pub struct Diff { pub files: Vec<FileDiff> }                        // FileDiff → Hunk → Line
```

### Parser dell'output di git

Ogni comando git ha un formato scelto apposta per essere letto senza ambiguità:

| Dato | Comando | Note |
| --- | --- | --- |
| Log | `git log -z --format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%s%x00%b` | campi e record separati da NUL, **numero fisso di campi** per record: un messaggio git non può contenere NUL, quindi è univoco |
| Ref | `git for-each-ref --format=...` con `%00` | include upstream e ahead/behind (`%(upstream:track)`) |
| Stato | `git status --porcelain=v2 -z --branch --untracked-files=all` | formato stabile per definizione |
| Diff | `git -c core.quotePath=false diff --no-color --no-ext-diff --find-renames` | parser del formato unified diff |

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

Per lo stage di singole righe: dato un `FileDiff` e le righe selezionate produce una patch valida
per `git apply --cached --recount`. Le righe aggiunte non selezionate si tolgono, quelle rimosse
non selezionate diventano righe di contesto. Funzione pura, con test sui casi limite: file nuovi,
fine file senza newline, hunk adiacenti.

## 4. Motore (L5)

### Backend

```rust
pub trait GitBackend: Send {
    fn head(&self) -> Result<Head, EngineError>;
    fn refs(&self) -> Result<Vec<RefInfo>, EngineError>;
    fn log(&self, query: &LogQuery) -> Result<Vec<Commit>, EngineError>;
    fn status(&self) -> Result<Status, EngineError>;
    fn diff(&self, spec: &DiffSpec) -> Result<Diff, EngineError>;
    // Le scritture si aggiungono uno step alla volta: stage, commit, checkout, ...
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
| `commit.show` | `{"id"}` | metadati e file cambiati |
| `diff.worktree` / `diff.index` / `diff.commit` | `{"path"?}` / `{"id"}` | |
| `index.stage` / `index.unstage` | `{"paths"}` | |
| `index.stage_lines` / `index.unstage_lines` | `{"path", "lines"}` | usa il costruttore di patch |
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
  che inizia con `-` non diventa un'opzione. Prima dei percorsi: `--`.
- Ambiente: `GIT_TERMINAL_PROMPT=0` (git non deve mai aspettare input da un terminale che non
  c'è), `LC_ALL=C` per i messaggi di errore da riconoscere. Il resto dell'ambiente si eredita
  (SSH agent, credential helper).
- Messaggi di commit via stdin (`git commit -F -`), mai come argomento.
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
