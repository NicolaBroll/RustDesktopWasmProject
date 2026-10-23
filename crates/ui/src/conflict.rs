//! Editor dei conflitti (S30): un file in conflitto si apre al posto del grafo o del diff, con
//! le tre versioni (base, nostra, loro) in alto e il risultato modificabile in basso. Solo
//! dati, senza egui: si prova con test unitari. Contratto in `impl-docs/git/conflitti.md`.
//!
//! - **Lettura**: `conflict.show` dà le versioni e il `digest` del file del worktree; il testo
//!   del worktree si analizza con [`parse_conflicts`]. Se è testo e si analizza, il risultato
//!   parte da lì ed è modificabile; altrimenti (binario, file assente, marcatori rotti) si
//!   offrono solo "Usa nostra" / "Usa loro" (e "Elimina il file" quando un lato l'ha eliminato).
//! - **Scelte**: i blocchi sono quelli del testo **attuale** del risultato, rianalizzato a ogni
//!   modifica; una scelta sostituisce un blocco con [`resolve_block`] (o tutti con
//!   [`resolve_all`]), il resto, modifiche a mano comprese, resta.
//! - **Segna come risolto**: `conflict.resolve` con il `digest` letto e il testo. Se restano
//!   marcatori si chiede conferma in due tempi; anche "Elimina il file" chiede conferma. Un
//!   `StaleDiff` (il file è cambiato dopo la lettura) rilegge il file.

use crate::ErrorKind;
use crate::{Action, App, Done, Request};
use project_domain::{
    Choice, Conflict, ConflictText, RepoPath, Status, parse_conflicts, resolve_all, resolve_block,
};
use serde::Deserialize;
use serde_json::{Value, json};

/// L'id del comando che legge le versioni del file in conflitto.
pub const SHOW: &str = "conflict.show";
/// L'id del comando che risolve il conflitto di un file.
pub const RESOLVE: &str = "conflict.resolve";

/// Una versione del file (`base`, `ours`, `theirs` o `worktree` di `conflict.show`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Version {
    /// Il contenuto, se è UTF-8 valido e senza NUL.
    pub text: Option<String>,
    #[serde(default)]
    pub binary: bool,
    #[serde(default)]
    pub size: u64,
}

/// Risultato di `conflict.show`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Shown {
    pub path: RepoPath,
    pub kind: Conflict,
    pub base: Option<Version>,
    pub ours: Option<Version>,
    pub theirs: Option<Version>,
    pub worktree: Option<Version>,
    pub marker_size: usize,
    pub digest: String,
}

/// Una delle tre colonne in alto, già divisa in righe (la vista le mostra virtualizzate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Column {
    /// Il testo, una voce per riga.
    Text(Vec<String>),
    /// Lo stadio manca (il file non c'era da quel lato).
    Missing,
    /// Il contenuto non è testo.
    Binary(u64),
}

impl Column {
    fn of(version: Option<&Version>) -> Self {
        match version {
            None => Column::Missing,
            Some(Version {
                text: Some(text), ..
            }) => Column::Text(text.lines().map(str::to_owned).collect()),
            Some(v) => Column::Binary(v.size),
        }
    }
}

/// Il lato da usare per intero quando il file non si modifica qui.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Ours,
    Theirs,
}

/// Un blocco di conflitto del risultato attuale, per la sua riga di pulsanti.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockInfo {
    /// L'indice da passare a [`resolve_block`].
    pub index: usize,
    /// La riga (da 1) del marcatore `<<<<<<<` nel risultato.
    pub line: usize,
    /// Il blocco ha la versione base (stile `diff3`): c'è il pulsante "Base".
    pub has_base: bool,
}

/// Cosa aspetta la conferma in due tempi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    /// Il risultato contiene ancora questi blocchi di marcatori.
    Markers(usize),
    /// Il file va eliminato.
    Delete,
}

/// Le versioni lette e il risultato in lavorazione.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub shown: Shown,
    pub base: Column,
    pub ours: Column,
    pub theirs: Column,
    /// Le etichette dei marcatori del file letto (`<<<<<<< HEAD`, `>>>>>>> feature`), se ci sono.
    pub ours_label: Option<String>,
    pub theirs_label: Option<String>,
    /// Il risultato modificabile: `None` se il file del worktree non è testo, manca o non si
    /// analizza (allora solo "Usa nostra" / "Usa loro").
    result: Option<String>,
    /// L'analisi del risultato attuale: `Err` se i marcatori sono rotti (niente pulsanti per
    /// blocco, il testo resta modificabile).
    parsed: Result<ConflictText, String>,
}

