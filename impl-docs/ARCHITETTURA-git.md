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
    // Undo/redo (§7): sposta HEAD e ref da `expected` a `target`, tutti o nessuno, solo se ora
    // valgono `expected`; restituisce i ref di backup creati.
    fn restore_refs(&mut self, request: &RefRestore) -> Result<Vec<String>, EngineError>;
    // Branch (S24): nomi brevi, già controllati dal comando.
    fn unmerged_count(&self, tip: &Oid, into: &[Oid]) -> Result<usize, EngineError>;
    fn create_branch(&mut self, name: &str, target: &Oid) -> Result<(), EngineError>;
    fn checkout(&mut self, target: &HeadState) -> Result<RestoreOutcome, EngineError>;
    fn delete_branch(&mut self, name: &str, expected: &Oid) -> Result<BranchDeleted, EngineError>;
    fn rename_branch(&mut self, from: &str, to: &str, expected: &Oid)
        -> Result<RestoreOutcome, EngineError>;
    // Remoti (S25): nomi dei remoti configurati e fetch (lento: avanzamento e annullamento da
    // `job`, §6). `FetchRequest`: remote (già controllato), branch; `FetchOutcome`: messages.
    fn remotes(&self) -> Result<Vec<String>, EngineError>;
    fn fetch(&mut self, request: &FetchRequest, job: &JobContext)
        -> Result<FetchOutcome, EngineError>;
    // Pull e push (S26). Letture: upstream configurato (`UpstreamConfig`: remote, merge,
    // tracking), configurazione di pull (`PullSettings`: rebase, ff, valori grezzi), commit di
    // `tip` fuori da `exclude` (per la conferma di un force).
    fn upstream(&self, branch: &str) -> Result<Option<UpstreamConfig>, EngineError>;
    fn pull_settings(&self, branch: &str) -> Result<PullSettings, EngineError>;
    // Tutti i valori di una chiave (`git config --get-all`): `branch.<b>.pushRemote`,
    // `remote.pushDefault`, `remote.<r>.mirror`, `remote.<r>.fetch`.
    fn config_values(&self, key: &str) -> Result<Vec<String>, EngineError>;
    fn missing_commits(&self, tip: &Oid, exclude: &[Oid], limit: usize)
        -> Result<Vec<Commit>, EngineError>;
    // L'integrazione dopo il fetch (`PullRequest`: branch, current, expected, upstream,
    // mode = FastForward | Merge { no_ff } | Rebase { merges }, message), l'undo/redo di un
    // pull del branch attivo (come `git reset --keep`) e il push (`PushRequest`: remote,
    // branch, remote_branch, set_upstream, lease, backup).
    fn pull(&mut self, request: &PullRequest) -> Result<PullOutcome, EngineError>;
    fn reset_branch(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError>;
    fn push(&mut self, request: &PushRequest, job: &JobContext)
        -> Result<PushOutcome, EngineError>;
    // Gestione dei remoti (S27): cosa c'è di un remoto (`RemoteSnapshot`: `remote.<r>.*` del
    // file del repo, branch remoti con i commit, `refs/remotes/<r>/HEAD`, upstream/pushRemote
    // che lo usano, `remote.pushDefault`), `git remote add`, e la configurazione tolta o
    // rimessa da `remove_remote`/`restore_remote` del motore (i branch remoti passano da
    // `restore_refs`).
    fn remote_snapshot(&self, name: &str) -> Result<RemoteSnapshot, EngineError>;
    fn create_remote(&mut self, name: &str, url: &str) -> Result<(), EngineError>;
    fn remove_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<String>;
    fn write_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<String>;
    // Stash (S28): l'elenco dal reflog di `refs/stash` (`StashEntry`: index, name, id, base,
    // message, branch, time, untracked); push (`StashPush`: message, include_untracked,
    // keep_index, paths), apply (`StashApply`: id, restore_index; `StashApplied`: conflicts),
    // drop (`StashDrop`: id, tipo del backup) e store (l'undo di un drop). Uno stash arriva
    // sempre come id: la posizione la ricalcola il backend.
    fn stashes(&self) -> Result<Vec<StashEntry>, EngineError>;
    fn stash_push(&mut self, request: &StashPush) -> Result<StashEntry, EngineError>;
    fn stash_apply(&mut self, request: &StashApply) -> Result<StashApplied, EngineError>;
    fn stash_drop(&mut self, request: &StashDrop) -> Result<StashDropped, EngineError>;
    fn stash_store(&mut self, id: &Oid, message: &str) -> Result<(), EngineError>;
    // poi merge, rebase, ...
}

