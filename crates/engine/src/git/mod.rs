//! Accesso a un repository git tramite trait (vedi `impl-docs/ARCHITETTURA-git.md` §4).
//!
//! Il motore non esegue `git` e non tocca il file system: legge il repo tramite
//! [`GitBackend`], aperto da un [`RepoOpener`] iniettato dall'app. Le implementazioni sono
//! [`MemoryBackend`] (qui, per i test e la versione web) e `CliBackend` (in `project-git`).

mod backup;
mod branch;
mod commit;
mod discard;
mod line_diff;
mod lines;
mod memory;
mod pull;
mod push;
mod remote;
mod restore;
mod sample;

#[cfg(test)]
mod memory_diff_tests;
#[cfg(test)]
mod memory_tests;

pub use backup::{
    BACKUP_PREFIX, StagedVersion, backup_message, path_matches, versions_at_risk,
    versions_at_risk_hidden,
};
pub use branch::{
    BranchDeleted, ConfigEntry, HEADS_PREFIX, branch_ref, branch_short, check_branch_name,
    check_new_branch_name, check_no_operation,
};
pub use commit::{
    CommitOutcome, CommitRequest, amend_impact, check_amend_published, check_commit_state,
    cleanup_message, split_message,
};
pub use discard::{
    DiscardOutcome, DiscardedFile, WorktreeBackup, WorktreeRestore, check_same_plan,
    discard_description, discard_plan,
};
pub use lines::{LinePatch, LineSelection, apply_patch, file_digest, line_patch};
pub use memory::{FetchFailure, MemoryBackend, MemoryOpener, StashedFile, WorktreeStash};
pub use pull::{
    PullMode, PullOperation, PullOutcome, PullPolicy, PullRequest, PullSettings, PullStrategy,
    StrategySource, UpstreamConfig, check_pull_state, merge_message, pull_mode,
};
pub use push::{PushKind, PushOutcome, PushRejection, PushRequest, tracking_ref};
pub use remote::{FetchOutcome, FetchRequest, check_remote_name, default_remote, unknown_remote};
pub(crate) use restore::short;
pub use restore::{HeadState, RefMove, RefRestore, RestoreOutcome, valid_ref_name};

use crate::{EngineError, JobContext};
use project_domain::{Commit, CommitDetails, Diff, Head, Oid, RefInfo, RepoPath, Status};
use serde::{Deserialize, Serialize};

/// Un repository aperto: letture e, uno step alla volta, scritture (stage e unstage per ora).
///
/// `Send` perché la sessione può spostarlo su un thread di lavoro per i comandi lenti (§6).
pub trait GitBackend: Send {
    /// Dove punta HEAD.
    fn head(&self) -> Result<Head, EngineError>;

    /// Branch locali, branch remoti, tag e stash, ordinati per nome completo (byte per byte),
    /// come `git for-each-ref`. Per i branch locali `upstream` riporta il nome completo del ref
    /// remoto (es. `refs/remotes/origin/main`) e ahead/behind, `None` se il ref non esiste più.
    fn refs(&self) -> Result<Vec<RefInfo>, EngineError>;

    /// Commit raggiungibili da `query.rev` (HEAD se manca), figli sempre prima dei genitori e,
    /// a parità, il più recente per data del committer prima. Su un branch senza commit e senza
    /// `rev` il risultato è vuoto; una `rev` che non si risolve dà
    /// [`EngineError::UnknownRev`]. Con `query.all` parte da tutti i branch locali e remoti, i
    /// tag e HEAD (una HEAD senza commit si salta); `rev` e `all` insieme danno
    /// [`EngineError::InvalidParams`] ([`LogQuery::check`]).
    fn log(&self, query: &LogQuery) -> Result<Vec<Commit>, EngineError>;

    /// Stato del worktree e dell'indice, con HEAD e upstream del branch corrente.
    fn status(&self) -> Result<Status, EngineError>;

