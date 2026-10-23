//! Scartare le modifiche fuori stage (`worktree.discard`) e il diario per Annulla/Ripeti
//! (`edit.undo`/`edit.redo`). Solo dati, senza egui: si prova con test unitari.
//!
//! Lo scarto va in due tempi: prima `worktree.discard` senza `confirm`, che non cambia niente e
//! risponde con `ConfirmRequired` (o rifiuta subito con `NotAllowed`, es. file in conflitto nel
//! repo); poi, solo se l'utente conferma nel dialogo, lo stesso comando con `"confirm": true`.

use crate::ErrorKind;
use crate::stage::StageItem;
use crate::{Action, App, Done, Request, discard};
use project_domain::{Change, RepoPath, Status};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

/// Vero se le modifiche del file si possono scartare: tracciato, non in conflitto. I file non
/// tracciati non si toccano mai (scartarli vorrebbe dire cancellarli, senza backup).
pub fn discardable(item: &StageItem) -> bool {
    !item.is_conflict() && !matches!(item.change, Change::Untracked | Change::Unmerged)
}

/// Perché le modifiche di `item` non si scartano, per il menu; `None` se si scartano.
pub fn not_discardable_reason(item: &StageItem) -> Option<String> {
    if item.is_conflict() || item.change == Change::Unmerged {
        Some(tr!("discard-reason-conflict"))
    } else if item.change == Change::Untracked {
        Some(tr!("discard-reason-untracked"))
    } else {
        None
    }
}

/// I percorsi da scartare con "Scarta tutto": i soli file tracciati e senza conflitti della
/// lista, sempre espliciti; `None` se non ce n'è nessuno.
pub fn discardable_paths(items: &[StageItem]) -> Option<Vec<RepoPath>> {
    let mut paths: Vec<RepoPath> = Vec::new();
    for item in items.iter().filter(|item| discardable(item)) {
        if !paths.contains(&item.path) {
            paths.push(item.path.clone());
        }
    }
    (!paths.is_empty()).then_some(paths)
}

/// L'id del comando del Registry.
pub const COMMAND: &str = "worktree.discard";

/// Nome dell'operazione per i messaggi, es. "Scarto di README.md".
pub fn describe(paths: &[RepoPath]) -> String {
    match paths {
        [one] => tr!(
            "discard-label-one",
            path = one.to_string_lossy().into_owned()
        ),
        many => tr!("discard-label-many", n = many.len()),
    }
}

/// Il controllo prima dello scarto, es. "Controllo per scarto delle modifiche di README.md".
pub fn describe_check(paths: &[RepoPath]) -> String {
    match paths {
        [one] => tr!(
            "discard-check-label-one",
            path = one.to_string_lossy().into_owned()
        ),
        many => tr!("discard-check-label-many", n = many.len()),
    }
}

/// I parametri di `worktree.discard`; `confirm` solo dopo la conferma dell'utente.
pub fn params(paths: &[RepoPath], confirm: bool) -> Value {
    let mut params = json!({ "paths": paths });
    if confirm {
        params["confirm"] = json!(true);
    }
    params
}

/// A che punto è lo scarto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Mandato senza `confirm`: il motore controlla e risponde con la richiesta di conferma.
    Checking,
    /// Il dialogo di conferma è aperto; `reason` è il motivo del motore.
    Confirm { reason: String },
    /// Mandato con `"confirm": true`.
    Sending,
}

/// Uno scarto in corso o da confermare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discard {
    pub paths: Vec<RepoPath>,
    pub phase: Phase,
}

/// Lo scarto delle modifiche, dal controllo alla risposta. Uno solo alla volta.
#[derive(Debug, Default)]
pub struct DiscardBox {
    current: Option<Discard>,
}

