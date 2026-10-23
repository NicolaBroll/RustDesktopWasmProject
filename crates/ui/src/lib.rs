//! L6 — interfaccia egui condivisa da desktop e web.
//!
//! Regola: i callback grafici non modificano il repository direttamente, producono
//! un'[`Action`]; i comandi passano dall'[`Executor`], che li esegue con
//! [`Registry::run`](project_engine::Registry::run) fuori dal thread della UI.
//!
//! Layout (`impl-docs/ARCHITETTURA-git.md` §8): barra in alto, ref a sinistra, commit al
//! centro, dettagli del commit selezionato a destra. In cima alla lista la riga "Modifiche non
//! committate" apre a destra l'area di stage. Il diff di un file si apre al posto del grafo,
//! con il pannello a destra accanto per passare da un file all'altro. Sotto le liste dell'area
//! di stage c'è la casella del messaggio di commit.
//! Con un'operazione in corso (merge, rebase, ...) sotto la barra c'è il suo riquadro
//! (`merge_view`), con i file in conflitto e i comandi per concluderla o annullarla.

mod branch;
mod commit;
mod conflict;
mod details;
mod diff;
mod diff_view;
mod discard;
mod executor;
mod fetch;
mod format;
mod graph;
mod lines;
mod list;
mod merge;
mod merge_view;
mod remotes;
mod stage;
mod stash;
mod sync;
mod view;

pub use executor::{Done, ErrorKind, Executor, Job, Ticket, Wake};

use branch::{BranchBox, Start};
use commit::{CommitBox, Gate};
use conflict::ConflictBox;
use details::DetailsState;
use diff::DiffDoc;
use diff::{DiffSource, DiffState, DiffTarget, Pane};
use discard::{DiscardBox, History, UndoOp};
use fetch::FetchBox;
use graph::RefLabel;
use list::{CommitList, PageRequest, Seek};
use merge::MergeBox;
use project_domain::{Commit, CommitDetails, Diff, Head, Oid, RefInfo, RepoPath, Status};
use project_engine::{Platform, Registry, RepoOpener, Session};
use remotes::{AddFields, RemotesBox};
use serde::Deserialize;
use serde_json::{Value, json};
use stage::{Follow, PendingWrite, Side, StageLists, StageOp};
use stash::{PushFields, StashBox};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use sync::SyncBox;