impl Loaded {
    fn new(shown: Shown) -> Self {
        let worktree_text = shown.worktree.as_ref().and_then(|v| v.text.clone());
        let parsed_worktree = worktree_text
            .as_deref()
            .map(|t| parse_conflicts(t.as_bytes(), shown.marker_size).map_err(|e| e.to_string()));
        let (result, parsed) = match (worktree_text, parsed_worktree) {
            (Some(text), Some(Ok(parsed))) => (Some(text), Ok(parsed)),
            (_, Some(Err(e))) => (None, Err(e)),
            _ => (
                None,
                Err("il file del worktree non è testo o non c'è".to_owned()),
            ),
        };
        let label = |pick: fn(&project_domain::ConflictBlock) -> &Vec<u8>| {
            parsed
                .as_ref()
                .ok()
                .and_then(|p| p.blocks().map(pick).find(|l| !l.is_empty()))
                .map(|l| String::from_utf8_lossy(l).into_owned())
        };
        let ours_label = label(|b| &b.ours_label);
        let theirs_label = label(|b| &b.theirs_label);
        Self {
            base: Column::of(shown.base.as_ref()),
            ours: Column::of(shown.ours.as_ref()),
            theirs: Column::of(shown.theirs.as_ref()),
            ours_label,
            theirs_label,
            result,
            parsed,
            shown,
        }
    }

    /// Il risultato modificabile, se il file si modifica qui.
    pub fn result(&self) -> Option<&str> {
        self.result.as_deref()
    }

    /// I blocchi del risultato attuale; vuoto se non si analizza o non è modificabile.
    pub fn blocks(&self) -> Vec<BlockInfo> {
        let (Some(text), Ok(parsed)) = (&self.result, &self.parsed) else {
            return Vec::new();
        };
        parsed
            .blocks()
            .enumerate()
            .map(|(index, block)| BlockInfo {
                index,
                line: 1 + text.as_bytes()[..block.range.start.min(text.len())]
                    .iter()
                    .filter(|&&b| b == b'\n')
                    .count(),
                has_base: block.base.is_some(),
            })
            .collect()
    }

    /// Perché il risultato attuale non si analizza (marcatori rotti a mano), se è così.
    pub fn parse_error(&self) -> Option<&str> {
        self.result.as_ref()?;
        self.parsed.as_ref().err().map(String::as_str)
    }

    /// Il testo di un lato, se è testo: per "Usa nostra" / "Usa loro".
    pub fn side_text(&self, pick: Pick) -> Option<&str> {
        let version = match pick {
            Pick::Ours => self.shown.ours.as_ref(),
            Pick::Theirs => self.shown.theirs.as_ref(),
        };
        version?.text.as_deref()
    }

    /// Vero se un lato ha eliminato il file: si offre "Elimina il file".
    pub fn can_delete(&self) -> bool {
        matches!(
            self.shown.kind,
            Conflict::DeletedByUs | Conflict::DeletedByThem
        )
    }

    fn set_result(&mut self, text: String) {
        self.parsed =
            parse_conflicts(text.as_bytes(), self.shown.marker_size).map_err(|e| e.to_string());
        self.result = Some(text);
    }

    /// Applica una sostituzione pura al risultato attuale; `false` se non si può.
    fn replace(
        &mut self,
        f: impl FnOnce(&[u8], &ConflictText) -> Result<Vec<u8>, project_domain::DomainError>,
    ) -> Result<(), String> {
        let (Some(text), Ok(parsed)) = (&self.result, &self.parsed) else {
            return Err("il risultato non ha blocchi da scegliere".to_owned());
        };
        let bytes = f(text.as_bytes(), parsed).map_err(|e| e.to_string())?;
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        self.set_result(text);
        Ok(())
    }
}

/// A che punto è la lettura del file.
#[derive(Debug, Clone)]
pub enum Load {
    Loading,
    Failed(String),
    Ready(Box<Loaded>),
}

/// L'editor aperto su un file.
#[derive(Debug, Clone)]
pub struct Editor {
    pub path: RepoPath,
    /// Il numero della lettura attesa: le risposte vecchie si scartano.
    seq: u64,
    pub load: Load,
    /// La conferma in due tempi aperta, se c'è.
    pub pending: Option<Pending>,
    /// Vero mentre `conflict.resolve` è in corso.
    pub sending: bool,
    /// L'ultimo errore (rifiuto, file cambiato), mostrato nell'editor.
    pub error: Option<String>,
}