/// Cosa è arrivato da `worktree.discard`.
#[derive(Debug)]
pub enum Received {
    /// Serve la conferma: il dialogo si apre.
    Confirm,
    /// Le modifiche sono scartate.
    Done(Box<DiscardResult>),
    /// Rifiutato o non riuscito: il messaggio da mostrare. Niente è cambiato, tranne se il
    /// risultato non si legge (allora lo scarto potrebbe essere fatto: si rilegge lo stato).
    Failed { message: String, reload: bool },
}

impl DiscardBox {
    #[cfg(test)]
    pub fn current(&self) -> Option<&Discard> {
        self.current.as_ref()
    }

    /// Lo scarto in attesa di conferma, se il dialogo è aperto.
    pub fn confirm(&self) -> Option<&Discard> {
        self.current
            .as_ref()
            .filter(|d| matches!(d.phase, Phase::Confirm { .. }))
    }

    /// Comincia uno scarto: i parametri del controllo (senza `confirm`). `None` se ce n'è già
    /// uno o se non c'è niente da scartare.
    pub fn start(&mut self, paths: Vec<RepoPath>) -> Option<Value> {
        if self.current.is_some() || paths.is_empty() {
            return None;
        }
        let params = params(&paths, false);
        self.current = Some(Discard {
            paths,
            phase: Phase::Checking,
        });
        Some(params)
    }

    /// L'utente ha confermato: i parametri con `"confirm": true`.
    pub fn confirmed(&mut self) -> Option<Value> {
        let discard = self.current.as_mut()?;
        if !matches!(discard.phase, Phase::Confirm { .. }) {
            return None;
        }
        discard.phase = Phase::Sending;
        Some(params(&discard.paths, true))
    }

    /// Chiude il dialogo senza scartare niente.
    pub fn cancel(&mut self) {
        if self.confirm().is_some() {
            self.current = None;
        }
    }

    /// La risposta di `worktree.discard`, nella fase in cui era.
    pub fn receive(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(discard) = self.current.take() else {
            return Received::Failed {
                message: tr!("discard-unexpected-response", command = COMMAND),
                reload: true,
            };
        };
        let label = describe(&discard.paths);
        match result {
            Err(_)
                if kind == Some(ErrorKind::ConfirmRequired) && discard.phase == Phase::Checking =>
            {
                self.current = Some(Discard {
                    paths: discard.paths,
                    phase: Phase::Confirm {
                        reason: detail.unwrap_or_default(),
                    },
                });
                Received::Confirm
            }
            Err(e) if kind == Some(ErrorKind::NotAllowed) => Received::Failed {
                message: tr!("discard-not-allowed", label = &label, error = e),
                reload: false,
            },
            Err(e) => Received::Failed {
                message: tr!("discard-failed", label = &label, error = e),
                reload: false,
            },
            Ok(value) => match serde_json::from_value::<DiscardResult>(value) {
                Ok(result) => Received::Done(Box::new(result)),
                Err(e) => Received::Failed {
                    message: tr!(
                        "discard-unexpected-result",
                        command = COMMAND,
                        error = e.to_string()
                    ),
                    reload: true,
                },
            },
        }
    }
}

/// Un file scartato, come lo elenca `worktree.discard`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DiscardedFile {
    pub path: RepoPath,
}

/// Risultato di `worktree.discard` con `"confirm": true`.
#[derive(Debug, Clone, Deserialize)]
pub struct DiscardResult {
    pub files: Vec<DiscardedFile>,
    /// Il ref del backup del worktree.
    pub backup: String,
    /// Il nuovo stato; `None` se non si è riletto (lo scarto è fatto lo stesso).
    pub status: Option<Status>,
    pub status_error: Option<String>,
    #[serde(flatten)]
    pub history: History,
}

impl DiscardResult {
    /// Il messaggio per la barra di stato.
    pub fn notice(&self) -> String {
        match self.files.as_slice() {
            [one] => tr!(
                "discard-done-one",
                path = one.path.to_string_lossy().into_owned(),
                backup = &self.backup
            ),
            many => tr!("discard-done-many", n = many.len(), backup = &self.backup),
        }
    }
}