/// Ciò che l'utente ha chiesto durante il frame.
#[derive(Debug, Clone)]
enum Action {
    /// Dialogo per scegliere la cartella del repo.
    PickFolder,
    Open(String),
    /// Rilegge ref, stato e log del repo aperto.
    Refresh,
    /// Seleziona il commit alla riga indicata della lista.
    Select(usize),
    /// Seleziona la riga "Modifiche non committate": a destra l'area di stage.
    SelectWorktree,
    /// Seleziona il commit puntato da un ref, se è nella lista.
    SelectCommit(Oid),
    /// Sposta la selezione di tante righe (tastiera).
    Move(isize),
    /// Seleziona il primo commit (`true`) o l'ultimo caricato (`false`).
    Edge(bool),
    /// La lista mostra le righe fino a questa (esclusa): se serve chiede la pagina seguente.
    Visible(usize),
    /// Richiede i dettagli del commit selezionato dopo un errore.
    RetryDetails,
    /// Apre il diff di un file del commit selezionato al posto del grafo.
    OpenDiff(DiffTarget),
    /// Chiude il diff e torna al grafo.
    CloseDiff,
    /// Richiede il diff aperto dopo un errore.
    RetryDiff,
    /// Stage o unstage dei percorsi indicati (mai vuoti). Lo stage di file in conflitto chiede
    /// prima una conferma.
    Write(StageOp, Vec<RepoPath>),
    /// Conferma la scrittura in attesa.
    ConfirmWrite,
    /// Annulla la scrittura in attesa di conferma.
    CancelWrite,
    /// Nasconde l'errore dell'ultima scrittura.
    DismissWriteError,
    /// Scarta le modifiche fuori stage dei file indicati (mai vuoti): prima il motore controlla
    /// e chiede la conferma, che si apre in un dialogo.
    Discard(Vec<RepoPath>),
    /// Conferma lo scarto in attesa: reinvia con `"confirm": true`.
    ConfirmDiscard,
    /// Chiude il dialogo dello scarto senza scartare niente.
    CancelDiscard,
    /// Annulla l'ultima operazione del diario, o rifà l'ultima annullata.
    Undo(UndoOp),
    /// Clic su una riga del diff (indice in `DiffDoc::rows`): la seleziona o la deseleziona;
    /// con Maiusc seleziona l'intervallo dall'ultima cliccata.
    SelectLine {
        row: usize,
        shift: bool,
    },
    /// Trascinamento sulle righe del diff.
    DragLines(Drag),
    /// Deseleziona tutte le righe del diff.
    ClearLines,
    /// Stage (diff worktree) o unstage (diff indice) dell'hunk con questo indice.
    WriteHunk(usize),
    /// Stage o unstage delle righe selezionate.
    WriteSelectedLines,
    /// Attiva o disattiva "Amend" nella casella del messaggio.
    SetAmend(bool),
    /// Commit del messaggio scritto (pulsante o Ctrl+Invio).
    Commit,
    /// Conferma l'amend di un commit già pubblicato: reinvia con `"confirm": true`.
    ConfirmCommit,
    /// Annulla l'amend in attesa di conferma.
    CancelCommit,
    /// Nasconde l'errore dell'ultimo commit.
    DismissCommitError,
    /// Apre il dialogo per un branch nuovo da `start`, con il nome proposto.
    NewBranch(Start, String),
    /// Apre il dialogo per rinominare il branch locale (nome breve).
    RenameBranch(String),
    /// Il testo del dialogo del nome del branch è cambiato.
    BranchName(String),
    /// Manda il dialogo del nome: crea o rinomina.
    SubmitBranchName,
    /// Passa al branch locale (nome breve).
    Checkout(String),
    /// Elimina il branch locale (nome breve) visto sul commit indicato: prima senza conferma;
    /// se il motore la chiede si apre il dialogo. Il commit va al motore come `expected`.
    DeleteBranch(String, Oid),
    /// Conferma l'eliminazione in attesa: reinvia con `"confirm": true`.
    ConfirmDeleteBranch,
    /// Chiude il dialogo dei branch aperto senza mandare niente.
    CloseBranchDialog,
    /// Fetch dal remoto indicato (`None`: quello scelto dal motore: upstream, `origin`,
    /// l'unico).
    Fetch(Option<String>),
    /// "Interrompi" il fetch in corso.
    CancelFetch,
    /// Chiude la finestra dell'errore del fetch.
    DismissFetchError,
    /// Pull del branch indicato (`None`: quello di HEAD), con la strategia indicata (`None`:
    /// quella della configurazione).
    Pull(Option<String>, Option<sync::Strategy>),
    /// Push del branch indicato (`None`: quello di HEAD), mai forzato.
    Push(Option<String>),
    /// "Forza (con lease)…" nella finestra del push rifiutato: `"force": true` senza
    /// `confirm`.
    ForcePush,
    /// "Forza il push" nella conferma: `"force": true, "confirm": true`.
    ConfirmForcePush,
    /// "Interrompi" il pull o push in corso.
    CancelSync,
    /// Chiude la finestra di pull e push (anche la conferma del force, senza forzare).
    DismissSync,
    /// Apre il dialogo "Aggiungi remoto".
    AddRemote,
    /// I campi del dialogo "Aggiungi remoto" sono cambiati.
    AddRemoteFields(AddFields),
    /// "Nascondi" (`true`) o "Mostra" l'URL con credenziali nel dialogo "Aggiungi remoto".
    HideRemoteUrl(bool),
    /// Manda il dialogo "Aggiungi remoto".
    SubmitAddRemote,
    /// "Interrompi" il fetch dell'aggiunta in corso (il remoto resta aggiunto).
    CancelAddRemote,
    /// Rimuove il remoto indicato: prima senza conferma, il motore la chiede sempre e si apre
    /// il dialogo.
    RemoveRemote(String),
    /// Conferma la rimozione in attesa: reinvia con `"confirm": true`.
    ConfirmRemoveRemote,
    /// Chiude il dialogo dei remoti aperto senza mandare niente.
    CloseRemoteDialog,
    /// Chiude la finestra dell'errore del fetch dopo l'aggiunta.
    DismissRemoteError,
    /// Apre il dialogo "Stash" per mettere da parte le modifiche.
    OpenStash,
    /// I campi del dialogo "Stash" sono cambiati.
    StashFields(PushFields),
    /// Manda il dialogo "Stash" (`stash.push`).
    SubmitStash,
    /// Seleziona lo stash con questo id: a destra i suoi file, nel grafo la sua base.
    SelectStash(Oid),
    /// Applica lo stash con questo id (`true`: pop, cioè applica e se va tutto bene lo
    /// elimina), con `index` ed `expected` dall'elenco visto.
    ApplyStash(Oid, bool),
    /// "Rimetti anche lo stage" per apply e pop.
    StashRestoreIndex(bool),
    /// Elimina lo stash con questo id: prima senza conferma, il motore la chiede sempre e si
    /// apre il dialogo.
    DropStash(Oid),
    /// Conferma l'eliminazione in attesa: reinvia con `"confirm": true`.
    ConfirmDropStash,
    /// Chiude il dialogo degli stash aperto senza mandare niente.
    CloseStashDialog,
    /// Chiude la finestra dei conflitti di apply o pop.
    DismissStashConflicts,
    /// Unisce `rev` (mostrato come `what`) nel branch attivo; `no_ff`: sempre un commit di
    /// merge.
    Merge {
        rev: String,
        what: String,
        no_ff: bool,
    },
    /// Apre il dialogo "Continua il merge".
    ContinueMerge,
    /// Il messaggio del dialogo "Continua il merge" è cambiato.
    MergeMessage(String),
    /// Conclude il merge con il messaggio del dialogo (`merge.continue`).
    SubmitContinueMerge,
    /// Comincia l'annullamento del merge: il motore risponde con la conferma.
    AbortMerge,
    /// L'utente ha confermato l'annullamento.
    ConfirmAbortMerge,
    /// Cherry-pick o revert del commit `rev` (mostrato come `what`) sul branch attivo; con più
    /// `parents` (commit di merge) si apre prima il dialogo del genitore.
    Pick {
        pick: merge::Pick,
        rev: Oid,
        what: String,
        parents: Vec<Oid>,
    },
    /// Il genitore (da 1) scelto nel dialogo del `mainline`.
    Mainline(usize),
    /// Manda il cherry-pick o il revert con il genitore scelto.
    SubmitMainline,
    /// Chiude il dialogo aperto del merge, del cherry-pick o del revert.
    CloseMergeDialog,
    /// Chiude la finestra di un merge rifiutato.
    DismissMergeError,
    /// Apre l'area di stage e, al centro, l'editor dei conflitti del file.
    OpenConflict(RepoPath),
    /// Chiude l'editor dei conflitti (le scelte non segnate si perdono).
    CloseConflict,
    /// "Ricomincia": rilegge il file in conflitto.
    RestartConflict,
    /// Il risultato dell'editor dei conflitti modificato a mano.
    ConflictText(String),
    /// Sostituisce il blocco di conflitto con questo indice con la scelta.
    ChooseBlock(usize, project_domain::Choice),
    /// Sostituisce tutti i blocchi di conflitto con la stessa scelta.
    ChooseAll(project_domain::Choice),
    /// "Segna come risolto" con il risultato (chiede conferma se restano marcatori).
    ResolveConflict,
    /// Risolve con la versione intera di un lato.
    UseSide(conflict::Pick),
    /// "Elimina il file": apre la conferma.
    DeleteConflicted,
    /// Conferma la risoluzione in attesa (marcatori rimasti o eliminazione).
    ConfirmConflict,
    /// Rinuncia alla risoluzione in attesa di conferma.
    CancelConflict,
}

/// Un passo del trascinamento sulle righe del diff.
#[derive(Debug, Clone, Copy)]
enum Drag {
    Start(usize),
    Over(usize),
    End,
}

/// Risultato di `index.stage`/`index.unstage` (e delle versioni per righe): il nuovo stato e il ref del backup dell'indice,
/// se prima di scrivere ne è stato salvato uno.
#[derive(serde::Deserialize)]
struct WriteResult {
    status: Status,
    backup: Option<String>,
}