impl Editor {
    /// Le versioni lette, se sono arrivate.
    pub fn loaded(&self) -> Option<&Loaded> {
        match &self.load {
            Load::Ready(loaded) => Some(loaded),
            _ => None,
        }
    }

    fn loaded_mut(&mut self) -> Option<&mut Loaded> {
        match &mut self.load {
            Load::Ready(loaded) if !self.sending => Some(loaded),
            _ => None,
        }
    }
}

/// Cosa è arrivato da `conflict.resolve`.
#[derive(Debug)]
pub enum Received {
    /// Risolto: l'editor è chiuso; lo stato nuovo e il messaggio per la barra.
    Done { status: Box<Status>, notice: String },
    /// Rifiutato: l'errore è nell'editor. Con `reread` si rilegge il file (era cambiato), con
    /// `reload` lo stato (qualcosa può essere cambiato).
    Failed {
        message: String,
        reread: Option<(Value, u64)>,
        reload: bool,
    },
}

/// Risultato di `conflict.resolve`.
#[derive(Deserialize)]
struct Resolved {
    status: Status,
    backup: Option<String>,
    index_backup: Option<String>,
}

/// Lo stato dell'editor dei conflitti: al più un file aperto.
#[derive(Debug, Default)]
pub struct ConflictBox {
    editor: Option<Editor>,
    seq: u64,
}

impl ConflictBox {
    /// L'editor aperto, se c'è.
    pub fn editor(&self) -> Option<&Editor> {
        self.editor.as_ref()
    }