    /// Metadati e file cambiati del commit a cui porta `rev` (un id, anche abbreviato, o
    /// qualunque rev accettata da [`GitBackend::log`]). I file sono confrontati con il **primo
    /// genitore**, anche per i merge (i file portati nel branch dal merge); un commit radice è
    /// confrontato con l'albero vuoto. Le rinomine sono riconosciute. Una `rev` che non si
    /// risolve in un commit dà [`EngineError::UnknownRev`].
    fn show_commit(&self, rev: &str) -> Result<CommitDetails, EngineError>;

    /// Il diff descritto da `spec` (vedi [`DiffSpec`]), con le rinomine riconosciute.
    ///
    /// Con un percorso in `spec` il risultato ha solo il file con quel percorso **esatto**,
    /// nuovo o vecchio: una cartella non comprende i file che contiene, e una rinomina resta
    /// una rinomina (con l'altro percorso) anche se si chiede solo uno dei due. Un percorso
    /// che non è cambiato dà un diff vuoto.
    ///
    /// Non c'è un limite di dimensione: il diff di un commit enorme è tutto in memoria. Per i
    /// commit grandi conviene chiedere un file alla volta (l'elenco viene da
    /// [`GitBackend::show_commit`]). Per [`DiffSpec::Commit`] una `rev` che non si risolve in
    /// un commit dà [`EngineError::UnknownRev`].
    fn diff(&self, spec: &DiffSpec) -> Result<Diff, EngineError>;

    // --- Scritture: cambiano il repo. Prendono `&mut self` perché il `MemoryBackend` tiene lo
    // stato in sé; una lettura fatta dopo vede sempre il nuovo stato.
    //
    // Stage e unstage restituiscono il nome del ref di backup (`refs/project/backup/...`) se
    // prima di scrivere hanno salvato versioni in stage che esistevano solo nell'indice
    // ([`versions_at_risk`]); se il backup non riesce l'indice non si tocca e il risultato è
    // l'errore.
    //
    // Il controllo dei rischi legge lo stato prima di scrivere: fra la lettura e la scrittura
    // un altro processo git potrebbe cambiare l'indice (finestra breve, solo con scritture
    // esterne concorrenti). Il `CliBackend` ricontrolla le voci toccate subito prima di
    // scrivere e, se sono cambiate, dà errore senza scrivere; resta solo l'istante fra
    // quell'ultimo controllo e la scrittura.

    /// Mette in stage i file `paths` com'erano nel worktree (`git add`): modifiche, file nuovi
    /// non tracciati e cancellazioni. Per una rinomina fatta nel worktree si passano i due
    /// percorsi (la cancellazione del vecchio e il nuovo file). Non tocca il worktree.
    ///
    /// - I percorsi sono presi alla lettera (niente glob né pathspec magici); un percorso che
    ///   è una cartella comprende i file che contiene, come in git (i file ignorati al suo
    ///   interno sono saltati).
    /// - Un file ignorato da `.gitignore` e non tracciato, chiesto per nome, dà
    ///   [`EngineError::NotAllowed`] e **niente** viene messo in stage (non si forza mai).
    /// - Un percorso che non esiste né nel worktree né nell'indice né in HEAD dà errore e
    ///   niente cambia; uno (file o cartella) che indica solo cancellazioni già in stage non
    ///   fa niente.
    /// - Un file in conflitto messo in stage è **risolto** con il contenuto del worktree (come
    ///   `git add`): gli stadi del conflitto spariscono dall'indice (git ne tiene le versioni
    ///   nell'estensione REUC, da cui `git checkout -m` rifà il conflitto). Solo se chiesto per
    ///   nome: un file in conflitto dentro una cartella richiesta dà
    ///   [`EngineError::NotAllowed`] e niente cambia.
    /// - `paths` vuoto non fa niente (non vuol dire "tutto").
    fn stage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError>;

    /// Toglie dallo stage i file `paths`: l'indice torna come in HEAD per quei percorsi (un
    /// file aggiunto torna non tracciato), come `git restore --staged`. Su un branch senza
    /// commit l'indice torna vuoto per quei percorsi. **Non tocca mai il worktree.**
    ///
    /// - Percorsi alla lettera, cartelle comprese, come in [`GitBackend::stage`].
    /// - Un file in conflitto dà [`EngineError::NotAllowed`]: togliere dall'indice gli stadi
    ///   del conflitto ne perderebbe le versioni; si risolve con [`GitBackend::stage`].
    /// - Un percorso sconosciuto a git (né in HEAD né nell'indice) dà errore e niente cambia.
    /// - `paths` vuoto non fa niente.
    fn unstage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError>;