/// A cosa serve un comando mandato all'executor. Le letture portano la generazione del repo
/// aperto: se nel frattempo se n'è aperto un altro il risultato si scarta.
#[derive(Debug, Clone, Copy)]
enum Request {
    /// `index.stage`/`index.unstage`/`index.stage_lines`/`index.unstage_lines`: il risultato
    /// è il nuovo stato.
    Write(u64),
    Open,
    Refs(u64),
    Status(u64),
    Log(u64, PageRequest),
    /// `commit.show` del commit indicato.
    Show(u64, Oid),
    /// `diff.commit` con il numero di richiesta di [`DiffState`].
    Diff(u64, u64),
    /// `commit.create`: il risultato è il nuovo commit e lo stato.
    Commit(u64),
    /// `commit.show` di HEAD per proporre il suo messaggio con amend, quando HEAD non è tra
    /// i commit caricati.
    HeadMessage(u64),
    /// `worktree.discard`, senza o con `confirm` (la fase è in [`DiscardBox`]).
    Discard(u64),
    /// `edit.undo`/`edit.redo`.
    Undo(u64),
    /// `edit.history`: lo stato dei pulsanti Annulla e Ripeti.
    History(u64),
    /// `branch.*` (lo stato del dialogo è in [`BranchBox`]).
    Branch(u64, branch::Op),
    /// `remote.fetch` (lo stato è in [`FetchBox`]).
    Fetch(u64),
    /// `remote.pull`/`remote.push` (lo stato è in [`SyncBox`]).
    Sync(u64),
    /// `remote.list`.
    Remotes(u64),
    /// `remote.add`/`remote.remove` (lo stato del dialogo è in [`RemotesBox`]).
    Remote(u64, remotes::Op),
    /// `stash.list`.
    Stashes(u64),
    /// `stash.push`/`stash.apply`/`stash.pop`/`stash.drop` (lo stato è in [`StashBox`]).
    Stash(u64, stash::Op),
    /// `repo.operation`: l'operazione in corso per il riquadro.
    Operation(u64),
    /// `merge.*`, `commit.cherry_pick`, `commit.revert`, `cherry_pick.*`, `revert.*` (lo stato
    /// è in [`MergeBox`]).
    Merge(u64, merge::Op),
    /// `conflict.show` con il numero di lettura di [`ConflictBox`].
    ConflictShow(u64, u64),
    /// `conflict.resolve` (lo stato è in [`ConflictBox`]).
    ConflictResolve(u64),
}

/// Il repo mostrato e i dati letti finora.
struct RepoView {
    path: String,
    head: Head,
    refs: Vec<RefInfo>,
    status: Option<Status>,
    /// Le voci dello stato divise per l'area di stage.
    stage: StageLists,
    /// Vero se è selezionata la riga "Modifiche non committate" invece di un commit.
    worktree_selected: bool,
    /// Porta in vista la riga delle modifiche al prossimo frame.
    scroll_header: bool,
    /// Scrittura in attesa di conferma.
    confirm: Option<PendingWrite>,
    /// La scrittura in corso, descritta (es. "Stage di note.txt").
    write_label: Option<String>,
    /// L'errore dell'ultima scrittura.
    write_error: Option<String>,
    list: CommitList,
    /// Etichette dei ref per commit, rifatte quando cambiano ref o HEAD.
    labels: HashMap<Oid, Vec<RefLabel>>,
    /// Dettagli del commit selezionato (`commit.show`).
    details: DetailsState,
    /// Diff del file aperto (`diff.commit`), mostrato al posto del grafo.
    diff: DiffState,
    /// La casella del messaggio di commit: bozza, commit in corso, conferma, errore.
    commit: CommitBox,
    /// Lo scarto delle modifiche: controllo, conferma, invio.
    discard: DiscardBox,
    /// I comandi sui branch: dialoghi del nome, conferma dell'eliminazione, checkout.
    branch: BranchBox,
    /// Il fetch in corso e l'ultimo errore da spiegare.
    fetch: FetchBox,
    /// Il pull o push in corso e la finestra aperta (conflitto, rifiuto, conferma del force).
    sync: SyncBox,
    /// I remoti configurati (`remote.list`) e i dialoghi per aggiungerli e rimuoverli.
    remotes: RemotesBox,
    /// Gli stash (`stash.list`), lo stash selezionato, i dialoghi di push e drop.
    stash: StashBox,
    /// Il merge avviato, l'operazione in corso (riquadro) e i dialoghi Continua e Annulla.
    merge: MergeBox,
    /// L'editor dei conflitti, al posto del grafo quando è aperto.
    conflict: ConflictBox,
    /// Cresce a ogni stato nuovo: l'operazione in corso si rilegge una volta per stato.
    status_seq: u64,
    /// Cosa farebbero Annulla e Ripeti.
    history: History,
    /// Il commit da selezionare quando arriva la prima pagina del log (dopo un commit, se la
    /// riga delle modifiche è sparita).
    select_after_load: Option<Oid>,
}

impl RepoView {
    fn new(path: String, head: Head) -> Self {
        // All'apertura si seleziona HEAD: con tutti i ref nel grafo può non essere il primo
        // commit (un branch remoto appena scaricato è più recente).
        let select_after_load = head.target();
        let mut view = Self {
            path,
            head,
            refs: Vec::new(),
            status: None,
            stage: StageLists::default(),
            worktree_selected: false,
            scroll_header: false,
            confirm: None,
            write_label: None,
            write_error: None,
            list: CommitList::default(),
            labels: HashMap::new(),
            details: DetailsState::default(),
            diff: DiffState::default(),
            commit: CommitBox::default(),
            discard: DiscardBox::default(),
            branch: BranchBox::default(),
            fetch: FetchBox::default(),
            sync: SyncBox::default(),
            remotes: RemotesBox::default(),
            stash: StashBox::default(),
            merge: MergeBox::default(),
            conflict: ConflictBox::default(),
            status_seq: 0,
            history: History::default(),
            select_after_load,
        };
        view.update_labels();
        view
    }

    fn update_labels(&mut self) {
        self.labels = graph::ref_labels(&self.refs, &self.head);
        let stashes = self.stash.list().unwrap_or_default();
        graph::add_stash_labels(
            &mut self.labels,
            stashes.iter().map(|s| (s.base, s.name.as_str())),
        );
    }

    /// Nuovo stato del repo, da `repo.status` o da una scrittura. Senza modifiche la riga
    /// delle modifiche sparisce: se era selezionata si torna al commit.
    fn set_status(&mut self, status: Status) {
        self.head = status.head.clone();
        self.stage = StageLists::new(&status);
        self.conflict.follow_status(&status);
        self.status_seq += 1;
        self.status = Some(status);
        if self.stage.is_empty() {
            self.worktree_selected = false;
            self.confirm = None;
            self.discard.cancel();
        }
        self.update_labels();
    }

    /// Vero se è aperta una conferma (stage di conflitti, amend pubblicato, scarto) o un
    /// dialogo dei branch: finché c'è, niente scritture e i tasti vanno solo a lei.
    fn modal_open(&self) -> bool {
        self.confirm.is_some()
            || self.commit.confirm().is_some()
            || self.discard.confirm().is_some()
            || self.branch.modal_open()
            || self.sync.modal_open()
            || self.remotes.modal_open()
            || self.stash.modal_open()
            || self.merge.modal_open()
    }