/// Lo stato del diario delle operazioni: cosa annullerebbe "Annulla" e cosa rifarebbe "Ripeti".
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct History {
    pub can_undo: bool,
    pub can_redo: bool,
    pub next_undo: Option<String>,
    pub next_redo: Option<String>,
}

/// Annulla o Ripeti.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndoOp {
    Undo,
    Redo,
}

impl UndoOp {
    /// L'id del comando del Registry.
    pub fn command(self) -> &'static str {
        match self {
            UndoOp::Undo => "edit.undo",
            UndoOp::Redo => "edit.redo",
        }
    }

    pub fn label(self) -> String {
        match self {
            UndoOp::Undo => tr!("undo-undo"),
            UndoOp::Redo => tr!("undo-redo"),
        }
    }

    /// Vero se c'è qualcosa da fare.
    pub fn available(self, history: &History) -> bool {
        match self {
            UndoOp::Undo => history.can_undo,
            UndoOp::Redo => history.can_redo,
        }
    }

    /// La descrizione della voce che toccherebbe.
    pub fn next(self, history: &History) -> Option<&str> {
        match self {
            UndoOp::Undo => history.next_undo.as_deref(),
            UndoOp::Redo => history.next_redo.as_deref(),
        }
    }

    /// Il suggerimento del pulsante.
    pub fn hint(self, history: &History) -> String {
        match (self, self.next(history)) {
            (UndoOp::Undo, Some(next)) => tr!("undo-undo-hint", next = next),
            (UndoOp::Redo, Some(next)) => tr!("undo-redo-hint", next = next),
            (UndoOp::Undo, None) => tr!("undo-nothing-to-undo"),
            (UndoOp::Redo, None) => tr!("undo-nothing-to-redo"),
        }
    }
}

/// La voce del diario toccata da undo o redo: basta la descrizione.
#[derive(Debug, Clone, Deserialize)]
pub struct EntryName {
    pub description: String,
}

/// Risultato di `edit.undo`/`edit.redo`.
#[derive(Debug, Clone, Deserialize)]
pub struct UndoResult {
    pub undone: Option<EntryName>,
    pub redone: Option<EntryName>,
    #[serde(default)]
    pub backups: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub status: Option<Status>,
    pub status_error: Option<String>,
    #[serde(flatten)]
    pub history: History,
}

impl UndoResult {
    /// Il messaggio per la barra di stato.
    pub fn notice(&self) -> String {
        let mut text = match (&self.undone, &self.redone) {
            (Some(entry), _) => tr!("undo-undone", description = &entry.description),
            (None, Some(entry)) => tr!("undo-redone", description = &entry.description),
            (None, None) => tr!("undo-done"),
        };
        if !self.backups.is_empty() {
            text.push_str(&tr!("undo-backups", refs = self.backups.join(", ")));
        }
        for warning in &self.warnings {
            text.push_str(&tr!("undo-warning", warning = warning));
        }
        text
    }
}