    /// Vero mentre `conflict.resolve` è in corso.
    pub fn busy(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.sending)
    }

    /// Apre l'editor su `path`: i parametri di `conflict.show` e il numero della lettura.
    /// `None` se è già aperto su quel file (le modifiche restano) o se un invio è in corso.
    pub fn open(&mut self, path: RepoPath) -> Option<(Value, u64)> {
        if self.busy() {
            return None;
        }
        if let Some(editor) = &self.editor
            && editor.path == path
            && !matches!(editor.load, Load::Failed(_))
        {
            return None;
        }
        self.seq += 1;
        self.editor = Some(Editor {
            path: path.clone(),
            seq: self.seq,
            load: Load::Loading,
            pending: None,
            sending: false,
            error: None,
        });
        Some((json!({ "path": path }), self.seq))
    }

    /// "Ricomincia": rilegge il file, le scelte e le modifiche fatte qui si perdono.
    pub fn restart(&mut self) -> Option<(Value, u64)> {
        let editor = self.editor.as_mut().filter(|e| !e.sending)?;
        self.seq += 1;
        editor.seq = self.seq;
        editor.load = Load::Loading;
        editor.pending = None;
        Some((json!({ "path": editor.path }), self.seq))
    }

    /// Chiude l'editor; non durante un invio.
    pub fn close(&mut self) {
        if !self.busy() {
            self.editor = None;
        }
    }

    /// La risposta di `conflict.show` per la lettura `seq`.
    pub fn receive_show(&mut self, seq: u64, result: Result<Value, String>) {
        let Some(editor) = self.editor.as_mut().filter(|e| e.seq == seq) else {
            return;
        };
        editor.load = match result.and_then(|value| {
            serde_json::from_value::<Shown>(value)
                .map_err(|e| format!("{SHOW}: risultato inatteso: {e}"))
        }) {
            Ok(shown) => Load::Ready(Box::new(Loaded::new(shown))),
            Err(e) => Load::Failed(e),
        };
    }

    /// Un nuovo stato: se il file aperto non è più in conflitto (risolto altrove, merge
    /// annullato) l'editor si chiude.
    pub fn follow_status(&mut self, status: &Status) {
        let still = |path: &RepoPath| {
            status
                .entries
                .iter()
                .any(|e| e.is_conflict() && &e.path == path)
        };
        if let Some(editor) = &self.editor
            && !editor.sending
            && !still(&editor.path)
        {
            self.editor = None;
        }
    }

    /// Il risultato modificato a mano.
    pub fn set_result(&mut self, text: String) {
        if let Some(editor) = &mut self.editor {
            editor.pending = None;
            if let Some(loaded) = editor.loaded_mut()
                && loaded.result.is_some()
            {
                loaded.set_result(text);
            }
        }
    }

    /// Sostituisce il blocco `index` del risultato attuale con `choice`. `false` se non si può
    /// (il testo non si analizza, il blocco non c'è, "Base" senza base).
    pub fn choose(&mut self, index: usize, choice: Choice) -> bool {
        self.edit(|text, parsed| resolve_block(text, parsed, index, choice))
    }

    /// Sostituisce tutti i blocchi con la stessa scelta.
    pub fn choose_all(&mut self, choice: Choice) -> bool {
        self.edit(|text, parsed| resolve_all(text, parsed, choice))
    }

    fn edit(
        &mut self,
        f: impl FnOnce(&[u8], &ConflictText) -> Result<Vec<u8>, project_domain::DomainError>,
    ) -> bool {
        let Some(editor) = &mut self.editor else {
            return false;
        };
        editor.pending = None;
        let Some(loaded) = editor.loaded_mut() else {
            return false;
        };
        match loaded.replace(f) {
            Ok(()) => {
                editor.error = None;
                true
            }
            Err(e) => {
                editor.error = Some(e);
                false
            }
        }
    }

    /// "Segna come risolto" con il risultato: i parametri di `conflict.resolve` e la
    /// descrizione. Se restano marcatori `None` e si apre la conferma ([`Pending::Markers`]).
    pub fn resolve(&mut self) -> Option<(Value, String)> {
        let editor = self.editor.as_mut().filter(|e| !e.sending)?;
        let loaded = editor.loaded()?;
        loaded.result.as_ref()?;
        let left = loaded.parsed.as_ref().map_or(0, ConflictText::block_count);
        if left > 0 {
            editor.pending = Some(Pending::Markers(left));
            return None;
        }
        self.send_result()
    }

    /// "Usa nostra" / "Usa loro": il testo di quel lato come risultato, subito.
    pub fn use_side(&mut self, pick: Pick) -> Option<(Value, String)> {
        let editor = self.editor.as_mut().filter(|e| !e.sending)?;
        let loaded = editor.loaded()?;
        let content = loaded.side_text(pick)?.to_owned();
        let side = match pick {
            Pick::Ours => "nostra",
            Pick::Theirs => "loro",
        };
        let label = format!("Risoluzione di {} con la versione {side}", editor.path);
        let params = json!({
            "path": editor.path,
            "digest": loaded.shown.digest,
            "content": content,
        });
        Some(self.sending(params, label))
    }

    /// "Elimina il file": apre la conferma ([`Pending::Delete`]).
    pub fn delete(&mut self) {
        if let Some(editor) = self.editor.as_mut().filter(|e| !e.sending)
            && editor.loaded().is_some_and(Loaded::can_delete)
        {
            editor.pending = Some(Pending::Delete);
        }
    }

    /// La conferma in due tempi è stata data: i parametri da mandare.
    pub fn confirm(&mut self) -> Option<(Value, String)> {
        let editor = self.editor.as_mut().filter(|e| !e.sending)?;
        match editor.pending.take()? {
            Pending::Markers(_) => self.send_result(),
            Pending::Delete => {
                let loaded = editor.loaded()?;
                let params = json!({
                    "path": editor.path,
                    "digest": loaded.shown.digest,
                    "delete": true,
                });
                let label = format!("Eliminazione di {} per risolvere il conflitto", editor.path);
                Some(self.sending(params, label))
            }
        }
    }

    /// La conferma in due tempi è stata rifiutata.
    pub fn cancel(&mut self) {
        if let Some(editor) = &mut self.editor {
            editor.pending = None;
        }
    }

    fn send_result(&mut self) -> Option<(Value, String)> {
        let editor = self.editor.as_ref()?;
        let loaded = editor.loaded()?;
        let params = json!({
            "path": editor.path,
            "digest": loaded.shown.digest,
            "content": loaded.result.as_ref()?,
        });
        let label = format!("Risoluzione del conflitto di {}", editor.path);
        Some(self.sending(params, label))
    }

    fn sending(&mut self, params: Value, label: String) -> (Value, String) {
        if let Some(editor) = &mut self.editor {
            editor.sending = true;
            editor.pending = None;
            editor.error = None;
        }
        (params, label)
    }

    /// La risposta di `conflict.resolve`.
    pub fn receive_resolve(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
    ) -> Received {
        if let Some(editor) = &mut self.editor {
            editor.sending = false;
        }
        let path = self.editor.as_ref().map(|e| e.path.to_string());
        let result = result.and_then(|value| {
            serde_json::from_value::<Resolved>(value)
                .map_err(|e| format!("{RESOLVE}: risultato inatteso: {e}"))
        });
        match result {
            Ok(resolved) => {
                self.editor = None;
                let mut notice = match &path {
                    Some(path) => format!("Conflitto di {path} risolto e in stage"),
                    None => "Conflitto risolto e in stage".to_owned(),
                };
                if let Some(backup) = &resolved.backup {
                    notice.push_str(&format!("; il file com'era è salvato in {backup}"));
                }
                if let Some(backup) = &resolved.index_backup {
                    notice.push_str(&format!("; backup dell'indice in {backup}"));
                }
                Received::Done {
                    status: Box::new(resolved.status),
                    notice,
                }
            }
            Err(e) => {
                let stale = kind == Some(ErrorKind::StaleDiff);
                let message = if stale {
                    "Il file è cambiato dopo la lettura: riletto, rifare le scelte".to_owned()
                } else {
                    format!("Risoluzione non riuscita: {e}")
                };
                let reread = if stale { self.restart() } else { None };
                if let Some(editor) = &mut self.editor {
                    editor.error = Some(message.clone());
                }
                // Rifiuti senza scritture: si rilegge solo lo stato se qualcosa può essere
                // cambiato (errore di git dopo il backup, risultato illeggibile).
                let reload = !matches!(
                    kind,
                    Some(
                        ErrorKind::NotAllowed
                            | ErrorKind::InvalidParams
                            | ErrorKind::IndexLocked
                            | ErrorKind::StaleDiff
                    )
                );
                Received::Failed {
                    message,
                    reread,
                    reload,
                }
            }
        }
    }
}