pub trait RepoOpener: Send + Sync {
    fn open(&self, path: &str) -> Result<Box<dyn GitBackend>, EngineError>;
}
```

- `MemoryBackend` (nel motore): repo finto costruito in codice. Serve ai test dei comandi e alla
  versione web. Le scritture modificano lo stato in memoria. Ha anche remoti in memoria
  (`add_remote`, `set_branch_on_remote`, `insert_remote_commit`, `fail_fetch` per simulare
  `Auth`/`RemoteUnreachable`): il fetch copia i commit raggiungibili e aggiorna
  `refs/remotes/<remote>/<branch>` (senza prune). Nel repo di esempio `origin` ha un commit in
  più su `feature/grafo` (un pull di `feature/grafo` lo porta avanti) e `main` è avanti di un
  commit rispetto a `origin/main` (un push lo pubblica). Pull e push (S26): merge file per file
  (un file cambiato dai due lati è un conflitto), rebase dei commit senza merge, push che copia
  i commit nel remoto in memoria; `set_pull_settings` dà `pull.rebase`/`pull.ff`,
  `fail_pre_push` simula un hook `pre-push` che rifiuta, `branch_on_remote` legge il remoto. Remoti (S27):
  `remote.<r>.url`/`.fetch` vengono dal remoto in memoria, le altre chiavi da `set_config`,
  gli upstream da `set_upstream`; nessun ref simbolico. Togliendo un remoto il suo "server"
  (branch e commit del remoto) resta da parte per URL, e un remoto aggiunto o rimesso con lo
  stesso URL lo ritrova. Stash (S28): ogni stash tiene le voci dello stato e il contenuto dei
  file messi da parte, più un commit (con i file cambiati rispetto alla base, così `commit.show`
  sull'id funziona); `set_stash` aggiunge uno stash senza file. Più severo di git: un file
  cambiato dallo stash e da HEAD rispetto alla base è sempre un conflitto (senza marcatori), e
  con `paths` lo stash contiene solo i file scelti (git ci mette tutto l'indice, vedi
  `stash.push`). Gli stash eliminati restano da parte per id, per l'undo del drop;
  `fail_stash_drop` simula un drop che fallisce (per il pop applicato e non tolto).
- `CliBackend` (in `project-git`): esegue `git` (§5).

### Sessione

`Session` è la sessione di un repo: contiene il `Box<dyn GitBackend>` aperto (o nessuno) e il
**diario delle operazioni** (`Journal`) per l'undo (§7), che appartiene al repo aperto: aprire un
repo, anche lo stesso, ne parte uno vuoto (gli id delle voci continuano). `repo_and_journal()`
dà backend e diario insieme ai comandi che scrivono e registrano l'operazione. La cache di
quello che la UI mostra (stato, ref, prima pagina del log) per ora sta nella UI. La `Session`
con gli snapshot del documento dell'esempio originale è stata tolta (S08): undo e redo
esistono solo sul diario.

### Comandi

Ogni operazione è un comando con id stabile e parametri JSON. Le letture restituiscono i dati
come JSON, così CLI e MCP li usano direttamente. Catalogo iniziale:

| Id | Parametri | Note |
| --- | --- | --- |
| `repo.open` | `{"path"}` | usa il `RepoOpener` iniettato |
| `repo.status` | `{}` | |
| `repo.refs` | `{}` | |
| `repo.log` | `{"skip"?, "limit"?, "rev"?, "all"?, "stashes"?}` | paginato (`limit` 200 se manca, risultato `{"commits", "has_more"}`). `git log --date-order` (figli prima dei genitori, poi il più recente per data del committer; vale anche con più punti di partenza, e le corsie del grafo §3 gestiscono più punte e più radici). Da HEAD, oppure da `rev` (ref, `HEAD` o id, dopo `--end-of-options`); HEAD senza commit dà un log vuoto. `"all": true`: da tutti i branch locali, i branch remoti, i tag e HEAD (`--branches --remotes --tags` e l'id di HEAD, così una HEAD staccata compare; una HEAD senza commit si salta): mostra i commit dei branch non uniti e quelli appena scaricati su `origin/*`. Restano fuori `refs/stash`, i backup in `refs/project/backup/` e ogni altro ref fuori da `refs/heads/`, `refs/remotes/`, `refs/tags/` (es. le note); un tag su un albero o un blob si ignora. `"stashes": true` (solo con `all`, altrimenti `InvalidParams`): parte anche dalle basi degli stash (`StashEntry::base`, il primo genitore), così il commit a cui la UI aggancia uno stash è sempre nel grafo, anche se nessun branch lo contiene più; i commit degli stash (e quelli dell'indice e dei non tracciati che hanno per genitori) restano fuori, l'elenco è `stash.list`. `rev` e `all` insieme → `InvalidParams`. `skip`/`limit` paginano nello stesso ordine, quindi le pagine sono contigue |
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
| `commit.create` | `{"message", "amend"?, "confirm"?}` | `git commit -F -` (§5). Controlli comuni ai backend (`check_commit_state`, `CommitRequest::check_message`, `check_amend_published`), sempre prima di scrivere: messaggio vuoto o di soli spazi (o con NUL) → `InvalidParams`; file in conflitto → `NotAllowed`; rebase, cherry-pick, revert o `git am` in corso (anche un cherry-pick/revert di più commit fermo fra un commit e l'altro: `.git/sequencer`) → `NotAllowed` (un commit da qui li concluderebbe o cambierebbe in modo poco visibile: un cherry-pick prenderebbe l'autore originale); niente in stage (indice uguale a HEAD, file `intent-to-add` esclusi) senza `amend` e senza merge → `NothingToCommit`, mai un commit vuoto. Merge in corso: il commit è il commit di merge (genitori HEAD e `MERGE_HEAD`), anche senza modifiche. Bisect: nessun controllo. Primo commit di un branch e HEAD staccata funzionano (con HEAD staccata si sposta solo HEAD). Identità mancante: l'errore di git, nessun default. `amend`: stessi genitori e autore, albero dall'indice, messaggio nuovo (anche solo il messaggio); rifiutato senza commit, durante un merge e se renderebbe vuoto il commit (errore di git); un merge si riscrive tenendo i due genitori. Se il commit riscritto è raggiungibile da un branch remoto o da un tag (`for-each-ref --contains <HEAD> refs/heads/ refs/remotes/ refs/tags/`, senza i ref simbolici; divisione in `amend_impact`) serve `"confirm": true`, altrimenti `ConfirmRequired` che elenca i ref (un tag resterebbe sul commit vecchio); un commit non pubblicato si riscrive senza conferma (è nel reflog e nel backup). "Pubblicato" si vede solo dai ref di tracciamento: un push per URL, senza `refs/remotes/...`, non è visibile e il commit risulta non pubblicato. Gli altri branch locali e i tag che contengono il commit riscritto finiscono in `also_on`: dopo l'`amend` restano sulla storia vecchia. Un `amend` che produce lo stesso commit (stesso albero, messaggio, autore e date: es. due `amend` uguali nello stesso secondo) è un successo senza cambiamenti: `commit.id == previous`, `backup` `null`. Prima di scrivere, con `amend`, un ref di backup sul commit riscritto (§7). Risultato: `{"commit": Commit, "previous": oid \| null, "branch": string \| null, "backup": string \| null, "also_on": [string], "status": Status}`: il commit come in `repo.log`, HEAD prima, il branch spostato con il nome completo (`refs/heads/main`; `null` con HEAD staccata), il ref di backup (solo `amend`), gli altri branch e tag rimasti sul commit riscritto (solo `amend`, altrimenti vuoto) e il nuovo stato. Un problema **dopo** un commit riuscito (HEAD spostato da un hook `post-commit` o da un altro processo, reflog che non torna, stato non rileggibile) è `PostCommit { commit, backup, reason }`: il commit non va ripetuto, `commit` è il commit creato se noto (`null` se HEAD non è più quello), `backup` il ref del commit riscritto, che resta |
| `branch.create` | `{"name", "start"?}` | crea `refs/heads/<name>` su `start` (una rev; HEAD se manca, errore `NotAllowed` se HEAD non ha commit) senza cambiare branch e senza upstream; mai sopra un ref esistente (`NotAllowed`). Nome nuovo controllato (`check_new_branch_name` e, nel `CliBackend`, `git check-ref-format --branch`, che deve restituire il nome identico): niente `-` iniziale, `HEAD`, `@`, nomi completi `refs/...` → `InvalidParams`. Risultato: `{"branch", "target", "refs", "can_undo", "can_redo", "next_undo", "next_redo"}` (vedi §7 per tutti i `branch.*`) |
| `branch.checkout` | `{"name"}` | passa al branch locale `name` (`git switch`, mai forzato). Risultato: `{"branch", "previous": HeadState, "changed", "warnings", "status", ...diario}`. Modifiche locali che verrebbero sovrascritte → `CheckoutConflict { target, files, stderr }` e niente cambia; operazione in corso o file in conflitto → `NotAllowed`; già sul branch: `changed: false` |
| `branch.delete` | `{"name", "confirm"?, "expected"?}` | `expected`: id **completo** (40 cifre, altrimenti `InvalidParams`) del commit su cui il branch è stato mostrato; se il branch punta altrove → `NotAllowed` ("si è spostato da quando è stato visto: ... ora punta a ...") prima della conferma e dell'eliminazione, niente cambia: la conferma vale solo per il commit visto (senza `expected` nessun controllo in più). Branch non unito (commit non raggiungibili né da HEAD né dal suo upstream) → `ConfirmRequired` senza `confirm`; branch di HEAD, aperto in un altro worktree, operazione in corso → `NotAllowed`. Backup sempre. Risultato: `{"branch", "target", "unmerged", "backup", "removed_config": [{"key", "value"}], "warnings", "refs", ...diario}` |
| `branch.rename` | `{"name", "new_name"}` | `git branch -m` (mai `-M`): reflog, upstream e HEAD seguono; nome nuovo controllato come in `create`, mai sopra un branch esistente (`NotAllowed`). Risultato: `{"branch", "previous", "target", "head", "warnings", "refs", ...diario}` |
| `remote.fetch` | `{"remote"?, "branch"?}` | lento (§6), scrive (`writes`: branch remoti, tag, `FETCH_HEAD`). `remote`: il **nome** di un remoto configurato (`GitBackend::remotes`, cioè `git remote`), controllato con `check_remote_name` (come git: `refs/remotes/<nome>/x` dev'essere un nome di ref valido; un `-` iniziale è ammesso) e cercato fra i configurati prima di chiamare il backend: un nome che non c'è → `NotAllowed` con l'elenco dei remoti (mai passato a git, che lo userebbe come URL o percorso). Senza `remote`: quello dell'upstream del branch attivo, altrimenti `origin`, altrimenti l'unico remoto (`default_remote`; nessuno o ambiguo → `NotAllowed`). `branch`: nome breve di un branch **del remoto** (`check_branch_name`), scaricato da solo come `refs/heads/<branch>`; il remoto non lo ha → `NotAllowed`. Errori: credenziali rifiutate o da chiedere → `Auth { remote, message }`; remoto non raggiungibile (cartella che non c'è, host, rete, repository inesistente sul server) → `RemoteUnreachable { remote, message }`; annullato → `Cancelled`; altro → `Git`. Risultato: `{"remote", "branch": string \| null, "updated": [{"name", "before": oid \| null, "after": oid \| null}] \| null, "messages": [string], "warnings": [string], "refs": [RefInfo] \| null, "refs_error"?}`: `updated` sono i ref cambiati (confronto di `refs()` prima e dopo, un tag annotato con il commit puntato; `null` con `refs_error` se i ref non si rileggono: il fetch è fatto), `messages` le righe di git che non sono avanzamento (`From <url>`, ` * [new branch] ...`), `warnings` i ref fuori da `refs/remotes/` e `refs/tags/` spostati da un refspec configurato (es. un mirror verso `refs/heads/`). Non va nel diario (§7). Dettagli del `CliBackend` in §5 |
| `remote.pull` | `{"remote"?, "branch"?, "strategy"?}` | lento (§6), scrive (`writes`), entra nel diario (§7). Fetch del solo branch dell'upstream con `GitBackend::fetch` (avanzamento e annullamento come `remote.fetch`; annullato durante il fetch → `Cancelled { command: "remote.pull" }` anche se il fetch è finito, prima di integrare), poi l'integrazione in locale con `GitBackend::pull`, che non si interrompe. **Scelta: non `git pull`**, ma fetch + merge/rebase guidati dal motore: così il comando sa cosa è arrivato prima di scrivere, sceglie la strategia, controlla lo stato e salva il backup. `branch`: nome breve del branch locale (HEAD se manca; HEAD staccata senza `branch` → `NotAllowed`); un branch che non è quello di HEAD si può solo portare avanti (fast-forward del ref con `update-ref` e il valore vecchio, senza toccare il worktree; divergente → `NotAllowed` che chiede di passarci; aperto in un altro worktree → `NotAllowed`). `remote`: se c'è dev'essere quello dell'upstream (`NotAllowed` altrimenti): il pull integra sempre l'upstream. Upstream da `branch.<nome>.remote`/`.merge` (`GitBackend::upstream`); senza → `NotAllowed` che dice come impostarlo (`git branch --set-upstream-to=<remoto>/<branch>`) o di pubblicare il branch con `remote.push`; upstream locale (`.`), `merge` che non è un branch, nessun refspec che lo mappa in `refs/remotes/` → `NotAllowed`, niente scaricato. **Strategia** (`PullSettings::policy`, letta prima del fetch): `strategy` (`merge`, `rebase`, `ff_only`) sostituisce la configurazione; altrimenti `branch.<nome>.rebase`, poi `pull.rebase` (`true` → rebase, `merges` → `--rebase-merges`, `false` → merge, `interactive` → `NotAllowed`) e `pull.ff` (`only` → solo fast-forward, `false` → sempre un commit di merge); **senza configurazione merge** (scelta: git ≥ 2.33 rifiuterebbe i branch divergenti; un merge non riscrive niente e si annulla). Poi (`pull_mode`, da `ahead`/`behind` con `unmerged_count`): niente da fare se il branch contiene l'upstream (`up_to_date`, niente nel diario), fast-forward se può, altrimenti merge o rebase (`ff_only` divergente → `NotAllowed`). **Controlli** (`check_pull_state`, prima del fetch per HEAD e di nuovo nel backend): operazione in corso o file in conflitto → `NotAllowed`; merge e rebase con qualunque modifica ai file tracciati → `NotAllowed` (mai `--autostash`; i non tracciati vanno bene); il fast-forward accetta modifiche che non toccano i file cambiati, e se git rifiuta perché sovrascriverebbe modifiche o non tracciati → `NotAllowed` con i file. **Backup**: prima di scrivere `refs/project/backup/pull-...` sul branch com'era (tolto se poi niente cambia). **Conflitti** (scelta): il merge o il rebase fermo in conflitto si **annulla subito** (`--abort`: il worktree era pulito, quindi torna identico) e l'errore è `PullConflict { branch, upstream, operation: merge \| rebase, files, message }`; lasciare il repo nel merge/rebase in corso non avrebbe uscita dal client finché non ci sono `merge.abort`/`merge.continue` (S29), l'editor dei conflitti (S30) e `rebase.abort`/`rebase.continue` (S32); allora si potrà aggiungere un'opzione per tenerlo. Errori del fetch: `Auth`, `RemoteUnreachable`, `Cancelled`. Risultato: `{"branch", "remote", "upstream", "strategy": "fast_forward" \| "merge" \| "rebase" \| "up_to_date", "strategy_source": "param" \| "config" \| "default", "before", "after", "ahead", "behind", "backup": string \| null, "fetch": {"updated", "messages"}, "warnings", "status" \| null, "refs" \| null, "can_undo", "can_redo", "next_undo", "next_redo"}`. Dettagli del `CliBackend` in §5, undo in §7 |
| `remote.push` | `{"remote"?, "branch"?, "force"?, "confirm"?, "expected"?}` | lento (§6), scrive (`writes`), **non** entra nel diario (un push non si annulla: il remoto è di altri; il risultato dice cosa è cambiato). Pubblica solo `refs/heads/<branch>:refs/heads/<remote_branch>` (refspec esplicito: mai tag, `--all`, `--mirror`, `--tags`, qualunque sia `push.default`, `remote.<nome>.push`, `push.followTags`). `branch`: HEAD se manca (staccata → `NotAllowed`). **Remoto** come lo sceglie `git push` (risultato `remote_source`): `remote` (`param`), altrimenti `branch.<nome>.pushRemote` (`push_remote`), `remote.pushDefault` (`push_default`), il remoto dell'upstream (`upstream`), `origin` (`origin`) o l'unico remoto (`only`); più remoti senza `origin` → `NotAllowed`; un remoto della configurazione che non è configurato → `NotAllowed` che nomina la chiave; un remoto con `remote.<r>.mirror=true` → `NotAllowed` (un mirror pubblica tutti i ref insieme). **Destinazione**: se il remoto è quello dell'upstream, il branch dell'upstream (con un nome diverso dal branch → `NotAllowed`, come `push.default=simple`) e il branch remoto locale dell'upstream; altrimenti lo stesso nome, e il branch remoto locale si ricava dai refspec di fetch del remoto (`tracking_ref` su `remote.<r>.fetch`, anche con refspec non standard e negativi; nessuno → nessun branch remoto locale). **Primo push** (nessun upstream): `--set-upstream` verso il remoto scelto (l'upstream si imposta solo se il push riesce); con un upstream l'upstream non cambia mai (es. pubblicando su `fork` per `remote.pushDefault`). **Force**: solo con `"force": true`, sempre e solo `--force-with-lease=refs/heads/<b>:<oid>` con l'oid del branch remoto locale letto prima (vuoto se non c'è: il branch non deve esistere), mai `--force` né `+refspec`; se il remoto perderebbe commit che il branch non ha (`unmerged_count` del branch remoto locale) serve `"confirm": true` **con `"expected"`**, l'id completo (40 cifre) del branch remoto locale mostrato nella conferma: senza, `ConfirmRequired` con i commit (`missing_commits`, al più 10), l'id letto ora e la frase fissa `"expected": "<id>"` da ripetere. La conferma vale solo per quel commit: se `expected` non è il branch remoto locale letto ora (un fetch nel frattempo) → di nuovo `ConfirmRequired` con l'id nuovo, niente pubblicato; il lease usa `expected`. Prima del push il branch remoto locale si salva in `refs/project/backup/push-...` (tolto se il push è rifiutato). `confirm` senza `expected`, `confirm`/`expected` senza `force`, `expected` non completo → `InvalidParams`. `--force-if-includes` non serve: con un oid esplicito git lo ignora. Errori: `PushRejected { remote, branch, reason: non_fast_forward \| stale_lease \| remote_rejected \| hook \| other, message }` (non fast-forward suggerisce `remote.pull`; lease: il remoto si è spostato, fare fetch), niente cambia in locale; `Auth`, `RemoteUnreachable`, `Cancelled` (annullato a metà: il remoto potrebbe aver già aggiornato il branch, rileggere con un fetch). Risultato: `{"remote", "remote_source": "param" \| "push_remote" \| "push_default" \| "upstream" \| "origin" \| "only", "branch", "remote_branch", "kind": "new" \| "fast_forward" \| "forced" \| "up_to_date", "summary", "before": oid \| null, "after", "set_upstream", "forced", "lost": [Commit], "backup": string \| null, "updated" \| null, "messages", "warnings", "status" \| null, "refs" \| null}` |
| `remote.list` | `{}` | sola lettura. Risultato: `{"remotes": [{"name", "url": string \| null, "push_urls": [string], "fetch": [string], "mirror": bool, "branches": n, "default": bool, "push_default": bool, "credentials": bool}], "default": string \| null, "push_default": string \| null}`, in ordine di nome (`git remote`). `url`: il primo `remote.<r>.url` come scritto (senza `insteadOf`); `push_urls`: i `pushurl`, o tutti gli `url` se più di uno; vuoto = il push va a `url`. `branches`: i branch remoti `refs/remotes/<r>/...` (esclusi quelli di un remoto `<r>/...`, senza i simbolici); `default`: il remoto di `remote.fetch` senza parametri (`default_remote`; `null` se ambiguo); `push_default`: l'ultimo `remote.pushDefault`. **Credenziali mascherate** (`mask_url`, ovunque un URL esca dal motore: `remote.list`, `remote.add`, `remote.remove`, la voce del diario in `edit.history`, i messaggi d'errore sugli URL): `schema://utente:password@host` → `utente:***`; in `http(s)`/`ftp(s)` anche un utente senza password diventa `***` (spesso è un token); un utente in `ssh://`/`git://` o alla scp (`git@host:r`) resta; vale dentro un helper (`<x>::<url>`). `credentials`: l'URL ne aveva. Ahead/behind dei branch rispetto all'upstream non sono qui: sono in `RefInfo::upstream` (`repo.refs`, da `%(upstream:track)`), riletti nei `refs` di `remote.fetch`, `remote.pull` e `remote.push` |
| `remote.add` | `{"name", "url", "fetch"?}` | scrive (`writes`), entra nel diario (§7). Nome nuovo (`check_new_remote_name`: `check_remote_name` e niente `-` iniziale) → `InvalidParams`; nome già configurato (`git remote`), voci `remote.<name>.*` nel file del repo o branch remoti `refs/remotes/<name>/...` rimasti da prima (l'undo li cancellerebbe) → `NotAllowed`. **Politica degli URL** (`check_remote_url`, prima di git): non vuoto, senza caratteri di controllo né spazi in testa o in coda, non inizia con `-` (`--upload-pack=...`, `-oProxyCommand=...`), niente trasporti `ext::`/`ext://`/`fd::`/`fd://` (eseguono comandi o usano descrittori; anche dentro un altro helper e in qualsiasi maiuscolo: da qui mai, qualunque sia `protocol.allow`), niente utente o host che inizia con `-` (`ssh://-x/r`, `ssh://-oX@h/r`, `ssh://[-x]/r`, `u@-x:r`); gli altri helper `<x>::` e i percorsi locali vanno bene. Poi `git remote add --end-of-options <name> <url>` (URL e refspec di default; un nome che git rifiuta → `InvalidParams`, uno che esiste → `NotAllowed`). Un URL con credenziali si accetta con un avviso in `warnings` (git lo salva in chiaro). `"fetch": true`: poi un fetch di tutto il remoto come `remote.fetch` (lento, §6: avanzamento e annullamento dal `JobContext`); un fetch fallito o annullato **non** annulla l'aggiunta: il remoto resta (ed è nel diario) e l'errore va in `fetch_error` (`kind`: `auth`, `unreachable`, `cancelled`, `other`). Risultato: `{"remote", "url" (mascherato), "credentials", "fetched", "fetch": {"updated", "messages", "warnings"} \| null, "fetch_error": {"kind", "message"} \| null, "warnings", "remotes": [come remote.list], "refs", ...diario}` |
| `remote.remove` | `{"name", "confirm"?}` | scrive (`writes`), entra nel diario (§7). `name`: un remoto configurato (`check_remote_name`, `-` iniziale ammesso; altrimenti `NotAllowed` con l'elenco). Rifiutati (`NotAllowed`, niente cambia): un remoto definito **anche solo in parte** fuori dal file del repo (`GitBackend::remote_config_elsewhere`: configurazione globale o di sistema, `include`, `config.worktree`, `.git/remotes/`; l'errore elenca chiave, ambito e file), perché ne resterebbe una parte che git vede ancora; il remoto promisor di un clone parziale (`remote.<name>.promisor` vero o `extensions.partialClone` = `<name>`: senza, gli oggetti mancanti non si scaricherebbero più e checkout, log e diff fallirebbero); un'operazione in corso. Se dopo la rimozione `git remote` elenca ancora il remoto (`ensure_remote_gone`), tutto si rimette subito com'era e il risultato è `NotAllowed`; se il ripristino riesce solo in parte (es. un branch remoto ricreato nel frattempo: la configurazione si riscrive comunque, con un avviso) o l'elenco dei remoti non si rilegge, la rimozione entra comunque nel diario, così `edit.undo` e i backup restano disponibili (l'errore lo dice). Chiede **sempre** `"confirm": true`: senza, `ConfirmRequired` che dice quanti branch remoti spariscono e quali branch perdono l'upstream o il `pushRemote`, e se `remote.pushDefault` lo nomina; niente cambia. Toglie (`remove_remote`, §7) i branch remoti `refs/remotes/<name>/...` (non quelli di un remoto `<name>/...`; a differenza di `git remote remove`, i ref scritti fuori da `refs/remotes/<name>/` da un refspec personalizzato restano), `refs/remotes/<name>/HEAD`, la sezione `remote.<name>`, `branch.<b>.remote`/`merge` dei branch con quell'upstream, i `branch.<b>.pushRemote` e `remote.pushDefault` che lo nominano (tutto del file del repo); i tag scaricati restano, il remoto vero non cambia. Risultato: `{"remote", "removed": RemoteSnapshot (URL mascherati), "upstreams_removed": [branch], "push_remotes_removed": [branch], "push_default_removed": bool, "backups": [string], "warnings", "remotes", "refs", ...diario}` |
| `stash.list` | `{}` | sola lettura. Risultato: `{"stashes": [{"index": n, "name": "stash@{n}", "id", "base", "message", "branch": string \| null, "time", "untracked": bool}]}`, dal più recente, dal reflog di `refs/stash` (vuoto se non c'è). `base`: il primo genitore, il commit di HEAD quando lo stash è stato creato (la UI disegna lo stash accanto a questo commit; con `repo.log {"all": true, "stashes": true}` è sempre nel grafo); `message`: il messaggio del reflog come in `git stash list` (`On <branch>: <msg>` o `WIP on <branch>: <id> <oggetto>`); `branch` letto da lì (`null` con HEAD staccata o un altro formato); `time`: data del commit dello stash; `untracked`: ha il terzo genitore con i non tracciati. I file tracciati di uno stash si leggono con `commit.show`/`diff.commit` sull'`id` (confronto con `base`: il worktree salvato, in stage o no); i non tracciati non ci sono. `refs/stash` resta anche in `repo.refs` (`kind: "Stash"`, lo stash più recente) |
| `stash.push` | `{"message"?, "include_untracked"?, "keep_index"?, "paths"?}` | scrive (`writes`), entra nel diario (§7). `git stash push` (§5): salva le modifiche ai file tracciati (in stage e no) e, con `include_untracked`, i non tracciati (mai gli ignorati: niente `--all`), poi riporta indice e worktree come HEAD (con `keep_index` l'indice resta e il worktree diventa come l'indice). `paths`: lista non vuota di percorsi alla lettera, come `index.stage` (**git mette comunque nello stash tutto l'indice**: le modifiche in stage degli altri file restano nel worktree ma sono anche nello stash, e applicarlo le riporta). `message`: una riga, senza caratteri di controllo (`InvalidParams`); vuoto = assente (git scrive `WIP on ...`). Rifiutati prima di git (`check_stash_push`, `NotAllowed`, niente cambia): operazione in corso, file in conflitto, branch senza commit, file `intent-to-add` (git non riesce a salvarli), file `--assume-unchanged` fra quelli scelti (solo `CliBackend`, §5: git non ne salverebbe le modifiche e poi le cancellerebbe con il reset del worktree), niente da mettere da parte fra i file scelti (anche se git non crea lo stash: `refs/stash` invariato). Risultato: `{"stash": StashEntry, "stashes": [StashEntry], "status", ...diario}` |
| `stash.apply` / `stash.pop` | `{"index"?, "expected"?, "restore_index"?}` | scrivono (`writes`), **non** entrano nel diario (§7). Lo stash è `stash@{index}` (0 se manca; non c'è → `NotAllowed` che dice quanti sono); `expected`: l'id **completo** dello stash visto (altrimenti `InvalidParams`), se l'elenco è cambiato → `NotAllowed` che dice dov'è ora (consigliato: la UI lo passa sempre). Al backend arriva l'id. `restore_index`: rimette anche la divisione fra in stage e no (`--index`); senza, tutto torna fuori stage tranne i file nuovi, che git lascia in stage. **Mai sopra modifiche locali** (controlli prima di git, `NotAllowed` e niente cambia): operazione in corso, file in conflitto, una modifica (in stage o no) a un file che lo stash cambia (rispetto alla base, nel worktree o nell'indice salvati), un non tracciato o ignorato al posto di un file che lo stash aggiunge o di un non tracciato che rimette (anche al posto di una sua cartella), file `--assume-unchanged` che lo stash cambia. **Eccezione**: un file senza modifiche fuori stage la cui voce dell'indice è la stessa nell'indice e nel worktree salvati dallo stash (`<id>^2` e `<id>`; modo 0: assente da tutti e due) ha già la versione dello stash e non blocca: è il caso dei file rimasti in stage dopo un push con `keep_index`, o con `paths` (git salva comunque tutto l'indice); senza, l'undo di quei push e il pop dopo un `keep_index` sarebbero sempre rifiutati. Un file `MM` rimasto con `keep_index` resta bloccato (il worktree dello stash è diverso: git andrebbe in conflitto). Servono perché `git stash apply` non controlla tutto prima di scrivere: con un non tracciato dello stash già presente rimette i file tracciati e poi fallisce, e rifiutando per una modifica locale ha già rimesso i non tracciati. `restore_index` che non si applica sopra l'indice attuale (HEAD cambiato dalla base) → `NotAllowed` che suggerisce di riprovare senza. **Conflitti** (scelta): se HEAD ha cambiato gli stessi file dalla base il merge si ferma e i file restano in conflitto come li lascia git (`conflicts`; nessuna operazione in corso, solo voci `U` nell'indice); niente si perde, perché quei file non avevano modifiche locali (controllo sopra) e lo stash resta nell'elenco: `pop` non lo elimina (`dropped: false`, con un avviso). Si risolvono con `index.stage` del file (poi l'editor dei conflitti, S30); finché ci sono, apply e push sono rifiutati. `pop` senza conflitti: drop come `stash.drop` (backup `refs/project/backup/stash-...` prima). Un drop che fallisce **dopo** l'apply non è un errore del pop (`StashPopped::drop_error`): l'apply è fatto e non va ripetuto, il risultato ha `dropped: false` e un avviso "applicato ma non tolto dall'elenco". Risultato: `{"stash": StashEntry, "conflicts": [percorso], "dropped": bool, "backup": string \| null, "warnings", "stashes", "status"}` |
| `stash.drop` | `{"index"?, "expected", "confirm"}` | scrive (`writes`), entra nel diario (§7). Stash scelto come `stash.apply`. Chiede **sempre** `"confirm": true` **con `"expected"`**, l'id completo dello stash: senza `confirm`, `ConfirmRequired` con posizione, id, messaggio, numero di file tracciati e se ha non tracciati, e l'id in `expected` (come per il force di `remote.push`); `confirm` senza `expected` → `InvalidParams`; niente cambia. La conferma vale solo per lo stash visto (`expected` diverso da `stash@{index}` → `NotAllowed`). Prima un backup del commit in `refs/project/backup/stash-<s>-<ns>` (se non riesce non si elimina niente). Risultato: `{"stash": StashEntry, "backup": string, "warnings", "stashes", ...diario}` |
| `edit.undo` / `edit.redo` | `{}` | sul diario delle operazioni (§7). Scritture. `undo` annulla l'ultima voce fatta, `redo` rifà l'ultima annullata; senza voci `NotAllowed`. I ref devono valere ancora quello che l'operazione (o l'undo) ha lasciato, altrimenti `UndoConflict { command, refs }` e niente cambia (la voce resta). Undo e redo di `branch.checkout` sono checkout sicuri: con modifiche locali in conflitto `CheckoutConflict` (la voce resta). Merge, rebase, cherry-pick, revert, bisect o `git am` in corso (anche un sequencer fermo, `.git/sequencer` senza `CHERRY_PICK_HEAD`/`REVERT_HEAD`): `NotAllowed`. Risultato: `{"undone" \| "redone": JournalEntry, "backups": [string], "warnings": [string], "status": Status \| null, "can_undo": bool, "can_redo": bool, "next_undo": string \| null, "next_redo": string \| null}` (`warnings`: avvisi scoperti dopo uno spostamento riuscito, es. HEAD cambiata da un altro processo in quell'istante; di solito vuoto. `status` `null`, con `status_error`, se non si rilegge: l'operazione è fatta, non va ripetuta) |
| `edit.history` | `{}` | sola lettura: `{"done": [JournalEntry], "undone": [JournalEntry], "can_undo", "can_redo", "next_undo", "next_redo"}`; in tutte e due le liste l'ultima voce è la prossima per `undo` / `redo` |

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
  - un hook che lascia processi in background con stdout o stderr aperti fa aspettare il
    comando finché quei processi non li chiudono (git e il backend leggono fino alla fine);
  - una firma ssh con chiave protetta da passphrase (senza agent) e un hook che legge
    `/dev/tty` possono aspettare sul terminale di controllo, se c'è (CLI o server MCP avviati
    da un terminale); l'app desktop di solito non ne ha. Nessun timeout, per scelta (vedi
    sopra);
  - ambiente: oltre alle variabili di §5 si tolgono `GIT_AUTHOR_DATE` e `GIT_COMMITTER_DATE`
    ereditate (un'app avviata da un hook darebbe a ogni commit la data di allora); nome ed
    email (`GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_*`, `EMAIL`) restano, come per
    git da terminale: sono l'identità scelta dall'utente;
  - dopo il commit il backend rilegge HEAD e controlla che l'ultima voce del reflog del ref
    aggiornato (il branch, o HEAD se staccata) porti da `previous` al nuovo commit
    (`<ref>@{1}` e `<ref>@{0}`): così si scopre anche un altro processo che ha fatto `amend`
    fra la lettura di `previous` e `git commit` (git avrebbe riscritto quell'altro commit, non
    quello salvato nel backup). Controlla poi i genitori (con `amend` gli stessi del commit
    riscritto). Se qualcosa non torna: `PostCommit` (il commit resta). Limite: senza reflog
    (`core.logAllRefUpdates=false` e nessun file di log per il ref) resta solo il controllo
    sui genitori. Dentro `git commit` il ref si aggiorna con il valore vecchio letto da git,
    quindi uno spostamento durante gli hook fa fallire il commit.
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
- **Fetch** (`remote.fetch`, S25): `git fetch --progress --end-of-options <remote>
  [refs/heads/<branch>]`. Il remoto (sempre un remoto configurato, mai un URL) e il branch
  vanno dopo `--end-of-options`: un remoto che si chiama `-x` o `--upload-pack=false` non
  diventa un'opzione (test con questi nomi). Il branch è un refspec senza destinazione, senza
  `:` né `+`: non scrive ref locali, e il branch remoto si aggiorna con il refspec configurato.
  Niente `--prune`, `--force`, `--tags`, `--update-head-ok`, `--recurse-submodules`: tag,
  pulizia dei branch remoti spariti (`fetch.prune`, `remote.<nome>.prune`) e sottomoduli come
  dice la configurazione dell'utente, come `git fetch` da terminale. **Nessuna richiesta di
  input**, mai: stdin chiuso, `GIT_TERMINAL_PROMPT=0` e, su Unix, il processo parte in una
  sessione nuova (`setsid`, dipendenza `libc`) senza terminale di controllo, così né git, né
  ssh (passphrase, conferma della chiave dell'host), né un credential helper possono aprire
  `/dev/tty` e aspettare, anche se CLI o server MCP sono avviati da un terminale. Restano le
  richieste grafiche (`SSH_ASKPASS` con un display, `GIT_ASKPASS`, `core.askPass`) e i
  credential helper che non chiedono niente (cache, keychain, libsecret, GCM); senza, una
  credenziale mancante fallisce subito con `Auth`. stderr si legge mentre git gira
  (`Git::run_streaming`): diviso a ogni `\r` e `\n`, le righe di avanzamento vanno al
  `JobContext` (§6), le altre, solo quelle chiuse da `\n`, sono i `messages` del risultato o il
  testo dell'errore. Errori riconosciuti dal testo (`LC_ALL=C`, `progress::remote_error`):
  `Auth` per "Authentication failed", "could not read Username/Password", "terminal prompts
  disabled", "Permission denied (publickey...)", "Host key verification failed", HTTP 401/403;
  `NotAllowed` per "couldn't find remote ref"; `RemoteUnreachable` per "does not appear to be a
  git repository", "Could not resolve host", "Connection refused/timed out", "repository not
  found", HTTP 404, "unable to access", "Could not read from remote repository" (guardati dopo
  quelli di `Auth`, che li precedono nello stesso stderr). Dopo l'uscita di git il backend
  aspetta al massimo un secondo che stderr si chiuda: un processo rimasto in background (un
  master ssh con `ControlPersist`) non blocca il comando. Annullamento: vedi §6. Limiti: su
  Windows niente `setsid` (non c'è `/dev/tty`; l'annullamento usa `kill`); la configurazione
  del repo può dare remoti con URL fuori dalla radice del server MCP (il fetch legge da lì, come
  da terminale).
- **Pull** (`remote.pull`, S26): dopo il fetch di `refs/heads/<merge>` (come sopra), il backup
  e poi, con `GIT_REFLOG_ACTION=project: remote.pull`, `GIT_EDITOR=:` e `GIT_SEQUENCE_EDITOR=:`:
  fast-forward del branch attivo `git -c core.fsmonitor=false merge --ff-only --no-autostash
  --no-overwrite-ignore --no-edit --end-of-options <oid>` (git rifiuta prima di scrivere se sovrascriverebbe modifiche
  o non tracciati); di un altro branch `update-ref -m ... refs/heads/<b> <nuovo> <vecchio>`;
  merge `git -c core.fsmonitor=false merge --no-autostash --no-overwrite-ignore --no-edit --commit --no-squash
  --ff|--no-ff -F <file> --end-of-options <oid>` (il messaggio, `Merge remote-tracking branch 'origin/main'` più ` into
  <b>` fuori da `main`/`master`, da un file temporaneo nella cartella git: `git merge` non legge
  `-F -` da stdin, e il suo percorso va a git come `OsStr`; `--ff`/`--no-ff`, `--commit`, `--no-squash` espliciti contro `merge.ff` e
  `branch.<nome>.mergeOptions`); rebase `git -c
  core.fsmonitor=false -c rebase.updateRefs=false rebase --no-autostash --no-autosquash [--rebase-merges]
  --end-of-options <oid>` (`rebase.updateRefs` sposterebbe altri branch fuori dal diario; senza il
  fork-point di `git pull --rebase`: con un upstream riscritto il rebase di solito si ferma in
  conflitto, e si annulla). **File ignorati**: git sovrascrive senza dirlo un file ignorato che
  l'upstream aggiunge (e un merge annullato lo cancellerebbe); il rebase non ha
  `--no-overwrite-ignore`. Per questo, per il branch attivo e prima del backup, i file che
  l'upstream porta e HEAD non ha (`diff-tree -r -z --name-only --no-renames --diff-filter=A HEAD
  <upstream>`) non devono esistere nel worktree, né al posto di una loro cartella
  (`symlink_metadata`): altrimenti `NotAllowed` con i file e niente cambia (per fast-forward e
  merge è una seconda difesa dopo `--no-overwrite-ignore`). Per il rebase (anche
  `--rebase-merges`) contano anche i file toccati da ognuno dei commit da riapplicare e che
  HEAD non ha (`git log -m --no-renames --format= -z --name-only <upstream>..HEAD`): il rebase
  passa per gli alberi intermedi, quindi un file che un commit locale aggiunge e uno successivo
  toglie (o rinomina, o toglie e ricrea), o che un commit locale modifica mentre l'upstream lo
  toglie (conflitto modify/delete, poi `rebase --abort`), verrebbe scritto e poi cancellato
  insieme all'ignorato che c'era al suo posto. Il merge passa solo da HEAD al
  risultato e non ne ha bisogno. Il `MemoryBackend` fa lo stesso controllo sugli alberi dei
  commit da riapplicare. Hook (`pre-merge-commit`, `commit-msg`, `post-merge`,
  `post-rewrite`) e firma seguono la configurazione. Fermo (conflitto o altro): file in conflitto
  dallo stato e quelli che git dice in conflitto (`Merge conflict in <file>`, e con
  `rerere.autoUpdate` `Resolved '<file>' using previous resolution`: anche un conflitto risolto
  da rerere lascia il merge/rebase fermo ed è `PullConflict`), `git merge --abort` / `git rebase
  --abort`, e se l'annullamento non riesce un
  errore che lo dice. Dopo: il branch deve contenere l'upstream, HEAD sul branch, altrimenti
  `warnings`. File `--assume-unchanged` diversi fra i due commit → `NotAllowed` (git li
  sovrascriverebbe senza controllarli).
- **Push** (`remote.push`, S26): `git push --porcelain --progress --no-follow-tags
  --recurse-submodules=check [--set-upstream] [--force-with-lease=refs/heads/<dst>:<oid>]
  --end-of-options <remote> refs/heads/<b>:refs/heads/<dst>`, con l'ambiente del fetch (nessun
  terminale, stdin chiuso, `setsid`). `--recurse-submodules=check` al posto di un eventuale
  `push.recurseSubmodules=on-demand`, che pubblicherebbe anche i sottomoduli (e rifiuta un push
  che riferisce commit di sottomoduli non pubblicati). Mai `--no-verify`: gli hook `pre-push`
  girano. stderr come il fetch (avanzamento al `JobContext`, il resto in `messages` o
  nell'errore); l'esito dallo stdout di `--porcelain` (`Git::run_streaming` con
  `capture_stdout`): la riga `<flag>\t<da>:<a>\t<riassunto>` del ref (` ` fast-forward, `+`
  forzato, `*` nuovo, `=` già aggiornato, `!` rifiutato: `non-fast-forward`/`fetch first`,
  `stale info`, `remote rejected`). Uscita non zero senza quella riga: `Auth`/`RemoteUnreachable`
  dal testo come il fetch, altrimenti con "failed to push some refs" un hook `pre-push` (o il
  controllo dei sottomoduli): `PushRejected` con l'output dell'hook.
- **Remoti** (`remote.list`, `remote.add`, `remote.remove`, S27). Lettura di un remoto
  (`remote_snapshot`): un solo `git config --local -z --get-regexp
  '^(remote\.<nome>\.[^.]+|remote\.pushdefault|branch\..+\.(remote|merge|pushremote))$'`
  (solo il file del repo, niente `include` né globale; un valore non UTF-8 → `NotAllowed`), `git
  for-each-ref --format=%(refname)%00%(objectname)%00%(symref) refs/remotes/<nome>/` e `git
  symbolic-ref -q --end-of-options refs/remotes/<nome>/HEAD` (un simbolico che punta a un ref
  che non c'è non compare in `for-each-ref`). `remote.list` legge con `config_values` (tutte le
  fonti, come `git remote`). Le voci fuori dal file del repo (`remote_config_elsewhere`): `git
  config -z --show-scope --show-origin --includes --get-regexp '^remote\.<nome>\.[^.]+$'` meno
  le provenienze di `git config -z --local --show-origin` (un file incluso ha ambito `local` ma
  un'altra provenienza), più i file del formato vecchio `remotes/<nome>` e `branches/<nome>` (`git rev-parse --git-path`), che git legge come remoti. L'URL nello stderr di `git remote add` si sostituisce con quello
  mascherato prima di farne l'errore. Fetch e push girano con `-c protocol.ext.allow=never -c
  protocol.fd.allow=never`: un remoto `ext::`/`fd::` configurato a mano non esegue niente.
  Aggiunta: `git remote add --end-of-options <nome> <url>`, mai
  `-f`/`--mirror`/`-t`. **Rimozione senza `git remote remove`**: i branch remoti con la
  transazione di `restore_refs` (valori attesi, backup), poi esattamente le voci salvate: `git
  config --local --remove-section --end-of-options remote.<nome>`, `git config --local
  --fixed-value --unset-all --end-of-options <chiave> <valore>` per upstream, `pushRemote` e
  `remote.pushdefault` (codice 5, niente da togliere, va bene), `git symbolic-ref --delete
  --end-of-options refs/remotes/<nome>/HEAD`. Così niente sparisce senza essere nel diario
  (`git remote remove` sceglierebbe da sé i ref da togliere dai refspec, e toglierebbe
  `remote.pushDefault` anche da altri file). Ripristino: `git config --local --add
  --end-of-options <chiave> <valore>` per ogni voce e `git symbolic-ref -m <reflog>
  refs/remotes/<nome>/HEAD <ref>`.
- **Stash** (`stash.*`, S28). Elenco: `git log --walk-reflogs -z --no-show-signature
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
legge da stderr con `--progress`. `remote.pull` e `remote.push` (S26) usano lo stesso
`JobContext` di `remote.fetch`: il pull lo passa al fetch (l'unica parte annullabile; un
annullamento visto dopo il fetch ferma il comando prima di integrare, mai a metà di un merge o
di un rebase), il push lo usa per tutto il `git push` (annullato a metà, il remoto potrebbe aver
già aggiornato il branch).

### Avanzamento e annullamento (S25)

`Registry::run` resta sincrono e senza callback. Per i comandi lenti c'è
`Registry::run_with(id, session, params, job: JobContext)`: per la durata del comando la
sessione tiene `job` (`Session::job()`), il comando lo passa al backend, e dopo torna un
contesto vuoto (`Registry::run` è `run_with` con `JobContext::new()`: nessun avanzamento, mai
annullato). Così l'estensione non cambia la firma dei comandi né dei `Handler`.

- `JobContext::new().with_progress(sink).with_cancel(token)`: `sink` è un
  `Fn(&Progress) + Send + Sync`, chiamato dal thread del job; `token` è un `CancelToken`
  (`Arc<AtomicBool>` condiviso: `cancel()` da qualunque thread, `is_cancelled()`).
- `Progress` (serializzabile): `{"phase", "remote", "percent", "current", "total", "done",
  "text"}` da una riga di git (`Receiving objects:  45% (9/20), 1.2 MiB | 2 MiB/s` → fase
  `Receiving objects`, 45, 9, 20; `remote: Enumerating objects: 5, done.` → `remote: true`,
  `current` 5, `done`). Le fasi si susseguono (`Enumerating`, `Counting`, `Compressing` sul
  remoto, poi `Receiving`/`Unpacking objects` e `Resolving deltas` in locale); la UI mostra
  l'ultima ricevuta. Git ne scrive al più qualche decina al secondo: il sink deve solo salvarla
  (es. in un `Mutex`) e chiedere un frame, non disegnare.
- **La UI** (`Executor`): per `remote.fetch` chiama `run_with` nel job di `Platform::spawn` con
  un sink che salva l'ultimo `Progress` del ticket e chiama `wake`, e tiene il `CancelToken`
  del ticket per un pulsante "Annulla". La sessione resta bloccata dal job per tutto il fetch
  (§6): le letture mandate nel frattempo aspettano la fine. Sul web il `MemoryBackend` finisce
  subito (eventi finti con la stessa forma).
- **Annullamento**: il `CliBackend` controlla il token ogni 50 ms mentre git gira; se è
  annullato manda SIGTERM al gruppo di processi del fetch (git, ssh, `git-remote-http`,
  `index-pack`: git toglie i propri `.lock`), dopo 3 secondi SIGKILL, e il comando dà
  `EngineError::Cancelled`. I ref già aggiornati da git restano (git li aggiorna alla fine, ogni
  ref sotto il suo lock), gli oggetti già ricevuti restano nel database (li toglie un `gc`):
  nessun ref locale, indice o worktree è toccato. Un token annullato prima dell'inizio dà
  `Cancelled` senza avviare git.
- CLI: con stderr su un terminale stampa l'avanzamento su stderr (una riga riscritta con `\r`);
  stdout resta il JSON. Server MCP: usa `Registry::run`, niente avanzamento.

## 7. Undo e backup

Il diario delle operazioni registra, per ogni comando che sposta ref, i valori prima e dopo:
`HEAD`, il branch coinvolto, l'oid. `edit.undo` riporta i ref ai valori precedenti, solo se nel
frattempo non sono cambiati (altrimenti errore chiaro, nessuna sovrascrittura).

### Diario delle operazioni (S22)

Ogni voce (`JournalEntry`, JSON `{"id", "command", "description", "head": {"before",
"after"}, "refs": [{"name", "before", "after"}], "backup", "undo_backups", "worktree",
"branch_config", "remote", "stash"}`) ha l'id del
comando, una descrizione per le persone (`Commit a0744b4 «oggetto»`, `Amend b12c3d4 «oggetto»
(era a0744b4)`), HEAD prima e dopo (`HeadState`: `{"branch": "refs/heads/main"}` o
`{"detached": oid}`), i ref spostati con il nome completo e l'oid prima e dopo (`null` = il ref
non esisteva / è stato cancellato: più ref per voce, per branch, rinomine, merge, rebase e reset
futuri), il backup salvato dall'operazione (es. quello di `amend`) e i backup creati da undo e
redo. Oggi la registrano `commit.create` (normale e `amend`; con HEAD staccata nessun ref, si
sposta HEAD; un `amend` che produce lo stesso commit e un `PostCommit` non entrano), i comandi
`branch.*` (vedi "Branch (S24)" sotto), `stash.push` e `stash.drop` (vedi "Stash (S28)") e `worktree.discard` (nessun ref, HEAD uguale prima e dopo, il backup del worktree nel campo
`"worktree"`, vedi sotto). Stage e unstage non spostano ref e non entrano nel diario: il loro recupero è il
backup dell'indice. Una nuova voce svuota le voci annullate (il redo); il diario tiene al
massimo 100 voci.

Scelte:

- **In memoria, per repo aperto.** Non si salva su disco: le CLI eseguono una sessione per
  invocazione, quindi più `--cmd` della stessa invocazione condividono il diario e invocazioni
  diverse no; il server MCP e l'app lo tengono finché il repo resta aperto. Persisterlo (es. in
  `.git/project/journal`) vorrebbe dire gestire più istanze sullo stesso repo e formati di
  versioni diverse; per non perdere dati non serve, perché i commit annullati restano in ref di
  backup e ogni spostamento è nel reflog (`project: undo ...`). Si può aggiungere dopo senza
  cambiare i comandi: il controllo sui valori attesi rende sicura anche una voce vecchia.
- **Undo di un commit = `git reset --soft`.** Si spostano solo i ref: indice e worktree non si
  toccano, quindi le modifiche del commit annullato ricompaiono in stage rispetto al nuovo HEAD
  e `edit.redo` riporta il branch allo stesso commit. Con `amend`, l'undo riporta il branch al
  commit riscritto (che esiste: c'è il backup di `amend`) e quello che `amend` aveva aggiunto
  resta in stage; nessun controllo sull'indice serve, perché niente lo scrive. L'undo del primo
  commit cancella il branch (`delete` con il valore vecchio): HEAD resta sul branch, che torna
  senza commit, e i file restano in stage come aggiunti. Cancellare il branch ne cancella anche
  il reflog (come `git update-ref -d`); resta la voce nel reflog di HEAD.
- **Controllo atomico.** `GitBackend::restore_refs` riceve una `RefRestore` (HEAD e ref attesi e
  nuovi). Prima: `RefRestore::check` (operazioni in corso, nomi di ref validi, cambi di branch
  di HEAD rifiutati: undo e redo di checkout e rinomina passano da `GitBackend::checkout` e
  `GitBackend::rename_branch`, vedi "Branch (S24)"), poi HEAD e ref attuali confrontati con quelli attesi
  (`RefRestore::conflicts`: se qualcosa è cambiato, `UndoConflict` con un ref per riga e niente
  cambia). Il `CliBackend` scrive con una transazione `git update-ref -m "project: undo <descrizione>"
  -z --stdin`: `start`, poi `update <ref> <nuovo> <vecchio>` / `create <ref> <nuovo>` /
  `delete <ref> <vecchio>` e, con HEAD staccata, `option no-deref` + `update HEAD <nuovo>
  <vecchio>`, infine `commit`. Git controlla i valori vecchi sotto il lock dei ref: un ref
  spostato fra il controllo e la transazione la fa fallire tutta, e l'errore torna
  `UndoConflict`. Con HEAD su un branch si aggiorna il branch (HEAD non si stacca) e git scrive
  la voce anche nel reflog di HEAD. Un hook `reference-transaction` che rifiuta dà l'errore di
  git e niente cambia.
- **Limiti su HEAD.** HEAD non si verifica dentro la transazione. Con HEAD su un branch, che
  punti ancora a quel branch si controlla prima: resta l'istante fra il controllo e la
  transazione. Con HEAD staccata `option no-deref` + `update HEAD` controlla solo il valore: un
  `git checkout main` fatto in quell'istante da un altro processo, con `main` sul commit
  atteso, verrebbe staccato di nuovo (`main` non si sposta, nessun commit è perso).
  `symref-verify` (git ≥ 2.46) non si usa: servirebbe solo al caso del branch, ma verificare
  HEAD nella stessa transazione che aggiorna il branch a cui punta urta il controllo di git
  sugli aggiornamenti multipli di HEAD (git aggiunge da sé una voce di reflog per HEAD), e la
  CI non ha quel git per provarlo. Al suo posto, dopo la transazione il `CliBackend` rilegge
  HEAD e conta le voci nuove del reflog di HEAD (al massimo una, la nostra): se HEAD non è
  quella chiesta o è cambiata nel frattempo, il risultato lo dice in `warnings`
  (`RestoreOutcome::warnings`). Senza reflog di HEAD resta solo il confronto con HEAD.
- **Backup dei commit annullati.** Prima di spostare i ref, ogni commit che verrebbe abbandonato
  (`RefRestore::abandoned`: i valori attesi che non sono fra i nuovi) e che non resta
  raggiungibile da un nuovo valore (`merge-base --is-ancestor`) né da un altro ref non
  simbolico, backup compresi (`for-each-ref --format=%(refname)%00%(symref) --contains`, un solo
  comando per tutti i commit; i ref simbolici si scartano come in `amend_impact`: un
  `refs/heads/master -> refs/heads/main` comparirebbe con il valore di `main` prima della
  transazione e dopo non terrebbe niente), si salva in `refs/project/backup/undo-<secondi>-<nanosecondi>`
  (`redo-...` per il redo; creato solo se non esiste, come gli altri backup). Così il gc non lo
  elimina e il redo resta possibile anche dopo un riavvio (a mano, dal ref). Se il backup non
  riesce non si sposta niente. Il risultato li elenca in `backups` e la voce in `undo_backups`.
  I controlli di raggiungibilità usano `--no-replace-objects`: conta la storia vera, quella che
  vede il gc. Le protezioni date da `refs/stash`, dai branch remoti e dai backup dell'indice
  (`refs/project/backup/index-*`, che hanno HEAD per genitore) sono **volatili**: uno stash si
  scarta, un remoto si aggiorna con un fetch, un backup dell'indice si può cancellare. Oggi non
  si crea un backup in più per loro; una futura pulizia di `refs/project/backup/*` non deve
  considerare indipendenti questi ref (prima di togliere un backup va controllato che il commit
  non resti tenuto solo da ref volatili o da altri backup che si stanno togliendo).
  Il `MemoryBackend` fa gli stessi controlli, registra i backup in `commit_backups`
  (`refs/project/backup/undo-<n>`) e ricalcola le voci dello stato rispetto al nuovo HEAD con lo
  stesso indice e worktree (le rinomine in stage tornano cancellazione e aggiunta).

### Scartare le modifiche (`worktree.discard`, S23)

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
ref si toglie (`update-ref -d <ref> <id>`, che lo toglie solo se punta ancora a quel commit):
non c'è niente da recuperare. Resta una finestra breve fra la rilettura di HEAD e
`update-ref -d` in cui un altro processo potrebbe spostare HEAD: il backup sparirebbe, ma il
commit resterebbe nel reflog. Se invece HEAD è cambiato (un hook o un altro processo) il
backup resta e l'errore lo nomina; lo nomina anche `PostCommit` dopo un commit riuscito. Un
`amend` che produce lo stesso commit toglie il backup (punterebbe al commit attuale). Un commit nuovo non
salva backup: il commit precedente è il suo genitore. Il `MemoryBackend` registra i backup in
`MemoryBackend::commit_backups`.

Per il diario (S22) `commit.create` restituisce già i valori prima e dopo del ref spostato:
`branch` (o `null` con HEAD staccata), `previous` e `commit.id`.

### Branch (S24)

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

### Stash (S28)

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
- `refs/stash` è una protezione **volatile** dei commit (vedi sopra "Backup dei commit
  annullati"): i backup `refs/project/backup/stash-...` e `undo-...` di drop e pop sono invece
  ref veri e propri, che una futura pulizia dei backup deve trattare come gli altri.

Il `MemoryBackend` registra i backup degli stash in `commit_backups`
(`refs/project/backup/stash-<n>`, `undo-<n>`, `redo-<n>`) e li risolve come ref.

L'undo non copre le operazioni sul remoto: un push non si annulla da qui (il pull sì, per la parte locale: vedi "Pull (S26)"; `remote.add` e `remote.remove` cambiano solo la configurazione e i branch remoti locali: vedi "Remoti (S27)").

### Pull (S26)

`remote.pull` entra nel diario quando sposta il branch: voce `remote.pull` con il branch da
`before` ad `after`, HEAD uguale prima e dopo, il backup `refs/project/backup/pull-...` (il
branch com'era, salvato prima di scrivere) e la descrizione `Pull di `main` (fast-forward: da
a1b2c3d a e4f5a6b)` / `(merge di origin/main: ...)` / `(rebase su origin/main: ...)`. Il fetch
fatto dal pull no, come `remote.fetch`.

- **Branch attivo: undo e redo con indice e worktree** (`GitBackend::reset_branch`): spostare
  solo il ref (come per un commit) lascerebbe nel worktree i file del pull, che sembrerebbero
  modifiche locali da annullare a mano. Prima gli stessi controlli di `restore_refs`
  (operazione in corso, HEAD e branch come li ha lasciati il pull, altrimenti `UndoConflict`) e
  i backup dei commit abbandonati (il commit di merge, i commit riscritti dal rebase:
  `refs/project/backup/undo-...`), poi `git -c core.fsmonitor=false reset --keep <oid>`: i file
  diversi fra i due commit tornano com'erano, **solo se non hanno modifiche locali** (altrimenti
  `NotAllowed` e la voce resta), le altre modifiche restano, e un non tracciato non si
  sovrascrive. Prima ancora si controlla che nessun file da ricreare esista nel worktree
  (`reset --keep` sovrascriverebbe un ignorato) e i file `--assume-unchanged`. Limite: fra il
  controllo dei ref e `reset` resta un istante in cui un altro processo potrebbe spostare il
  branch (`reset` aggiorna con il valore letto da sé); il commit spostato da fuori resterebbe nel
  reflog. Il redo è lo stesso spostamento al contrario (il commit di dopo è tenuto dal backup
  dell'undo).
- **Branch non attivo**: solo il ref (fast-forward), undo e redo con `restore_refs`.
- Un push non si annulla da qui; dopo un push l'undo di un pull precedente resta possibile
  (controlla solo il branch locale), ma il remoto ha già i commit.

Il `MemoryBackend` fa lo stesso con i suoi controlli del checkout; registra i backup dei pull in
`commit_backups` (`refs/project/backup/pull-<n>`) e quelli dei force push (`push-<n>`).

### Remoti (S27)

`remote.add` e `remote.remove` entrano nel diario con il remoto fotografato
(`JournalEntry::remote`, un `RemoteSnapshot`: configurazione `remote.<r>.*` del file del repo,
branch remoti nome → commit, dove punta `refs/remotes/<r>/HEAD`, le voci
`branch.<b>.remote`/`merge`/`pushRemote` che lo usano e `remote.pushDefault`; nel JSON del diario
gli URL sono mascherati, in memoria no). I `refs` della voce sono i branch remoti, solo da
mostrare. Due operazioni del motore, uguali per i backend:

- **`remove_remote`** (il comando, undo di `remote.add`, redo di `remote.remove`): rifiutato
  durante un'operazione in corso; il remoto deve essere **esattamente** come nella fotografia
  (`RemoteSnapshot::differences`, configurazione e upstream come insiemi), altrimenti
  `UndoConflict` (per il comando, che l'ha appena letta, `NotAllowed`) e niente cambia. Poi i
  branch remoti si tolgono con `restore_refs` (una transazione con i valori attesi; HEAD non si
  tocca) e i commit che non resterebbero raggiungibili da nessun altro ref (branch locali, tag,
  altri remoti, backup) vanno prima in un ref di backup per commit di punta,
  `refs/project/backup/remote-<s>-<ns>` (`RefRestore::backup_kind` di `remote.remove`; `undo-`
  e `redo-` per undo/redo): **scelta**: niente copia di tutti i branch remoti sotto un prefisso
  di backup, perché quasi sempre i loro commit sono anche in branch locali e il nome → commit
  è già nella voce del diario; basta che il gc non tolga quelli che erano solo lì. Infine la
  configurazione (`remove_remote_config`): sezione, `HEAD` simbolico e, solo per
  `remote.remove`, upstream, `pushRemote` e `remote.pushDefault` salvati; un problema qui va in
  `warnings` (i ref sono già tolti).
- **`restore_remote`** (undo di `remote.remove`, redo di `remote.add`): solo se del remoto non
  c'è niente (non è fra i remoti configurati, nessuna voce `remote.<r>.*`, nessun branch remoto),
  altrimenti `UndoConflict` (un remoto ricreato con lo stesso nome non si sovrascrive). Prima i
  branch remoti (`restore_refs`, creati solo se non esistono), poi la configurazione
  (`write_remote_config`). Per `remote.remove` anche gli upstream, ma **solo** per i branch che
  esistono ancora e non hanno nel frattempo un altro upstream (`GitBackend::upstream`) o un
  altro `pushRemote`, e `remote.pushDefault` solo se ora non c'è: gli altri restano come sono,
  con un avviso in `warnings`. La voce del diario si aggiorna con quello che è stato davvero
  rimesso, così il redo confronta e toglie solo quello (l'upstream reimpostato altrove resta).
  Nel confronto una chiave senza valore vale `true` (si riscrive così).

La fotografia nel JSON ha le credenziali mascherate in ogni valore (anche `remote.<r>.proxy`) e
per questo non si deserializza (`JournalEntry::remote` è `skip_deserializing`).

`remote.add` non tocca gli upstream: la sua voce li fotografa com'erano (di solito nessuno) e
l'undo è rifiutato se un branch nel frattempo ha preso il remoto come upstream (es. con il primo
`remote.push`), o se un fetch ha cambiato i branch remoti; l'undo non toglie mai i tag scaricati
(potrebbero essere arrivati anche da altri remoti). Un remoto aggiunto con branch remoti già
presenti sotto `refs/remotes/<nome>/` è rifiutato prima, perché l'undo li cancellerebbe.

**Il fetch non entra nel diario** (scelta, S25): sposta solo branch remoti e tag, che
rispecchiano il remoto. Riportarli indietro farebbe vedere un remoto che non esiste più (e il
fetch successivo li rimetterebbe dov'erano), e niente di locale va perso: gli oggetti si
aggiungono soltanto, branch locali, indice e worktree non si toccano, e un branch remoto tolto
da `fetch.prune` è sparito dal remoto. Le voci già nel diario non ne risentono: controllano solo
HEAD e i ref che hanno spostato. L'eccezione è un refspec configurato verso `refs/heads/` (un
mirror): il valore di prima resta nel reflog del ref e il risultato lo segnala in `warnings`.

## 8. Interfaccia (L6)

Layout come GitKraken:

- **Barra in alto**: repo aperto, pulsanti fetch/pull/push, undo/redo, stato del job in corso.
- **Colonna sinistra**: branch locali, remoti, tag, stash (sotto, "Stash (S28)").
- **Centro**: grafo e lista dei commit, con riga "modifiche non committate" in cima. Lista
  virtualizzata (`ScrollArea::show_rows`): si disegnano solo le righe visibili, il log si carica
  a pagine mentre si scorre.
- **Destra**: dettagli del commit selezionato, oppure area di stage (file non in stage e in stage,
  messaggio di commit) quando è selezionata la riga delle modifiche.
- **Diff**: sotto o al posto del grafo, con selezione di hunk e righe.

Ogni interazione produce un `Action` che passa da `Registry::run`, come oggi.

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