    /// Mette in stage ([`PatchDirection::Stage`](project_domain::PatchDirection)) o toglie
    /// dallo stage ([`PatchDirection::Unstage`](project_domain::PatchDirection)) solo le righe
    /// `selection.lines` di un file: `index.stage_lines` e `index.unstage_lines`. **Non tocca
    /// mai il worktree.**
    ///
    /// Rilegge il diff del file ([`LineSelection::diff_spec`]), controlla che sia quello visto
    /// (`selection.digest`) e costruisce la patch con [`line_patch`] (vedi lì gli errori), poi
    /// calcola il nuovo contenuto in stage con [`apply_patch`] (rigoroso: ogni hunk solo alla
    /// sua posizione) e lo scrive nell'indice. Come stage e unstage: prima un backup se la
    /// versione in stage esiste solo nell'indice (es. righe di un file `MM`), e il ref nel
    /// risultato; il `CliBackend` ricontrolla l'indice prima di scrivere, rilegge la voce dopo
    /// e con l'indice bloccato dà [`EngineError::IndexLocked`]. Un errore prima della scrittura
    /// non cambia niente.
    fn apply_lines(&mut self, selection: &LineSelection) -> Result<Option<String>, EngineError>;

    /// Crea un commit con quello che è in stage (`commit.create`), o con `request.amend`
    /// riscrive l'ultimo. Il worktree non si tocca; dopo, l'indice è uguale al nuovo HEAD.
    ///
    /// Prima di scrivere: [`CommitRequest::check_message`], [`check_commit_state`] (conflitti,
    /// operazioni in corso, niente da committare, `amend` senza commit o durante un merge) e,
    /// con `amend`, [`check_amend_published`] sui branch remoti e i tag che contengono HEAD
    /// ([`amend_impact`]; gli altri branch e tag che lo contengono vanno in `also_on`). Con
    /// `amend` il commit riscritto è salvato in un ref di backup
    /// (`refs/project/backup/amend-...`, nel risultato) prima di scrivere; se il backup non
    /// riesce non si committa. Il `CliBackend` esegue `git commit` con hook e firma secondo la
    /// configurazione dell'utente: un hook che rifiuta o una firma che non riesce danno
    /// [`EngineError::Git`] con tutto quello che hanno scritto, e nessun commit.
    ///
    /// Primo commit di un branch e HEAD staccata funzionano (con HEAD staccata si sposta
    /// HEAD, nessun branch). Senza identità (`user.name`/`user.email`) l'errore è quello di
    /// git: nessun nome inventato.
    fn commit(&mut self, request: &CommitRequest) -> Result<CommitOutcome, EngineError>;

