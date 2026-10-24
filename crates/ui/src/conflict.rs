//! Conflict editor (S30): a conflicted file opens in place of the graph or the diff, with
//! the three versions (base, ours, theirs) on top and the editable result below. Data
//! only, no egui: tested with unit tests. Contract in `impl-docs/git/conflicts.md`.
//!
//! - **Reading**: `conflict.show` gives the versions and the `digest` of the worktree file; the worktree
//!   text is parsed with [`parse_conflicts`]. If it is text and parses, the result
//!   starts from there and is editable; otherwise (binary, missing file, broken markers) only
//!   "Use ours" / "Use theirs" are offered (and "Delete the file" when a side deleted it).
//! - **Choices**: the blocks are those of the **current** text of the result, reparsed on every
//!   edit; a choice replaces a block with [`resolve_block`] (or all with
//!   [`resolve_all`]), the rest, manual edits included, stays.
//! - **Mark as resolved**: `conflict.resolve` with the `digest` read and the text. If markers
//!   remain a two-step confirmation is asked; "Delete the file" also asks for confirmation. A
//!   `StaleDiff` (the file changed after reading) rereads the file.

use crate::ErrorKind;
use crate::{Action, App, Done, Request};
use project_domain::{
    Choice, Conflict, ConflictText, RepoPath, Status, parse_conflicts, resolve_all, resolve_block,
};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

/// The id of the command that reads the versions of the conflicted file.
pub const SHOW: &str = "conflict.show";
/// The id of the command that resolves the conflict of a file.
pub const RESOLVE: &str = "conflict.resolve";

/// A version of the file (`base`, `ours`, `theirs` or `worktree` of `conflict.show`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Version {
    /// The content, if it is valid UTF-8 without NUL.
    pub text: Option<String>,
    #[serde(default)]
    pub binary: bool,
    #[serde(default)]
    pub size: u64,
}

/// Result of `conflict.show`.
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

/// One of the three columns on top, already split into lines (the view shows them virtualized).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Column {
    /// The text, one entry per line.
    Text(Vec<String>),
    /// The stage is missing (the file was not there on that side).
    Missing,
    /// The content is not text.
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

/// The side to use whole when the file is not edited here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Ours,
    Theirs,
}

/// A conflict block of the current result, for its row of buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockInfo {
    /// The index to pass to [`resolve_block`].
    pub index: usize,
    /// The line (from 1) of the `<<<<<<<` marker in the result.
    pub line: usize,
    /// The block has the base version (`diff3` style): the "Base" button is there.
    pub has_base: bool,
}

/// What awaits the two-step confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    /// The result still contains these blocks of markers.
    Markers(usize),
    /// The file is to be deleted.
    Delete,
}

/// The versions read and the result being worked on.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub shown: Shown,
    pub base: Column,
    pub ours: Column,
    pub theirs: Column,
    /// The labels of the markers of the file read (`<<<<<<< HEAD`, `>>>>>>> feature`), if any.
    pub ours_label: Option<String>,
    pub theirs_label: Option<String>,
    /// The editable result: `None` if the worktree file is not text, is missing or does not
    /// parse (then only "Use ours" / "Use theirs").
    result: Option<String>,
    /// The parse of the current result: `Err` if the markers are broken (no per-block buttons,
    /// the text stays editable).
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
            _ => (None, Err(tr!("conflict-worktree-not-text"))),
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

    /// The editable result, if the file is edited here.
    pub fn result(&self) -> Option<&str> {
        self.result.as_deref()
    }

    /// The blocks of the current result; empty if it does not parse or is not editable.
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

    /// Why the current result does not parse (markers broken by hand), if so.
    pub fn parse_error(&self) -> Option<&str> {
        self.result.as_ref()?;
        self.parsed.as_ref().err().map(String::as_str)
    }

    /// The text of a side, if it is text: for "Use ours" / "Use theirs".
    pub fn side_text(&self, pick: Pick) -> Option<&str> {
        let version = match pick {
            Pick::Ours => self.shown.ours.as_ref(),
            Pick::Theirs => self.shown.theirs.as_ref(),
        };
        version?.text.as_deref()
    }

    /// True if a side deleted the file: "Delete the file" is offered.
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

    /// Applies a pure replacement to the current result; `false` if it cannot.
    fn replace(
        &mut self,
        f: impl FnOnce(&[u8], &ConflictText) -> Result<Vec<u8>, project_domain::DomainError>,
    ) -> Result<(), String> {
        let (Some(text), Ok(parsed)) = (&self.result, &self.parsed) else {
            return Err(tr!("conflict-no-blocks"));
        };
        let bytes = f(text.as_bytes(), parsed).map_err(|e| e.to_string())?;
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        self.set_result(text);
        Ok(())
    }
}

/// How far the reading of the file has got.
#[derive(Debug, Clone)]
pub enum Load {
    Loading,
    Failed(String),
    Ready(Box<Loaded>),
}

/// The editor open on a file.
#[derive(Debug, Clone)]
pub struct Editor {
    pub path: RepoPath,
    /// The number of the awaited reading: old answers are discarded.
    seq: u64,
    pub load: Load,
    /// The open two-step confirmation, if any.
    pub pending: Option<Pending>,
    /// True while `conflict.resolve` is in flight.
    pub sending: bool,
    /// The last error (refusal, changed file), shown in the editor.
    pub error: Option<String>,
}

