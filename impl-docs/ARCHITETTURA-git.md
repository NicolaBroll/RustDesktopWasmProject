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


Il costruttore di patch per lo stage di righe è in [git/stage.md](git/stage.md).

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
    // Merge (S29): `MergeRequest` (branch, expected, target, target_name, mode = FastForward |
    // Merge { no_ff }, message); `MergeOutcome` (after, backup, in_progress, conflicts, message,
    // warnings): un merge fermo in conflitto resta in corso. L'abort salva prima indice e
    // worktree (`MergeAborted`: backup, files). `OperationState` (operation, incoming =
    // MERGE_HEAD / CHERRY_PICK_HEAD / REVERT_HEAD, message = MERGE_MSG, onto, rebase_branch)
    // per `repo.operation`. `merge.continue` usa `commit`.
    fn merge(&mut self, request: &MergeRequest) -> Result<MergeOutcome, EngineError>;
    fn merge_abort(&mut self) -> Result<MergeAborted, EngineError>;
    fn operation_state(&self) -> Result<OperationState, EngineError>;
    // poi rebase, ...
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
  `fail_stash_drop` simula un drop che fallisce (per il pop applicato e non tolto). Merge (S29):
  il merge file per file (`merge_three_way`, lo usa anche il pull in merge): un conflitto lascia il merge in corso (voci `u`
  con i marcatori nel worktree, `MERGE_HEAD` e `MERGE_MSG` in memoria) e il commit con un merge in
  corso fa il commit di merge; vedi [git/merge.md](git/merge.md).
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
come JSON, così CLI e MCP li usano direttamente. Contratto, `CliBackend`, undo e backup di
ogni comando stanno nel file della sua funzione; un comando nuovo si aggiunge al file della
sua funzione (o a un file nuovo in `impl-docs/git/`, elencato qui).

| Comandi | Dove |
| --- | --- |
| `repo.open`, `repo.status`, `repo.refs`, `repo.log`, `commit.show`, `diff.worktree` / `diff.index` / `diff.commit` | [git/lettura.md](git/lettura.md) |
| `index.stage` / `index.unstage`, `index.stage_lines` / `index.unstage_lines` | [git/stage.md](git/stage.md) |
| `worktree.discard` | [git/discard.md](git/discard.md) |
| `commit.create` | [git/commit.md](git/commit.md) |
| `branch.create`, `branch.checkout`, `branch.delete`, `branch.rename` | [git/branch.md](git/branch.md) |
| `remote.fetch`, `remote.pull`, `remote.push` | [git/sync.md](git/sync.md) |
| `remote.list`, `remote.add`, `remote.remove` | [git/remoti.md](git/remoti.md) |
| `stash.list`, `stash.push`, `stash.apply` / `stash.pop`, `stash.drop` | [git/stash.md](git/stash.md) |
| `merge.start`, `merge.continue`, `merge.abort`, `repo.operation` | [git/merge.md](git/merge.md) |
| `edit.undo` / `edit.redo`, `edit.history` | qui sotto e §7 |

| Id | Parametri | Note |
| --- | --- | --- |
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
- Messaggi di commit via stdin (`git commit -F -`), mai come argomento (dettagli in [git/commit.md](git/commit.md)).
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
- Fetch, pull e push: [git/sync.md](git/sync.md); remoti: [git/remoti.md](git/remoti.md); stash: [git/stash.md](git/stash.md); merge: [git/merge.md](git/merge.md).
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
`branch.*` (vedi [git/branch.md](git/branch.md)), `stash.push` e `stash.drop` (vedi [git/stash.md](git/stash.md)), il merge concluso (`merge.start`, `merge.continue`: vedi [git/merge.md](git/merge.md); un merge fermo in conflitto e `merge.abort` no) e `worktree.discard` (nessun ref, HEAD uguale prima e dopo, il backup del worktree nel campo
`"worktree"`, vedi [git/discard.md](git/discard.md)). Stage e unstage non spostano ref e non entrano nel diario: il loro recupero è il
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
  `GitBackend::rename_branch`, vedi [git/branch.md](git/branch.md)), poi HEAD e ref attuali confrontati con quelli attesi
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


Undo e backup dei singoli comandi stanno nel file della funzione: scartare le modifiche
([git/discard.md](git/discard.md)), backup dell'indice di stage e unstage ([git/stage.md](git/stage.md)),
backup di `amend` ([git/commit.md](git/commit.md)), branch ([git/branch.md](git/branch.md)), stash
([git/stash.md](git/stash.md)), pull e fetch ([git/sync.md](git/sync.md)), remoti ([git/remoti.md](git/remoti.md)), merge
e il backup di `merge.abort` ([git/merge.md](git/merge.md)).

L'undo non copre le operazioni sul remoto: un push non si annulla da qui (il pull sì, per la parte locale: vedi [git/sync.md](git/sync.md); `remote.add` e `remote.remove` cambiano solo la configurazione e i branch remoti locali: vedi [git/remoti.md](git/remoti.md)).

## 8. Interfaccia (L6)

Layout come GitKraken:

- **Barra in alto**: repo aperto, pulsanti fetch/pull/push, undo/redo, stato del job in corso.
- **Colonna sinistra**: branch locali, remoti, tag, stash ([git/stash.md](git/stash.md)).
- **Centro**: grafo e lista dei commit, con riga "modifiche non committate" in cima. Lista
  virtualizzata (`ScrollArea::show_rows`): si disegnano solo le righe visibili, il log si carica
  a pagine mentre si scorre.
- **Destra**: dettagli del commit selezionato, oppure area di stage (file non in stage e in stage,
  messaggio di commit) quando è selezionata la riga delle modifiche.
- **Diff**: sotto o al posto del grafo, con selezione di hunk e righe.

Ogni interazione produce un `Action` che passa da `Registry::run`, come oggi.

La sezione Stash e il suo pannello: [git/stash.md](git/stash.md).

**Operazione in corso** (S29, [git/merge.md](git/merge.md)): quando `repo.status` ha `operation`
(merge, rebase, cherry-pick, revert, bisect, `git am`) sotto la barra compare un riquadro per tutta
la larghezza, con i dettagli di `repo.operation` (riletto a ogni stato nuovo, uno alla volta): cosa
sta entrando, i file in conflitto (i primi otto; un clic apre l'area di stage con il loro diff) e
i pulsanti dei comandi `continue`/`abort` quando non sono `null`. "Continua il merge…" (attivo solo
con `ready`) apre un dialogo con il messaggio del commit di merge precompilato da `message`;
"Annulla il merge…" va in due tempi (senza `confirm` il motore risponde `ConfirmRequired`, il
dialogo rosso mostra il motivo, poi `"confirm": true`). Per le operazioni che la UI non conclude
ancora il riquadro dice come farlo da terminale. Durante un'operazione la casella del commit è
spenta (il merge si conclude con `merge.continue`, che entra nel diario come merge; durante un
bisect si committa normalmente). Il merge si avvia dal menu contestuale di un branch (locale non
attivo, remoto), di un tag o di un commit: "Unisci in «branch»" e "… con commit di merge"
(`no_ff`), spente con il motivo se HEAD è staccata o c'è già un'operazione in corso. L'esito va
nella barra di stato (fast-forward, commit di merge, già aggiornato); un merge fermo in conflitto
apre l'area di stage e il riquadro; un rifiuto (modifiche locali, rev sconosciuta) apre una
finestra. La demo web aggiunge al repo di esempio il branch `esperimento`, in conflitto con `main`
su `src/lib.rs` (`apps/project-web/src/demo.rs`): «Stash…», poi «Unisci in «main»» su
`esperimento`.

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