impl App {
    /// Le azioni di questa funzione, smistate da [`App::apply`].
    pub(crate) fn apply_discard(&mut self, action: Action) {
        match action {
            Action::Discard(paths) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let label = discard::describe_check(&paths);
                if let Some(params) = repo.discard.start(paths) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    self.submit(discard::COMMAND, params, Request::Discard(generation));
                }
            }
            Action::ConfirmDiscard => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                let label = repo.discard.confirm().map(|d| discard::describe(&d.paths));
                if let Some(params) = repo.discard.confirmed() {
                    repo.write_label = label;
                    self.submit(discard::COMMAND, params, Request::Discard(generation));
                }
            }
            Action::CancelDiscard => {
                if let Some(repo) = &mut self.repo {
                    repo.discard.cancel();
                }
            }
            Action::Undo(op) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some()
                    || repo.modal_open()
                    || !op.available(&repo.history)
                {
                    return;
                }
                repo.write_error = None;
                repo.write_label = Some(op.hint(&repo.history));
                self.submit(op.command(), json!({}), Request::Undo(generation));
            }
            _ => unreachable!("azione smistata alla funzione sbagliata"),
        }
    }

    /// Risposta di `worktree.discard`: il controllo apre il dialogo di conferma, lo scarto
    /// fatto porta il nuovo stato e i pulsanti Annulla/Ripeti; un rifiuto va nell'area di stage.
    pub(crate) fn receive_discard(&mut self, generation: u64, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match repo
            .discard
            .receive(done.result, done.error_kind, done.error_detail)
        {
            Received::Confirm => {}
            Received::Done(result) => {
                repo.history = result.history.clone();
                let notice = result.notice();
                match result.status {
                    Some(status) => {
                        repo.set_status(status);
                        self.error.clear();
                        self.follow_diff();
                    }
                    None => {
                        let reason = result.status_error.unwrap_or_default();
                        self.error = tr!("discard-status-not-reread", reason = reason);
                        self.submit("repo.status", json!({}), Request::Status(generation));
                    }
                }
                self.notice = notice;
            }
            Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                repo.write_error = Some(message.clone());
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
                }
            }
        }
    }

    /// Risposta di `edit.undo`/`edit.redo`: il diario aggiornato, poi si rilegge tutto (un
    /// undo può spostare branch o rimettere file).
    pub(crate) fn receive_undo(&mut self, generation: u64, done: Done) {
        let result = done.result.and_then(|value| {
            serde_json::from_value::<UndoResult>(value)
                .map_err(|e| format!("{}: risultato inatteso: {e}", done.command))
        });
        let Some(repo) = self.current(generation) else {
            return;
        };
        let label = repo
            .write_label
            .take()
            .unwrap_or_else(|| done.command.clone());
        match result {
            Ok(result) => {
                repo.history = result.history.clone();
                if let Some(status) = result.status.clone() {
                    repo.set_status(status);
                }
                self.notice = result.notice();
                self.error = result
                    .status_error
                    .map(|e| format!("Fatto, ma lo stato non si rilegge: {e}"))
                    .unwrap_or_default();
                self.follow_diff();
                self.load();
            }
            Err(e) => {
                log::warn!("{}: {e}", done.command);
                self.error = format!("{label} non riuscito: {e}");
                self.notice.clear();
                // Il diario si rilegge: una voce potrebbe non essere più annullabile.
                self.submit("edit.history", json!({}), Request::History(generation));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::{Side, StageLists};
    use project_domain::Conflict;

    fn item(path: &str, change: Change, conflict: Option<Conflict>) -> StageItem {
        StageItem {
            path: path.into(),
            change,
            original: None,
            conflict,
        }
    }

    #[test]
    fn only_tracked_files_without_conflicts_are_discardable() {
        let items = [
            item("zeta.rs", Change::Unmerged, Some(Conflict::BothModified)),
            item("README.md", Change::Modified, None),
            item("tolto.txt", Change::Deleted, None),
            item("note.txt", Change::Untracked, None),
        ];
        let can: Vec<bool> = items.iter().map(discardable).collect();
        assert_eq!(can, [false, true, true, false]);
        assert!(
            not_discardable_reason(&items[0])
                .unwrap()
                .contains("conflitto")
        );
        assert!(
            not_discardable_reason(&items[3])
                .unwrap()
                .contains("non tracciato")
        );
        assert_eq!(not_discardable_reason(&items[1]), None);
        assert_eq!(
            discardable_paths(&items).unwrap(),
            ["README.md", "tolto.txt"].map(RepoPath::from)
        );
        assert_eq!(discardable_paths(&items[3..]), None);
        assert_eq!(discardable_paths(&[]), None);
    }

    #[test]
    fn confirm_is_sent_only_after_the_dialog() {
        let mut b = DiscardBox::default();
        assert_eq!(b.start(Vec::new()), None);
        let check = b.start(vec!["README.md".into()]).unwrap();
        assert_eq!(check, json!({ "paths": ["README.md"] }));
        // Uno alla volta, e niente conferma prima della risposta del motore.
        assert_eq!(b.start(vec!["altro".into()]), None);
        assert_eq!(b.confirmed(), None);
        assert!(b.confirm().is_none());

        let r = b.receive(
            Err("chiede una conferma".to_owned()),
            Some(ErrorKind::ConfirmRequired),
            Some("motivo".to_owned()),
        );
        assert!(matches!(r, Received::Confirm));
        let confirm = b.confirm().unwrap();
        assert_eq!(
            confirm.phase,
            Phase::Confirm {
                reason: "motivo".to_owned()
            }
        );
        assert_eq!(confirm.paths, [RepoPath::from("README.md")]);

        let send = b.confirmed().unwrap();
        assert_eq!(send, json!({ "paths": ["README.md"], "confirm": true }));
        assert!(b.confirm().is_none());
        assert_eq!(b.current().unwrap().phase, Phase::Sending);
        assert_eq!(b.confirmed(), None);
    }

    #[test]
    fn cancelling_the_dialog_sends_nothing() {
        let mut b = DiscardBox::default();
        b.start(vec!["a".into(), "b".into()]).unwrap();
        b.receive(Err(String::new()), Some(ErrorKind::ConfirmRequired), None);
        b.cancel();
        assert!(b.current().is_none());
        assert_eq!(b.confirmed(), None);
    }

    #[test]
    fn refusals_are_shown_and_close_the_discard() {
        let mut b = DiscardBox::default();
        b.start(vec!["README.md".into()]).unwrap();
        let r = b.receive(
            Err("operazione rifiutata: ci sono file in conflitto".to_owned()),
            Some(ErrorKind::NotAllowed),
            None,
        );
        let Received::Failed { message, reload } = r else {
            panic!("{r:?}");
        };
        assert_eq!(
            message,
            "Scarto delle modifiche di README.md non possibile: operazione rifiutata: ci sono \
             file in conflitto"
        );
        assert!(!reload);
        assert!(b.current().is_none());

        // Una conferma chiesta di nuovo dopo `confirm: true` non riapre il dialogo all'infinito.
        b.start(vec!["a".into(), "b".into()]).unwrap();
        b.receive(Err(String::new()), Some(ErrorKind::ConfirmRequired), None);
        b.confirmed().unwrap();
        let r = b.receive(Err("x".to_owned()), Some(ErrorKind::ConfirmRequired), None);
        assert!(matches!(r, Received::Failed { .. }), "{r:?}");
        assert!(b.current().is_none());
    }

    #[test]
    fn undo_buttons_name_the_next_entry() {
        let history = History {
            can_undo: true,
            can_redo: false,
            next_undo: Some("Scarta le modifiche di «README.md»".to_owned()),
            next_redo: None,
        };
        assert!(UndoOp::Undo.available(&history));
        assert!(!UndoOp::Redo.available(&history));
        assert_eq!(
            UndoOp::Undo.hint(&history),
            "Annulla: Scarta le modifiche di «README.md»"
        );
        assert_eq!(UndoOp::Redo.hint(&history), "Niente da ripetere");
        assert_eq!(UndoOp::Undo.command(), "edit.undo");
        assert_eq!(UndoOp::Redo.command(), "edit.redo");
    }

    fn statuses(out: &Value) -> StageLists {
        StageLists::new(&serde_json::from_value(out["status"].clone()).unwrap())
    }

    /// Il percorso completo con il repo di esempio del `MemoryBackend`, come lo fa la UI:
    /// controllo, conferma, scarto, poi undo e redo dal pulsante.
    #[test]
    fn discarding_with_the_sample_repo_then_undo() {
        use project_engine::{EngineError, MemoryOpener, Registry, Session};
        use std::sync::Arc;

        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(MemoryOpener::sample()));
        let path = MemoryOpener::SAMPLE_PATH;
        registry
            .run("repo.open", &mut session, json!({ "path": path }))
            .unwrap();
        let history: History = serde_json::from_value(
            registry
                .run("edit.history", &mut session, json!({}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(history, History::default());
        let status = registry
            .run("repo.status", &mut session, json!({}))
            .unwrap();
        let lists = StageLists::new(&serde_json::from_value(status).unwrap());
        let paths = discardable_paths(&lists.unstaged).unwrap();
        assert!(paths.contains(&"src/lib.rs".into()), "{paths:?}");
        let diff_before = registry
            .run(
                "diff.worktree",
                &mut session,
                json!({ "path": "src/lib.rs" }),
            )
            .unwrap();

        // Il controllo: niente cambia, serve la conferma.
        let mut b = DiscardBox::default();
        let check = b.start(vec!["src/lib.rs".into()]).unwrap();
        let err = registry.run(COMMAND, &mut session, check).unwrap_err();
        assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
        let r = b.receive(
            Err(err.to_string()),
            Some(ErrorKind::of(&err)),
            ErrorKind::detail(&err),
        );
        assert!(matches!(r, Received::Confirm));
        let Phase::Confirm { reason } = &b.confirm().unwrap().phase else {
            unreachable!()
        };
        assert!(reason.contains("src/lib.rs"), "{reason}");

        // La conferma.
        let send = b.confirmed().unwrap();
        let out = registry.run(COMMAND, &mut session, send).unwrap();
        assert!(!statuses(&out).contains(Side::Unstaged, &"src/lib.rs".into()));
        let Received::Done(result) = b.receive(Ok(out), None, None) else {
            panic!()
        };
        assert!(result.backup.starts_with("refs/project/backup/discard-"));
        assert_eq!(
            result.files,
            [DiscardedFile {
                path: "src/lib.rs".into()
            }]
        );
        assert!(result.history.can_undo);
        assert_eq!(
            UndoOp::Undo.next(&result.history),
            Some("Scarta le modifiche di «src/lib.rs»")
        );
        assert!(result.notice().contains("Annulla"), "{}", result.notice());

        // Annulla: il file torna con le sue modifiche, identiche.
        let out = registry
            .run(UndoOp::Undo.command(), &mut session, json!({}))
            .unwrap();
        assert!(statuses(&out).contains(Side::Unstaged, &"src/lib.rs".into()));
        let undo: UndoResult = serde_json::from_value(out).unwrap();
        assert!(
            undo.notice().starts_with("Annullato: Scarta"),
            "{}",
            undo.notice()
        );
        assert!(undo.history.can_redo && !undo.history.can_undo);
        let diff_after = registry
            .run(
                "diff.worktree",
                &mut session,
                json!({ "path": "src/lib.rs" }),
            )
            .unwrap();
        assert_eq!(diff_before, diff_after);

        // Ripeti: scartate di nuovo.
        let out = registry
            .run(UndoOp::Redo.command(), &mut session, json!({}))
            .unwrap();
        assert!(!statuses(&out).contains(Side::Unstaged, &"src/lib.rs".into()));
        let redo: UndoResult = serde_json::from_value(out).unwrap();
        assert!(redo.notice().starts_with("Rifatto: "), "{}", redo.notice());
    }

    #[test]
    fn untracked_files_are_refused_before_the_dialog() {
        use project_engine::{MemoryOpener, Registry, Session};
        use std::sync::Arc;

        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(MemoryOpener::sample()));
        registry
            .run(
                "repo.open",
                &mut session,
                json!({ "path": MemoryOpener::SAMPLE_PATH }),
            )
            .unwrap();
        let mut b = DiscardBox::default();
        let check = b.start(vec!["note.txt".into()]).unwrap();
        let err = registry.run(COMMAND, &mut session, check).unwrap_err();
        let r = b.receive(Err(err.to_string()), Some(ErrorKind::of(&err)), None);
        let Received::Failed { message, .. } = r else {
            panic!("{r:?}")
        };
        assert!(message.contains("non possibile"), "{message}");
        assert!(b.confirm().is_none());
    }
}