impl App {
    /// Le azioni dell'editor dei conflitti, smistate da [`App::apply`].
    pub(crate) fn apply_conflict(&mut self, action: Action) {
        let generation = self.generation;
        let writing = self.executor.writing().is_some();
        let Some(repo) = &mut self.repo else {
            return;
        };
        let can_write = !writing && !repo.modal_open();
        let mut send = None;
        let mut read = None;
        match action {
            Action::OpenConflict(path) => {
                repo.stash.select(None);
                repo.select_worktree(true);
                repo.diff.close();
                read = repo.conflict.open(path);
            }
            Action::CloseConflict => repo.conflict.close(),
            Action::RestartConflict => read = repo.conflict.restart(),
            Action::ConflictText(text) => repo.conflict.set_result(text),
            Action::ChooseBlock(index, choice) => {
                repo.conflict.choose(index, choice);
            }
            Action::ChooseAll(choice) => {
                repo.conflict.choose_all(choice);
            }
            Action::ResolveConflict if can_write => send = repo.conflict.resolve(),
            Action::UseSide(pick) if can_write => send = repo.conflict.use_side(pick),
            Action::DeleteConflicted if can_write => repo.conflict.delete(),
            Action::ConfirmConflict if can_write => send = repo.conflict.confirm(),
            Action::CancelConflict => repo.conflict.cancel(),
            Action::ResolveConflict
            | Action::UseSide(_)
            | Action::DeleteConflicted
            | Action::ConfirmConflict => {}
            _ => unreachable!("azione smistata alla funzione sbagliata"),
        }
        if let Some((params, seq)) = read {
            self.submit(SHOW, params, Request::ConflictShow(generation, seq));
        }
        if let Some((params, label)) = send
            && let Some(repo) = &mut self.repo
        {
            repo.write_error = None;
            repo.write_label = Some(label);
            self.submit(RESOLVE, params, Request::ConflictResolve(generation));
        }
    }

    /// Risposta di `conflict.resolve`: stato nuovo (il riquadro dell'operazione si rilegge e
    /// "Continua" si attiva quando non restano conflitti), oppure l'errore nell'editor.
    pub(crate) fn receive_conflict(&mut self, generation: u64, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match repo.conflict.receive_resolve(done.result, done.error_kind) {
            Received::Done { status, notice } => {
                log::info!("{}: {notice}", done.command);
                repo.set_status(*status);
                self.notice = notice;
                self.error.clear();
                self.follow_diff();
            }
            Received::Failed {
                message,
                reread,
                reload,
            } => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
                let reread_file = reread.is_some();
                if let Some((params, seq)) = reread {
                    self.submit(SHOW, params, Request::ConflictShow(generation, seq));
                }
                if reload || reread_file {
                    self.submit("repo.status", json!({}), Request::Status(generation));
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "conflict_tests.rs"]
mod tests;