    /// Sposta HEAD e i ref come chiede `request` (`edit.undo` e `edit.redo`, §7): tutti insieme
    /// o nessuno, e **solo** se ora valgono `expected`. Indice e worktree non si toccano.
    /// Restituisce i ref di backup creati.
    ///
    /// Prima di scrivere: [`RefRestore::check`] (operazioni in corso, nomi, cambi di HEAD non
    /// supportati); poi HEAD e i ref attuali confrontati con `expected`
    /// ([`RefRestore::conflicts`]): se qualcosa è cambiato da fuori,
    /// [`EngineError::UndoConflict`] e niente cambia. Ogni commit di
    /// [`RefRestore::abandoned`] che dopo non sarebbe più raggiungibile né dai nuovi valori
    /// né da altri ref (backup compresi) si salva in `refs/project/backup/<undo|redo>-...`;
    /// se il backup non riesce non si sposta niente. Il `CliBackend` scrive con una sola
    /// transazione `git update-ref --stdin` (`start`, `update`/`create`/`delete` con il valore
    /// vecchio, `commit`) e il messaggio `request.reflog`; se la transazione fallisce perché
    /// un ref è cambiato nel frattempo, ancora [`EngineError::UndoConflict`]. Un problema
    /// scoperto dopo una transazione riuscita (HEAD cambiata da un altro processo proprio in
    /// quell'istante) va in [`RestoreOutcome::warnings`].
    fn restore_refs(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError>;

    /// Scarta le modifiche non in stage dei file `paths` (`worktree.discard`, §7): il worktree
    /// torna come l'indice, come `git restore --worktree`. **L'indice non si tocca.**
    ///
    /// I file sono quelli di [`discard_plan`] sullo stato letto ora (vedi lì i rifiuti: non
    /// tracciati chiesti per nome, sottomoduli, conflitti, `intent-to-add`, branch senza
    /// commit, niente da scartare); i non tracciati dentro una cartella non si toccano. Prima di
    /// scrivere il worktree si salva con `git stash create` in un ref nuovo
    /// `refs/project/backup/discard-...` (nel risultato, con il commit e i file); se il backup
    /// non riesce, o non contiene il contenuto attuale di tutti i file, niente si scarta.
    /// `paths` vuoto non fa niente (errore: niente da scartare).
    ///
    /// `expected` è il piano mostrato a chi ha confermato ([`discard_plan`] su uno stato letto
    /// prima): se il piano calcolato ora è diverso (un file in più o in meno, un'altra
    /// modifica, un'altra versione in stage), [`EngineError::NotAllowed`] e niente cambia.
    fn discard(
        &mut self,
        paths: &[RepoPath],
        expected: &[DiscardedFile],
    ) -> Result<DiscardOutcome, EngineError>;

    /// Annulla (`request.redo == false`) o rifà uno scarto (`edit.undo`/`edit.redo` di una voce
    /// di `worktree.discard`). L'indice non si tocca.
    ///
    /// Undo: ogni file deve avere ancora nell'indice la voce registrata
    /// ([`WorktreeRestore::index_conflicts`]) e il worktree uguale all'indice (come l'ha lasciato
    /// lo scarto); poi torna com'era nel commit `request.stash` (un file che lì non c'è, perché
    /// era eliminato, si cancella). Redo: stessa voce dell'indice e worktree uguale a
    /// `request.stash` (come l'ha lasciato l'undo); poi torna alla versione in stage. Se un file
    /// è cambiato, [`EngineError::UndoConflict`] e niente cambia.
    fn restore_worktree(
        &mut self,
        request: &WorktreeRestore,
    ) -> Result<RestoreOutcome, EngineError>;

    // --- Branch (S24). I nomi sono brevi (`feature/x`), già controllati dal comando con
    // [`check_branch_name`] / [`check_new_branch_name`]; ai backend arrivano solo come
    // `refs/heads/<nome>` o dopo `--end-of-options`, così un nome che inizia con `-` non
    // diventa un'opzione.

    /// Quanti commit raggiungibili da `tip` non lo sono da nessuno di `into` (`git rev-list
    /// --count <tip> ^<into>...`): 0 vuol dire che `tip` è unito. Lettura, per la conferma di
    /// `branch.delete`.
    fn unmerged_count(&self, tip: &Oid, into: &[Oid]) -> Result<usize, EngineError>;

    /// Crea il branch locale `name` sul commit `target` (`branch.create`), **solo se non
    /// esiste** (`update-ref refs/heads/<nome> <id> ""`: un ref esistente non si sovrascrive
    /// mai, nemmeno se creato da un altro processo nell'istante prima). HEAD, indice e
    /// worktree non si toccano; nessun upstream. Un nome già preso (anche da un ref che lo
    /// contiene, come `a` per `a/b`) dà [`EngineError::NotAllowed`]; un nome che git rifiuta
    /// [`EngineError::InvalidParams`].
    fn create_branch(&mut self, name: &str, target: &Oid) -> Result<(), EngineError>;

    /// Porta HEAD su `target` aggiornando indice e worktree (`git switch`): il branch locale
    /// (nome completo, deve esistere) o un commit con HEAD staccata (per l'undo di un checkout
    /// fatto da HEAD staccata). `branch.checkout` e il suo undo/redo.
    ///
    /// **Non perde mai modifiche locali**: mai `--force`, `--discard-changes` né `--merge`.
    /// Le modifiche che non toccano i file diversi fra HEAD e `target` restano (come in git);
    /// se un file modificato (in stage o no), non tracciato o ignorato verrebbe sovrascritto,
    /// [`EngineError::CheckoutConflict`] con i file e niente cambia. Rifiutati prima di
    /// toccare qualcosa ([`EngineError::NotAllowed`]): operazioni in corso, file in conflitto,
    /// `target` che non esiste. Restituisce gli avvisi scoperti dopo un checkout riuscito
    /// (es. HEAD diversa da `target` per un hook `post-checkout`).
    fn checkout(&mut self, target: &HeadState) -> Result<RestoreOutcome, EngineError>;

    /// Elimina il branch locale `name`, solo se vale ancora `expected` (`branch.delete`).
    /// Prima salva il commit in un ref di backup nuovo `refs/project/backup/delete-...` (nel
    /// risultato); se il backup non riesce non si elimina niente. Toglie poi la configurazione
    /// `branch.<nome>.*` (upstream, ...), come `git branch -d`. Rifiutato
    /// ([`EngineError::NotAllowed`], niente cambia) se è il branch di HEAD o se è aperto in un
    /// altro worktree, o se non vale più `expected`. Il controllo "unito o no" lo fa il comando
    /// ([`GitBackend::unmerged_count`]).
    fn delete_branch(&mut self, name: &str, expected: &Oid) -> Result<BranchDeleted, EngineError>;

    /// Rinomina il branch locale `from` in `to` (`branch.rename` e il suo undo/redo), solo se
    /// `from` vale ancora `expected` e `to` non esiste (mai sovrascritto). Come `git branch
    /// -m`: reflog e configurazione `branch.<nome>.*` seguono il branch, e HEAD (anche di altri
    /// worktree) che puntava a `from` punta a `to`. Indice e worktree non si toccano.
    /// Restituisce gli avvisi scoperti dopo la rinomina riuscita. Rifiuti
    /// ([`EngineError::NotAllowed`], niente cambia): `from` assente o cambiato, `to` già
    /// presente; un nome `to` che git rifiuta: [`EngineError::InvalidParams`].
    fn rename_branch(
        &mut self,
        from: &str,
        to: &str,
        expected: &Oid,
    ) -> Result<RestoreOutcome, EngineError>;
    /// La configurazione `branch.<name>.*` del repo per undo e redo di `branch.delete`, dopo
    /// che il ref è stato ricreato o eliminato. `remove == false` (undo): riscrive `entries`,
    /// solo se il repo non ha già voci `branch.<name>.*` (altrimenti non scrive niente e lo dice
    /// in un avviso). `remove == true` (redo): toglie la sezione, solo se contiene esattamente
    /// `entries` (quello che l'undo ha rimesso); una configurazione diversa resta, con un
    /// avviso. Restituisce gli avvisi: il ref è già spostato, quindi un problema qui non è un
    /// errore dell'operazione.
    fn restore_branch_config(
        &mut self,
        name: &str,
        entries: &[ConfigEntry],
        remove: bool,
    ) -> Vec<String>;

    /// I nomi dei remoti configurati (`git remote`), in ordine di nome. Lettura.
    fn remotes(&self) -> Result<Vec<String>, EngineError>;

    /// Scarica dal remoto `request.remote` (`git fetch --progress`, §6): oggetti nuovi e branch
    /// remoti (`refs/remotes/<remote>/...`) e tag come dice la configurazione del remoto, più
    /// `FETCH_HEAD`. Mai i branch locali, l'indice o il worktree (salvo un refspec configurato
    /// apposta verso `refs/heads/`, che il comando segnala). Con `request.branch` solo quel
    /// branch del remoto (`refs/heads/<branch>`), che aggiorna il suo branch remoto come dice il
    /// refspec configurato. Comando lento: l'avanzamento va a `job` ([`JobContext::report`]);
    /// se `job` è annullato il processo si ferma e il risultato è
    /// [`EngineError::Cancelled`]. Errori: credenziali → [`EngineError::Auth`], remoto non
    /// raggiungibile → [`EngineError::RemoteUnreachable`], branch che il remoto non ha →
    /// [`EngineError::NotAllowed`], altro → [`EngineError::Git`]. Il remoto è già controllato
    /// dal comando (configurato, nome valido).
    fn fetch(
        &mut self,
        request: &FetchRequest,
        job: &JobContext,
    ) -> Result<FetchOutcome, EngineError>;

    // --- Pull e push (S26). I nomi arrivano già controllati dal comando.

    /// L'upstream configurato del branch locale `branch` (nome breve): `branch.<nome>.remote`,
    /// `branch.<nome>.merge` e il branch remoto locale che lo rispecchia. `None` se il branch
    /// non ha un upstream. Lettura.
    fn upstream(&self, branch: &str) -> Result<Option<UpstreamConfig>, EngineError>;

    /// La configurazione della strategia di pull per il branch `branch` (`branch.<nome>.rebase`
    /// o `pull.rebase`, `pull.ff`), con i valori com'erano scritti. Lettura.
    fn pull_settings(&self, branch: &str) -> Result<PullSettings, EngineError>;

    /// Tutti i valori della chiave `key` della configurazione (tutte le fonti, come la legge
    /// git: `git config --get-all`), nell'ordine; vuoto se non c'è. Lettura. Le chiavi le
    /// costruisce il motore da nomi già controllati (es. `branch.<nome>.pushRemote`,
    /// `remote.<nome>.fetch`).
    fn config_values(&self, key: &str) -> Result<Vec<String>, EngineError>;

    /// Al più `limit` commit raggiungibili da `tip` ma da nessuno di `exclude`, dal più recente
    /// (`git log <tip> --not <exclude>...`). Lettura: per la conferma di un force push (i
    /// commit che il remoto perderebbe).
    fn missing_commits(
        &self,
        tip: &Oid,
        exclude: &[Oid],
        limit: usize,
    ) -> Result<Vec<Commit>, EngineError>;

    /// Integra l'upstream nel branch dopo il fetch (`remote.pull`): fast-forward, merge o
    /// rebase secondo `request.mode`. Prima di scrivere: [`check_pull_state`] sullo stato letto
    /// ora, il branch deve valere `request.expected` (e, con `request.current`, essere il branch
    /// di HEAD; altrimenti non deve essere aperto in un altro worktree) e un ref di backup
    /// `refs/project/backup/pull-...` sul valore di prima (nel risultato). Fast-forward del
    /// branch attivo: `git merge --ff-only`, che non sovrascrive modifiche locali
    /// ([`EngineError::NotAllowed`] con i file, niente cambia); di un altro branch:
    /// `update-ref` con il valore vecchio. Merge e rebase: mai `--autostash`; se si fermano in
    /// conflitto vengono annullati (`--abort`) e il risultato è
    /// [`EngineError::PullConflict`] con i file (niente cambia, il backup si toglie). Non si
    /// annulla a metà: il fetch, che si può annullare, è già fatto.
    fn pull(&mut self, request: &PullRequest) -> Result<PullOutcome, EngineError>;

    /// Undo e redo di un pull sul branch attivo: come [`GitBackend::restore_refs`] (un solo ref,
    /// il branch di HEAD, da `expected` a `target`, con gli stessi controlli e i backup dei
    /// commit abbandonati), ma aggiorna anche indice e worktree come `git reset --keep`: i
    /// file diversi fra i due commit tornano come in `target`, solo se non hanno modifiche
    /// locali (altrimenti [`EngineError::NotAllowed`] e niente cambia); le altre modifiche
    /// restano. Non sovrascrive mai un file non tracciato o ignorato.
    fn reset_branch(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError>;

    /// Pubblica il branch sul remoto (`remote.push`, lento come il fetch: avanzamento e
    /// annullamento da `job`): solo `refs/heads/<branch>:refs/heads/<remote_branch>`, mai tag
    /// o altri ref, con `--set-upstream` se chiesto e `--force-with-lease` con l'oid atteso se
    /// `request.lease` (mai `--force`). Prima di un force, il backup di `request.backup`.
    /// Errori: rifiutato (non fast-forward, lease, hook `pre-push`, remoto) →
    /// [`EngineError::PushRejected`] e niente cambia; credenziali → [`EngineError::Auth`];
    /// remoto non raggiungibile → [`EngineError::RemoteUnreachable`]; annullato →
    /// [`EngineError::Cancelled`].
    fn push(&mut self, request: &PushRequest, job: &JobContext)
    -> Result<PushOutcome, EngineError>;
}

/// Cosa confrontare in [`GitBackend::diff`]: parametri di `diff.worktree`, `diff.index` e
/// `diff.commit` (§4). `path`, se c'è, limita il diff a quel file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSpec {
    /// Worktree rispetto all'indice: le modifiche non in stage (`git diff`). I file non
    /// tracciati non compaiono.
    Worktree { path: Option<RepoPath> },
    /// Indice rispetto a HEAD: le modifiche in stage (`git diff --cached`). Su un branch senza
    /// commit il confronto è con l'albero vuoto (ogni file in stage è aggiunto).
    Index { path: Option<RepoPath> },
    /// Il commit a cui porta `rev` rispetto al **primo genitore**, anche per i merge; un
    /// commit radice rispetto all'albero vuoto. Gli stessi file di
    /// [`GitBackend::show_commit`].
    Commit { rev: String, path: Option<RepoPath> },
}

impl DiffSpec {
    /// Il file a cui è limitato il diff, se c'è.
    pub fn path(&self) -> Option<&RepoPath> {
        match self {
            DiffSpec::Worktree { path } | DiffSpec::Index { path } => path.as_ref(),
            DiffSpec::Commit { path, .. } => path.as_ref(),
        }
    }
}

/// Apre un repository dato il percorso. L'app lo inietta nella sessione: desktop e CLI usano
/// `CliOpener` di `project-git`, la versione web [`MemoryOpener`].
pub trait RepoOpener: Send + Sync {
    /// [`EngineError::RepoNotFound`] se al percorso non c'è un repository.
    fn open(&self, path: &str) -> Result<Box<dyn GitBackend>, EngineError>;
}

/// Una pagina del log: parametri di `repo.log` (§4), `{"skip"?, "limit"?, "rev"?, "all"?}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogQuery {
    /// Commit da saltare dall'inizio del log.
    pub skip: usize,
    /// Numero massimo di commit; `None` = tutti.
    pub limit: Option<usize>,
    /// Da dove partire: nome di un ref (`main`, `origin/main`, `v1.0`, `refs/heads/main`),
    /// `HEAD` o un id (anche abbreviato, almeno 4 cifre). `None` = HEAD.
    pub rev: Option<String>,
    /// Parte da tutti i branch locali, i branch remoti, i tag e HEAD (anche staccata), come
    /// `git log --branches --remotes --tags HEAD`. Gli altri ref (`refs/stash`, i backup in
    /// `refs/project/backup/`, le note) non ci sono. Esclude `rev`.
    pub all: bool,
}

impl LogQuery {
    /// Errore se i parametri si contraddicono: `rev` e `all` insieme.
    pub fn check(&self) -> Result<(), EngineError> {
        if self.all && self.rev.is_some() {
            return Err(EngineError::InvalidParams {
                id: "repo.log",
                reason: "`rev` e `all` sono alternativi: `all` parte già da tutti i branch, i \
                         tag e HEAD"
                    .to_owned(),
            });
        }
        Ok(())
    }

    /// Stessa pagina, a partire da tutti i branch, i tag e HEAD.
    pub fn from_all_refs(mut self) -> Self {
        self.all = true;
        self
    }

    /// Tutto il log da HEAD.
    pub fn all() -> Self {
        Self::default()
    }

    /// `limit` commit dopo i primi `skip`, da HEAD.
    pub fn page(skip: usize, limit: usize) -> Self {
        Self {
            skip,
            limit: Some(limit),
            rev: None,
            all: false,
        }
    }

    /// Stessa pagina, a partire da `rev`.
    pub fn from_rev(mut self, rev: impl Into<String>) -> Self {
        self.rev = Some(rev.into());
        self
    }
}