    /// L'ultimo commit (HEAD), se è tra quelli caricati.
    fn head_commit(&self) -> Option<&Commit> {
        let head = self.head.target()?;
        self.list.commits().iter().find(|c| c.id == head)
    }

    /// Lo stato del pulsante "Commit".
    fn commit_gate(&self, writing: bool) -> Gate<'_> {
        Gate {
            draft: &self.commit.draft,
            status: self.status.as_ref(),
            staged: self.stage.staged.len(),
            has_head: self.head.target().is_some(),
            busy: writing || self.modal_open(),
        }
    }

    /// Vero se c'è la riga "Modifiche non committate" in cima alla lista.
    fn has_changes(&self) -> bool {
        !self.stage.is_empty()
    }

    /// Seleziona la riga delle modifiche (`true`) o torna ai commit.
    fn select_worktree(&mut self, on: bool) {
        self.worktree_selected = on && self.has_changes();
        if self.worktree_selected {
            self.stash.select(None);
        }
        if self.worktree_selected {
            self.scroll_header = true;
        }
    }

    /// Frecce e Pagina su/giù: la riga delle modifiche sta sopra il primo commit.
    fn move_selection(&mut self, delta: isize) {
        if self.worktree_selected {
            if delta > 0 && !self.list.commits().is_empty() {
                self.worktree_selected = false;
                self.list.select(0);
                self.list.move_selection(delta - 1);
            }
            return;
        }
        let above_top = self
            .list
            .selected()
            .is_none_or(|i| i.checked_add_signed(delta).is_none());
        if delta < 0 && above_top && self.has_changes() {
            self.select_worktree(true);
        } else {
            self.list.move_selection(delta);
        }
    }
}

/// Dove era la lista dei commit nell'ultimo frame: serve per portare in vista la selezione e
/// per sapere quante righe salta Pagina giù.
#[derive(Debug, Default)]
struct ListViewport {
    /// Righe disegnate, anche solo in parte.
    rows: std::ops::Range<usize>,
    height: f32,
}

#[derive(Deserialize)]
struct Opened {
    path: String,
    head: Head,
}

#[derive(Deserialize)]
struct LogPage {
    commits: Vec<Commit>,
    has_more: bool,
}

pub struct App {
    executor: Executor,
    platform: Arc<dyn Platform>,
    /// Cartella scelta nel dialogo; sul desktop arriva subito, in generale da un callback.
    picked: Arc<Mutex<Option<String>>>,
    pending: HashMap<Ticket, Request>,
    /// Cresce a ogni repo aperto.
    generation: u64,
    repo: Option<RepoView>,
    viewport: ListViewport,
    error: String,
    /// Messaggio informativo nella barra di stato (es. il backup dell'indice salvato).
    notice: String,
}