impl Editor {
    /// The versions read, if they arrived.
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

/// What arrived from `conflict.resolve`.
#[derive(Debug)]
pub enum Received {
    /// Resolved: the editor is closed; the new state and the message for the bar.
    Done { status: Box<Status>, notice: String },
    /// Rejected: the error is in the editor. With `reread` the file is reread (it changed), with
    /// `reload` the state (something may have changed).
    Failed {
        message: String,
        reread: Option<(Value, u64)>,
        reload: bool,
    },
}

/// Result of `conflict.resolve`.
#[derive(Deserialize)]
struct Resolved {
    status: Status,
    backup: Option<String>,
    index_backup: Option<String>,
}

/// State of the conflict editor: at most one open file.
#[derive(Debug, Default)]
pub struct ConflictBox {
    editor: Option<Editor>,
    seq: u64,
}

impl ConflictBox {
    /// The open editor, if any.
    pub fn editor(&self) -> Option<&Editor> {
        self.editor.as_ref()
    }

    /// True while `conflict.resolve` is in flight.
    pub fn busy(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.sending)
    }

    /// Opens the editor on `path`: the parameters of `conflict.show` and the reading number.
    /// `None` if it is already open on that file (the edits stay) or if a send is in progress.
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

    /// "Start over": rereads the file, the choices and edits made here are lost.
    pub fn restart(&mut self) -> Option<(Value, u64)> {
        let editor = self.editor.as_mut().filter(|e| !e.sending)?;
        self.seq += 1;
        editor.seq = self.seq;
        editor.load = Load::Loading;
        editor.pending = None;
        Some((json!({ "path": editor.path }), self.seq))
    }

    /// Closes the editor; not during a send.
    pub fn close(&mut self) {
        if !self.busy() {
            self.editor = None;
        }
    }

    /// The answer of `conflict.show` for reading `seq`.
    pub fn receive_show(&mut self, seq: u64, result: Result<Value, String>) {
        let Some(editor) = self.editor.as_mut().filter(|e| e.seq == seq) else {
            return;
        };
        editor.load = match result.and_then(|value| {
            serde_json::from_value::<Shown>(value).map_err(|e| {
                tr!(
                    "conflict-unexpected-result",
                    command = SHOW,
                    error = e.to_string()
                )
            })
        }) {
            Ok(shown) => Load::Ready(Box::new(Loaded::new(shown))),
            Err(e) => Load::Failed(e),
        };
    }

    /// A new state: if the open file is no longer conflicted (resolved elsewhere, merge
    /// aborted) the editor closes.
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

    /// The result edited by hand.
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

    /// Replaces block `index` of the current result with `choice`. `false` if it cannot
    /// (the text does not parse, the block is not there, "Base" without base).
    pub fn choose(&mut self, index: usize, choice: Choice) -> bool {
        self.edit(|text, parsed| resolve_block(text, parsed, index, choice))
    }

    /// Replaces all blocks with the same choice.
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

    /// "Mark as resolved" with the result: the parameters of `conflict.resolve` and the
    /// description. If markers remain `None` and the confirmation opens ([`Pending::Markers`]).
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

    /// "Use ours" / "Use theirs": the text of that side as the result, right away.
    pub fn use_side(&mut self, pick: Pick) -> Option<(Value, String)> {
        let editor = self.editor.as_mut().filter(|e| !e.sending)?;
        let loaded = editor.loaded()?;
        let content = loaded.side_text(pick)?.to_owned();
        let path = editor.path.to_string();
        let label = match pick {
            Pick::Ours => tr!("conflict-label-use-ours", path = path),
            Pick::Theirs => tr!("conflict-label-use-theirs", path = path),
        };
        let params = json!({
            "path": editor.path,
            "digest": loaded.shown.digest,
            "content": content,
        });
        Some(self.sending(params, label))
    }

    /// "Delete the file": opens the confirmation ([`Pending::Delete`]).
    pub fn delete(&mut self) {
        if let Some(editor) = self.editor.as_mut().filter(|e| !e.sending)
            && editor.loaded().is_some_and(Loaded::can_delete)
        {
            editor.pending = Some(Pending::Delete);
        }
    }

    /// The two-step confirmation was given: the parameters to send.
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
                let label = tr!("conflict-label-delete", path = editor.path.to_string());
                Some(self.sending(params, label))
            }
        }
    }

    /// The two-step confirmation was declined.
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
        let label = tr!("conflict-label-resolve", path = editor.path.to_string());
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

    /// The answer of `conflict.resolve`.
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
            serde_json::from_value::<Resolved>(value).map_err(|e| {
                tr!(
                    "conflict-unexpected-result",
                    command = RESOLVE,
                    error = e.to_string()
                )
            })
        });
        match result {
            Ok(resolved) => {
                self.editor = None;
                let mut notice = match &path {
                    Some(path) => tr!("conflict-resolved-path", path = path),
                    None => tr!("conflict-resolved"),
                };
                if let Some(backup) = &resolved.backup {
                    notice.push_str(&tr!("conflict-resolved-backup", backup = backup));
                }
                if let Some(backup) = &resolved.index_backup {
                    notice.push_str(&tr!("conflict-resolved-index-backup", backup = backup));
                }
                Received::Done {
                    status: Box::new(resolved.status),
                    notice,
                }
            }
            Err(e) => {
                let stale = kind == Some(ErrorKind::StaleDiff);
                let message = if stale {
                    tr!("conflict-stale")
                } else {
                    tr!("conflict-resolve-failed", error = &e)
                };
                let reread = if stale { self.restart() } else { None };
                if let Some(editor) = &mut self.editor {
                    editor.error = Some(message.clone());
                }
                // Refusals without writes: only the state is reread if something may have
                // changed (git error after the backup, unreadable result).
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
    /// The actions of the conflict editor, dispatched by [`App::apply`].
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
            _ => unreachable!("action dispatched to the wrong function"),
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

    /// Answer of `conflict.resolve`: new state (the operation box rereads and
    /// "Continue" becomes active when no conflicts remain), or the error in the editor.
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