impl App {
    /// `opener` apre i repository: `CliOpener` sul desktop, `MemoryOpener` sul web. Se c'è,
    /// `repo` si apre subito, in background.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        platform: Arc<dyn Platform>,
        opener: Arc<dyn RepoOpener>,
        repo: Option<String>,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        let wake: Wake = Arc::new(move || ctx.request_repaint());
        let mut app = Self {
            executor: Executor::new(
                Session::new(opener),
                Registry::builtin(),
                Arc::clone(&platform),
                wake,
            ),
            platform,
            picked: Arc::default(),
            pending: HashMap::new(),
            generation: 0,
            repo: None,
            viewport: ListViewport::default(),
            error: String::new(),
            notice: String::new(),
        };
        if let Some(path) = repo {
            app.apply(&cc.egui_ctx, Action::Open(path));
        }
        app
    }

    fn submit(&mut self, command: &str, params: Value, request: Request) {
        let ticket = self.executor.submit(command, params);
        self.pending.insert(ticket, request);
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::PickFolder => {
                let picked = Arc::clone(&self.picked);
                let ctx = ctx.clone();
                self.platform.pick_folder(Box::new(move |folder| {
                    if let Some(folder) = folder {
                        *picked.lock().unwrap_or_else(|e| e.into_inner()) = Some(folder);
                        ctx.request_repaint();
                    }
                }));
            }
            Action::Open(path) => self.submit("repo.open", json!({ "path": path }), Request::Open),
            // Durante una scrittura (un fetch può durare) le letture aspetterebbero in coda.
            Action::Refresh => {
                if self.executor.writing().is_none() {
                    self.load();
                }
            }
            Action::Select(index) => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.select(None);
                    repo.select_worktree(false);
                    repo.list.select(index);
                }
            }
            Action::SelectWorktree => {
                if let Some(repo) = &mut self.repo {
                    repo.select_worktree(true);
                }
            }
            Action::SelectCommit(id) => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.select(None);
                }
                self.seek(id);
            }
            Action::Move(delta) => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.select(None);
                    repo.move_selection(delta);
                }
            }
            Action::Edge(first) => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.select(None);
                    if first && repo.has_changes() {
                        repo.select_worktree(true);
                    } else {
                        repo.select_worktree(false);
                        repo.list.select_edge(first);
                    }
                }
            }
            Action::RetryDetails => {
                if let Some(repo) = &mut self.repo {
                    repo.details.retry();
                }
            }
            Action::OpenDiff(target) => {
                if let Some(repo) = &mut self.repo {
                    repo.conflict.close();
                }
                if let Some(seq) = self.repo.as_mut().and_then(|r| r.diff.open(target.clone())) {
                    self.request_diff(&target, seq);
                }
            }
            Action::CloseDiff => {
                if let Some(repo) = &mut self.repo {
                    repo.diff.close();
                }
            }
            Action::RetryDiff => {
                let repo = self.repo.as_mut();
                if let Some((seq, target)) = repo.and_then(|r| {
                    let seq = r.diff.retry()?;
                    Some((seq, r.diff.target()?.clone()))
                }) {
                    self.request_diff(&target, seq);
                }
            }
            Action::Visible(end) => {
                if let Some(request) = self.repo.as_mut().and_then(|r| r.list.next_page(end)) {
                    self.request_page(request);
                }
            }
            Action::Write(..)
            | Action::ConfirmWrite
            | Action::CancelWrite
            | Action::DismissWriteError => self.apply_stage(action),
            Action::Discard(..)
            | Action::ConfirmDiscard
            | Action::CancelDiscard
            | Action::Undo(..) => self.apply_discard(action),
            Action::SelectLine { .. }
            | Action::DragLines(..)
            | Action::ClearLines
            | Action::WriteHunk(..)
            | Action::WriteSelectedLines => self.apply_lines(action),
            Action::SetAmend(..)
            | Action::Commit
            | Action::ConfirmCommit
            | Action::CancelCommit
            | Action::DismissCommitError => self.apply_commit(action),
            Action::NewBranch(..)
            | Action::RenameBranch(..)
            | Action::BranchName(..)
            | Action::SubmitBranchName
            | Action::Checkout(..)
            | Action::DeleteBranch(..)
            | Action::ConfirmDeleteBranch
            | Action::CloseBranchDialog => self.apply_branch(action),
            Action::Fetch(..) | Action::CancelFetch | Action::DismissFetchError => {
                self.apply_fetch(action)
            }
            Action::Pull(..)
            | Action::Push(..)
            | Action::ForcePush
            | Action::ConfirmForcePush
            | Action::CancelSync
            | Action::DismissSync => self.apply_sync(action),
            Action::AddRemote
            | Action::AddRemoteFields(..)
            | Action::HideRemoteUrl(..)
            | Action::SubmitAddRemote
            | Action::CancelAddRemote
            | Action::RemoveRemote(..)
            | Action::ConfirmRemoveRemote
            | Action::CloseRemoteDialog
            | Action::DismissRemoteError => self.apply_remotes(action),
            Action::OpenStash
            | Action::StashFields(..)
            | Action::SubmitStash
            | Action::SelectStash(..)
            | Action::ApplyStash(..)
            | Action::StashRestoreIndex(..)
            | Action::DropStash(..)
            | Action::ConfirmDropStash
            | Action::CloseStashDialog
            | Action::DismissStashConflicts => self.apply_stash(action),
            Action::Merge { .. }
            | Action::ContinueMerge
            | Action::MergeMessage(..)
            | Action::SubmitContinueMerge
            | Action::AbortMerge
            | Action::ConfirmAbortMerge
            | Action::Pick { .. }
            | Action::Mainline(..)
            | Action::SubmitMainline
            | Action::CloseMergeDialog
            | Action::DismissMergeError => self.apply_merge(action),
            Action::OpenConflict(..)
            | Action::CloseConflict
            | Action::RestartConflict
            | Action::ConflictText(..)
            | Action::ChooseBlock(..)
            | Action::ChooseAll(..)
            | Action::ResolveConflict
            | Action::UseSide(..)
            | Action::DeleteConflicted
            | Action::ConfirmConflict
            | Action::CancelConflict => self.apply_conflict(action),
        }
    }

    fn request_page(&mut self, page: PageRequest) {
        self.submit(
            "repo.log",
            page.params(),
            Request::Log(self.generation, page),
        );
    }

    /// Seleziona il commit di un ref; se non è ancora caricato chiede le pagine seguenti
    /// finché non arriva (fino a [`list::SEEK_LIMIT`] commit).
    fn seek(&mut self, id: Oid) {
        let Some(repo) = &mut self.repo else {
            return;
        };
        match repo.list.seek(id) {
            Seek::Selected => repo.select_worktree(false),
            Seek::Loading(Some(page)) => self.request_page(page),
            Seek::Loading(None) => {}
            Seek::Missing => {
                self.error = format!("il commit {} non è tra quelli caricati", id.short());
            }
        }
    }

    fn request_diff(&mut self, target: &DiffTarget, seq: u64) {
        let (command, params) = target.request();
        self.submit(command, params, Request::Diff(self.generation, seq));
    }

    /// Il diff caricato con la sua selezione, se permette la selezione parziale.
    fn selectable_diff(&mut self) -> Option<(&DiffTarget, &DiffDoc, &mut lines::LineSelection)> {
        let (target, doc, selection) = self.repo.as_mut()?.diff.loaded_mut()?;
        matches!(
            lines::partial(doc, target.source),
            lines::Partial::Available(_)
        )
        .then_some((target, doc, selection))
    }

    /// Dopo un nuovo stato, il diff aperto di worktree o indice si richiede; se il file è
    /// passato tutto nell'altra lista si apre quel diff, se non ha più modifiche si chiude.
    fn follow_diff(&mut self) {
        let Some(repo) = &mut self.repo else {
            return;
        };
        let Some(target) = repo.diff.target().cloned() else {
            return;
        };
        let side = match target.source {
            DiffSource::Worktree => Side::Unstaged,
            DiffSource::Index => Side::Staged,
            DiffSource::Commit(_) => return,
        };
        let request = match stage::follow(&repo.stage, side, &target.path) {
            Follow::Reload => repo.diff.reload().map(|seq| (seq, target)),
            Follow::Switch(other) => {
                let target = DiffTarget {
                    source: side_source(other),
                    path: target.path,
                };
                repo.diff.open(target.clone()).map(|seq| (seq, target))
            }
            Follow::Close => {
                repo.diff.close();
                None
            }
        };
        if let Some((seq, target)) = request {
            self.request_diff(&target, seq);
        }
    }

    /// Chiude il diff se non è più di ciò che è selezionato: il diff di un commit vale finché
    /// è selezionato quel commit, quelli di worktree e indice finché è selezionata la riga
    /// delle modifiche.
    fn close_stale_diff(&mut self) {
        if let Some(repo) = &mut self.repo {
            let selected = match repo.stash.selected() {
                Some(stash) => Some(stash.id),
                None => repo.list.selected_commit().map(|c| c.id),
            };
            let stale = repo.diff.target().is_some_and(|t| match t.source {
                DiffSource::Commit(id) => repo.worktree_selected || Some(id) != selected,
                DiffSource::Worktree | DiffSource::Index => !repo.worktree_selected,
            });
            if stale {
                repo.diff.close();
            }
            // L'editor dei conflitti vale finché è selezionata la riga delle modifiche.
            if !repo.worktree_selected {
                repo.conflict.close();
            }
        }
    }

    /// Chiede i dettagli del commit selezionato, se è cambiato e non ci sono già.
    fn request_details(&mut self) {
        let Some(repo) = &mut self.repo else {
            return;
        };
        // Uno stash selezionato si mostra come il suo commit: i file rispetto alla base.
        let selected = match repo.stash.selected() {
            Some(stash) => Some(stash.id),
            None => repo.list.selected_commit().map(|c| c.id),
        }
        .filter(|_| !repo.worktree_selected);
        if let Some(id) = repo.details.want(selected) {
            let request = Request::Show(self.generation, id);
            self.submit("commit.show", json!({ "id": id.to_string() }), request);
        }
    }

    /// Azioni dei tasti della lista: frecce, Pagina su/giù, Inizio/Fine. Con il diff aperto,
    /// Esc lo chiude e le frecce restano alla vista del diff. Solo se nessun campo di testo sta
    /// ricevendo la tastiera.
    fn keyboard(&self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(repo) = &self.repo else {
            return;
        };
        // Con la conferma aperta i tasti vanno solo a lei.
        if ctx.egui_wants_keyboard_input() || repo.modal_open() {
            return;
        }
        // Con l'editor dei conflitti aperto la lista non è in vista.
        if repo.conflict.editor().is_some() {
            return;
        }
        if repo.diff.target().is_some() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                // Prima si deseleziona, poi si chiude.
                if repo.diff.selection().is_some_and(|s| !s.is_empty()) {
                    actions.push(Action::ClearLines);
                } else {
                    actions.push(Action::CloseDiff);
                }
            }
            return;
        }
        let page = isize::try_from(self.viewport.rows.len().saturating_sub(2).max(1))
            .unwrap_or(isize::MAX);
        ctx.input(|i| {
            let keys = [
                (egui::Key::ArrowDown, Action::Move(1)),
                (egui::Key::ArrowUp, Action::Move(-1)),
                (egui::Key::PageDown, Action::Move(page)),
                (egui::Key::PageUp, Action::Move(-page)),
                (egui::Key::Home, Action::Edge(true)),
                (egui::Key::End, Action::Edge(false)),
            ];
            for (key, action) in keys {
                for _ in 0..i.num_presses(key) {
                    actions.push(action.clone());
                }
            }
        });
    }

    /// Chiede ref, stato e log del repo aperto.
    fn load(&mut self) {
        if self.repo.is_none() {
            return;
        }
        let generation = self.generation;
        self.submit("repo.refs", json!({}), Request::Refs(generation));
        self.submit("repo.status", json!({}), Request::Status(generation));
        self.submit("edit.history", json!({}), Request::History(generation));
        self.request_remotes(generation);
        self.submit(stash::LIST, json!({}), Request::Stashes(generation));
        if let Some(repo) = &mut self.repo {
            let first = repo.list.reset();
            self.request_page(first);
        }
    }

    /// Chiede `repo.operation` se lo stato arrivato dice che c'è un'operazione in corso e per
    /// quello stato non si è ancora chiesto (uno alla volta).
    fn request_operation(&mut self) {
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        let Some(operation) = repo.status.as_ref().map(|s| s.operation) else {
            return;
        };
        if repo.merge.want_info(operation, repo.status_seq) {
            self.submit(merge::OPERATION, json!({}), Request::Operation(generation));
        }
    }

    /// Rilegge l'elenco dei remoti (quanti branch remoti, quale è il predefinito).
    fn request_remotes(&mut self, generation: u64) {
        self.submit(remotes::LIST, json!({}), Request::Remotes(generation));
    }

    fn receive(&mut self, ctx: &egui::Context) {
        let folder = self.picked.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(folder) = folder {
            self.apply(ctx, Action::Open(folder));
        }
        for done in self.executor.poll() {
            let Some(request) = self.pending.remove(&done.ticket) else {
                continue;
            };
            // I dettagli hanno il loro stato, errori compresi: li mostra il pannello.
            if let Request::Show(generation, id) = request {
                let result = done.result.and_then(|value| {
                    serde_json::from_value::<CommitDetails>(value)
                        .map_err(|e| format!("{}: risultato inatteso: {e}", done.command))
                });
                if let Some(repo) = self.current(generation) {
                    repo.details.receive(id, result);
                }
                continue;
            }
            // Le scritture: il nuovo stato, oppure l'errore nell'area di stage.
            if let Request::Write(generation) = request {
                let result = done.result.and_then(|value| {
                    serde_json::from_value::<WriteResult>(value)
                        .map_err(|e| format!("{}: risultato inatteso: {e}", done.command))
                });
                let Some(repo) = self.current(generation) else {
                    continue;
                };
                let label = repo
                    .write_label
                    .take()
                    .unwrap_or_else(|| done.command.clone());
                match result {
                    Ok(WriteResult { status, backup }) => {
                        repo.set_status(status);
                        self.error.clear();
                        // Versioni in stage che esistevano solo nell'indice: salvate prima di
                        // scrivere, lo si dice nella barra di stato.
                        self.notice = backup
                            .map(|r| format!("Backup dell'indice salvato in {r}"))
                            .unwrap_or_default();
                        self.follow_diff();
                    }
                    Err(e) => {
                        log::warn!("{}: {e}", done.command);
                        let stale = done.error_kind == Some(ErrorKind::StaleDiff);
                        let message = if stale {
                            format!("{label} non riuscito: {}", lines::STALE_MESSAGE)
                        } else {
                            format!("{label} non riuscito: {e}")
                        };
                        repo.write_error = Some(message.clone());
                        // Il diff visto non è più quello attuale: si rileggono stato e diff
                        // (il diff segue lo stato) e la selezione si azzera.
                        if stale && let Some((_, _, selection)) = repo.diff.loaded_mut() {
                            selection.clear();
                        }
                        self.error = message;
                        self.notice.clear();
                        if stale {
                            self.submit("repo.status", json!({}), Request::Status(generation));
                        }
                    }
                }
                continue;
            }
            if let Request::Commit(generation) = request {
                self.receive_commit(generation, done);
                continue;
            }
            if let Request::Discard(generation) = request {
                self.receive_discard(generation, done);
                continue;
            }
            if let Request::Undo(generation) = request {
                self.receive_undo(generation, done);
                continue;
            }
            if let Request::Branch(generation, op) = request {
                self.receive_branch(generation, op, done);
                continue;
            }
            if let Request::Fetch(generation) = request {
                self.receive_fetch(generation, done);
                continue;
            }
            if let Request::Sync(generation) = request {
                self.receive_sync(generation, done);
                continue;
            }
            // L'elenco dei remoti ha il suo errore, mostrato nella sua sezione.
            if let Request::Remotes(generation) = request {
                if let Some(repo) = self.current(generation) {
                    if let Err(e) = &done.result {
                        log::warn!("{}: {e}", done.command);
                    }
                    repo.remotes.receive_list(done.result);
                }
                continue;
            }
            if let Request::Remote(generation, op) = request {
                self.receive_remote(generation, op, done);
                continue;
            }
            // Anche l'elenco degli stash ha il suo errore, nella sua sezione.
            if let Request::Stashes(generation) = request {
                if let Some(repo) = self.current(generation) {
                    if let Err(e) = &done.result {
                        log::warn!("{}: {e}", done.command);
                    }
                    repo.stash.receive_list(done.result);
                    repo.update_labels();
                }
                continue;
            }
            if let Request::Stash(generation, op) = request {
                self.receive_stash(generation, op, done);
                continue;
            }
            // Anche l'operazione in corso ha il suo errore, nel riquadro.
            if let Request::Operation(generation) = request {
                if let Some(repo) = self.current(generation) {
                    if let Err(e) = &done.result {
                        log::warn!("{}: {e}", done.command);
                    }
                    repo.merge.receive_info(done.result);
                }
                continue;
            }
            if let Request::Merge(generation, op) = request {
                self.receive_merge(generation, op, done);
                continue;
            }
            // L'editor dei conflitti mostra da sé gli errori di lettura.
            if let Request::ConflictShow(generation, seq) = request {
                if let Some(repo) = self.current(generation) {
                    if let Err(e) = &done.result {
                        log::warn!("{}: {e}", done.command);
                    }
                    repo.conflict.receive_show(seq, done.result);
                }
                continue;
            }
            if let Request::ConflictResolve(generation) = request {
                self.receive_conflict(generation, done);
                continue;
            }
            // Anche il diff: errori nella sua vista, risposte vecchie scartate.
            if let Request::Diff(generation, seq) = request {
                let result = done.result.and_then(|value| {
                    serde_json::from_value::<Diff>(value)
                        .map_err(|e| format!("{}: risultato inatteso: {e}", done.command))
                });
                if let Some(repo) = self.current(generation) {
                    repo.diff.receive(seq, result);
                }
                continue;
            }
            // Una pagina non arrivata si potrà chiedere di nuovo.
            if done.result.is_err()
                && let Request::Log(generation, page) = request
                && let Some(repo) = self.current(generation)
            {
                repo.list.failed(page);
            }
            match done.result {
                Ok(value) => {
                    if let Err(e) = self.store(request, value) {
                        log::error!("{}: risultato inatteso: {e}", done.command);
                        self.error = format!("{}: risultato inatteso: {e}", done.command);
                    } else {
                        self.error.clear();
                    }
                }
                Err(e) => {
                    log::warn!("{}: {e}", done.command);
                    self.error = e;
                }
            }
        }
    }

    /// Il repo mostrato, se è ancora quello della generazione `generation`.
    fn current(&mut self, generation: u64) -> Option<&mut RepoView> {
        self.repo.as_mut().filter(|_| generation == self.generation)
    }

    /// Mette il risultato di `request` nella vista.
    fn store(&mut self, request: Request, value: Value) -> Result<(), serde_json::Error> {
        match request {
            Request::Open => {
                let Opened { path, head } = serde_json::from_value(value)?;
                self.generation += 1;
                self.notice.clear();
                self.repo = Some(RepoView::new(path, head));
                self.load();
            }
            Request::Refs(generation) => {
                if let Some(repo) = self.current(generation) {
                    repo.refs = serde_json::from_value(value)?;
                    repo.update_labels();
                }
            }
            Request::Status(generation) => {
                if let Some(repo) = self.current(generation) {
                    let status: Status = serde_json::from_value(value)?;
                    repo.set_status(status);
                    self.follow_diff();
                }
            }
            Request::Log(generation, page) => {
                if let Some(repo) = self.current(generation) {
                    let LogPage { commits, has_more } = serde_json::from_value(value)?;
                    if !repo.list.receive(page, commits, has_more) {
                        return Ok(());
                    }
                    if page.skip == 0
                        && let Some(id) = repo.select_after_load.take()
                        && !repo.worktree_selected
                    {
                        repo.list.select_id(id);
                    }
                    // Un ref cliccato il cui commit non era ancora caricato: si guarda nella
                    // pagina arrivata e, se non c'è, si chiede la seguente.
                    if let Some(id) = repo.list.seeking() {
                        self.seek(id);
                    }
                }
            }
            Request::HeadMessage(generation) => {
                if let Some(repo) = self.current(generation) {
                    let details: CommitDetails = serde_json::from_value(value)?;
                    repo.commit.draft.propose(&details.commit);
                }
            }
            Request::History(generation) => {
                if let Some(repo) = self.current(generation) {
                    repo.history = serde_json::from_value(value)?;
                }
            }
            // Gestiti a parte in `receive`.
            Request::Show(..)
            | Request::Diff(..)
            | Request::Write(..)
            | Request::Commit(..)
            | Request::Discard(..)
            | Request::Undo(..)
            | Request::Branch(..)
            | Request::Fetch(..)
            | Request::Sync(..)
            | Request::Remotes(..)
            | Request::Remote(..)
            | Request::Stashes(..)
            | Request::Stash(..)
            | Request::Operation(..)
            | Request::Merge(..)
            | Request::ConflictShow(..)
            | Request::ConflictResolve(..) => {}
        }
        Ok(())
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.receive(&ctx);
        self.request_operation();
        let mut actions = Vec::new();
        self.keyboard(&ctx, &mut actions);
        let can_pick = self.platform.capabilities().open_folder;
        let writing = self.executor.writing().is_some();
        // La bozza del messaggio si scrive direttamente (stato della UI, non del repo): si
        // presta al pannello per questo frame e poi si rimette a posto.
        let mut draft = self
            .repo
            .as_mut()
            .map(|r| std::mem::take(&mut r.commit.draft))
            .unwrap_or_default();

        // Il fetch, pull o push in corso con il suo ultimo avanzamento, per la barra.
        let network = self.repo.as_ref().and_then(|r| {
            if let Some(running) = r.fetch.running() {
                return Some(view::Network {
                    what: fetch::label(running.remote.as_deref()),
                    cancelling: running.cancelling,
                    progress: self.executor.progress(running.ticket),
                    stop_hint: "Ferma il fetch: i branch remoti già aggiornati restano, i branch \
                                locali e i file non sono toccati",
                    cancel: Action::CancelFetch,
                });
            }
            if let Some(adding) = r.remotes.running_fetch() {
                return Some(view::Network {
                    what: adding.label(),
                    cancelling: adding.cancelling,
                    progress: adding.ticket.and_then(|t| self.executor.progress(t)),
                    stop_hint: "Ferma il fetch: il remoto resta aggiunto, i branch remoti già \
                                scaricati restano",
                    cancel: Action::CancelAddRemote,
                });
            }
            let running = r.sync.running()?;
            Some(view::Network {
                what: running.request.label(),
                cancelling: running.cancelling,
                progress: self.executor.progress(running.ticket),
                stop_hint: match running.request.op {
                    sync::Op::Pull => {
                        "Ferma il pull durante il fetch: il branch non viene toccato. Se \
                         l'integrazione è già partita finisce comunque"
                    }
                    sync::Op::Push => {
                        "Ferma il push: se l'invio era già finito il remoto potrebbe essere \
                         aggiornato (un Fetch lo mostra)"
                    }
                },
                cancel: Action::CancelSync,
            })
        });
        egui::Panel::top("barra").show(ui, |ui| {
            view::top_bar(
                ui,
                self.repo.as_ref(),
                can_pick,
                writing,
                network,
                &mut actions,
            );
        });
        egui::Panel::bottom("stato").show(ui, |ui| {
            let write = self.repo.as_ref().and_then(|r| r.write_label.as_deref());
            view::status_bar(
                ui,
                self.executor.running(),
                write,
                &self.error,
                &self.notice,
            );
        });
        // L'operazione in corso (merge, rebase, ...) sotto la barra, per tutta la larghezza.
        if let Some(repo) = &self.repo
            && let Some(operation) = repo.status.as_ref().and_then(|s| s.operation)
        {
            let can_write = !writing && !repo.modal_open() && !repo.merge.busy();
            egui::Panel::top("operazione").show(ui, |ui| {
                merge_view::banner(ui, repo, operation, can_write, &mut actions);
            });
        }
        // I pannelli laterali non superano un terzo della finestra: il contenuto va a capo.
        let side_max = (ui.available_width() / 3.0).max(200.0);
        match &self.repo {
            Some(repo) => {
                egui::Panel::left("ref")
                    .resizable(true)
                    .default_size(220.0)
                    .min_size(140.0)
                    .max_size(side_max)
                    .show(ui, |ui| view::refs_panel(ui, repo, writing, &mut actions));
                egui::Panel::right("dettagli")
                    .resizable(true)
                    .default_size(320.0)
                    .min_size(200.0)
                    .max_size(side_max)
                    .show(ui, |ui| {
                        if repo.worktree_selected {
                            view::stage_panel(ui, repo, &mut draft, writing, &mut actions);
                        } else if let Some(stash) = repo.stash.selected() {
                            view::stash_panel(ui, repo, stash, writing, &mut actions);
                        } else {
                            view::details_panel(
                                ui,
                                repo.list.selected_commit(),
                                repo.details.shown(),
                                repo.diff.target(),
                                &mut actions,
                            );
                        }
                    });
                if let Some(write) = &repo.confirm {
                    view::confirm_write(&ctx, write, &mut actions);
                }
                if let Some(pending) = repo.discard.confirm() {
                    view::confirm_discard(&ctx, pending, &mut actions);
                }
                if let Some(failure) = repo.fetch.failure() {
                    view::failure_window(
                        &ctx,
                        "errore-fetch",
                        failure,
                        Action::DismissFetchError,
                        &mut actions,
                    );
                }
                if let Some(failure) = repo.remotes.failure() {
                    view::failure_window(
                        &ctx,
                        "errore-remoto",
                        failure,
                        Action::DismissRemoteError,
                        &mut actions,
                    );
                }
                if let Some(dialog) = repo.remotes.dialog() {
                    let progress = repo
                        .remotes
                        .running_fetch()
                        .and_then(|d| d.ticket)
                        .and_then(|t| self.executor.progress(t));
                    view::remote_dialog(&ctx, dialog, progress.as_ref(), &mut actions);
                }
                if let Some(dialog) = repo.stash.dialog() {
                    view::stash_dialog(&ctx, repo, dialog, &mut actions);
                }
                if let Some(conflicts) = repo.stash.conflicts() {
                    view::stash_conflicts_window(&ctx, conflicts, &mut actions);
                }
                if let Some(window) = repo.sync.window() {
                    let can_write = !writing && !repo.modal_open();
                    view::sync_window(&ctx, window, writing, can_write, &mut actions);
                }
                if let Some(dialog) = repo.branch.dialog() {
                    view::branch_dialog(&ctx, dialog, &mut actions);
                }
                if let Some(dialog) = repo.merge.dialog() {
                    merge_view::merge_dialog(&ctx, repo, dialog, &mut actions);
                }
                if let Some(failure) = repo.merge.failure() {
                    view::failure_window(
                        &ctx,
                        "errore-merge",
                        failure,
                        Action::DismissMergeError,
                        &mut actions,
                    );
                }
                if let Some(confirm) = repo.commit.confirm() {
                    let branch = match &repo.head {
                        Head::Branch { name, .. } => Some(format!("refs/heads/{name}")),
                        Head::Detached(_) => None,
                    };
                    let left_behind =
                        commit::refs_left_behind(&repo.refs, repo.head.target(), branch.as_deref());
                    view::confirm_amend(&ctx, confirm, &left_behind, &mut actions);
                }
                let viewport = &mut self.viewport;
                let incoming = repo
                    .merge
                    .info()
                    .and_then(|i| i.incoming.first())
                    .map(|i| i.label.as_str());
                let conflict_write = !writing && !repo.modal_open();
                egui::CentralPanel::default().show(ui, |ui| match repo.diff.pane() {
                    _ if repo.conflict.editor().is_some() => {
                        if let Some(editor) = repo.conflict.editor() {
                            view::conflict_editor(
                                ui,
                                editor,
                                incoming,
                                conflict_write,
                                &mut actions,
                            );
                        }
                    }
                    Pane::Closed => view::commit_list(ui, repo, viewport, writing, &mut actions),
                    pane => {
                        let lines = diff_view::LinesUi {
                            selection: repo.diff.selection(),
                            can_write: !writing && !repo.modal_open(),
                        };
                        diff_view::diff_panel(ui, pane, lines, &mut actions);
                    }
                });
            }
            None => {
                egui::CentralPanel::default().show(ui, |ui| {
                    view::welcome(ui, can_pick, self.executor.is_busy(), &mut actions);
                });
            }
        }

        // La lista ha portato in vista la riga chiesta: non serve più.
        if let Some(repo) = &mut self.repo {
            repo.commit.draft = draft;
            repo.list.take_scroll_to();
            repo.scroll_header = false;
        }
        for action in actions {
            self.apply(&ctx, action);
        }
        // Dopo le azioni: la selezione appena cambiata si chiede in questo stesso frame.
        self.close_stale_diff();
        self.request_details();
    }
}

/// Il diff di un file della lista `side`.
fn side_source(side: Side) -> DiffSource {
    match side {
        Side::Staged => DiffSource::Index,
        Side::Unstaged => DiffSource::Worktree,
    }
}

/// `testo` con la prima lettera minuscola, per metterlo dentro una frase.
fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}
