//! Cherry-pick e revert (S31) dal menu del commit: `commit.cherry_pick` e `commit.revert`.
//! Solo dati, senza egui. Contratto dei comandi in `impl-docs/git/cherry-pick.md`.
//!
//! È una parte di [`MergeBox`]: l'avvio, la finestra di un rifiuto e il riquadro
//! dell'operazione in corso sono quelli del merge. Per un commit di merge si sceglie prima il
//! genitore (`mainline`) in un piccolo dialogo; per gli altri si parte subito. Fermo in
//! conflitto, il cherry-pick o il revert **resta in corso** e si conclude o si annulla dal
//! riquadro con `cherry_pick.continue`/`revert.continue` e `*.abort`, come il merge.

use super::{
    Dialog, ErrorKind, Failure, History, Kind, MergeBox, Op, Outcome, Received, Starting, parse,
    reread_warning, unexpected,
};
use project_domain::{Oid, RefInfo, RepoPath, Status};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

/// Cherry-pick o revert di un commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    CherryPick,
    Revert,
}

impl Pick {
    /// L'id del comando del Registry.
    pub fn command(self) -> &'static str {
        match self {
            Pick::CherryPick => "commit.cherry_pick",
            Pick::Revert => "commit.revert",
        }
    }

    /// L'operazione che resta in corso se si ferma.
    pub fn kind(self) -> Kind {
        match self {
            Pick::CherryPick => Kind::CherryPick,
            Pick::Revert => Kind::Revert,
        }
    }
}

/// Il dialogo che chiede da quale genitore di un commit di merge prendere le modifiche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainlineDialog {
    pub pick: Pick,
    /// Il commit di merge.
    pub rev: Oid,
    /// Come mostrarlo.
    pub what: String,
    /// Il branch attivo, su cui si applica.
    pub into: String,
    /// I genitori nell'ordine di git: id, nome (branch o id abbreviato), oggetto se noto.
    pub parents: Vec<(Oid, String, Option<String>)>,
    /// Il genitore scelto, da 1.
    pub mainline: usize,
}

impl MergeBox {
    /// Comincia il cherry-pick o il revert di `rev` (mostrato come `what`) sul branch `into`.
    /// Con più di un genitore apre il dialogo del `mainline` e dà `None`; altrimenti
    /// parametri e descrizione. `None` anche se un comando è in corso o un dialogo è aperto.
    pub fn start_pick(
        &mut self,
        pick: Pick,
        rev: Oid,
        what: String,
        into: String,
        parents: Vec<(Oid, String, Option<String>)>,
    ) -> Option<(Value, String)> {
        if self.busy() || self.dialog.is_some() {
            return None;
        }
        if parents.len() > 1 {
            self.failure = None;
            self.dialog = Some(Dialog::Mainline(MainlineDialog {
                pick,
                rev,
                what,
                into,
                parents,
                mainline: 1,
            }));
            return None;
        }
        Some(self.begin_pick(pick, rev, what, into, None))
    }

    /// Il genitore scelto nel dialogo del `mainline` (da 1).
    pub fn set_mainline(&mut self, mainline: usize) {
        if let Some(Dialog::Mainline(dialog)) = &mut self.dialog
            && (1..=dialog.parents.len()).contains(&mainline)
        {
            dialog.mainline = mainline;
        }
    }

    /// Invio del dialogo del `mainline`: comando, parametri e descrizione.
    pub fn submit_mainline(&mut self) -> Option<(Op, Value, String)> {
        let Some(Dialog::Mainline(dialog)) = self.dialog.take() else {
            return None;
        };
        let pick = dialog.pick;
        let (params, label) = self.begin_pick(
            pick,
            dialog.rev,
            dialog.what,
            dialog.into,
            Some(dialog.mainline),
        );
        Some((Op::Pick(pick), params, label))
    }

    fn begin_pick(
        &mut self,
        pick: Pick,
        rev: Oid,
        what: String,
        into: String,
        mainline: Option<usize>,
    ) -> (Value, String) {
        let mut params = json!({ "rev": rev.to_string() });
        if let Some(mainline) = mainline {
            params["mainline"] = json!(mainline);
        }
        let label = tr!(
            "merge-pick-label",
            title = pick.kind().title(),
            what = &what,
            into = &into
        );
        self.failure = None;
        self.starting = Some(Starting { what, into });
        (params, label)
    }

    pub(super) fn receive_pick(
        &mut self,
        pick: Pick,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(starting) = self.starting.take() else {
            return unexpected(Op::Pick(pick));
        };
        let title = pick.kind().title();
        let what = tr!(
            "merge-pick-label",
            title = title,
            what = &starting.what,
            into = &starting.into
        );
        match result {
            Err(e) => {
                // Rifiuti del motore (modifiche locali, già applicato, HEAD staccata, mainline):
                // niente è cambiato. Altri errori (git): forse sì, si rilegge.
                let refused = matches!(
                    kind,
                    Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams | ErrorKind::UnknownRev)
                );
                let (window, explanation) = if refused {
                    (
                        tr!("merge-refused-title", title = title),
                        tr!("merge-refused", what = &what, error = &e),
                    )
                } else {
                    (
                        tr!("merge-failed-title", title = title),
                        tr!("merge-failed", what = &what, error = &e),
                    )
                };
                let git = detail.filter(|d| kind == Some(ErrorKind::Git) && !d.trim().is_empty());
                self.failure = Some(Failure {
                    title: window,
                    explanation: explanation.clone(),
                    git,
                });
                Received::Failed {
                    message: explanation,
                    reload: !refused,
                }
            }
            Ok(value) => match parse::<PickResult>(Op::Pick(pick), value) {
                Ok(result) => {
                    let warning = reread_warning(
                        &tr!("merge-done", title = title),
                        &result.warnings,
                        result.status_error.clone(),
                        result.refs_error.clone(),
                    );
                    Received::Done(Box::new(Outcome {
                        notice: result.notice(pick, &starting),
                        moved: result.after != result.before,
                        show_changes: result.in_progress && !result.conflicts.is_empty(),
                        status: result.status,
                        refs: result.refs,
                        history: result.history,
                        warning,
                    }))
                }
                Err(message) => Received::Failed {
                    message,
                    reload: true,
                },
            },
        }
    }
}

/// Risultato di `commit.cherry_pick` e `commit.revert`.
#[derive(Debug, Clone, Deserialize)]
struct PickResult {
    branch: String,
    before: Oid,
    after: Oid,
    in_progress: bool,
    #[serde(default)]
    conflicts: Vec<RepoPath>,
    #[serde(default)]
    warnings: Vec<String>,
    status: Option<Status>,
    status_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

impl PickResult {
    /// Il messaggio per la barra di stato.
    fn notice(&self, pick: Pick, starting: &Starting) -> String {
        let kind = pick.kind();
        let title = kind.title();
        let name = kind.name();
        let what = &starting.what;
        let branch = self
            .branch
            .strip_prefix("refs/heads/")
            .unwrap_or(&self.branch);
        if self.in_progress {
            return match self.conflicts.len() {
                0 => tr!(
                    "merge-pick-stopped-before-commit",
                    title = title,
                    what = what,
                    branch = branch,
                    name = name
                ),
                n => tr!(
                    "merge-pick-stopped",
                    title = title,
                    what = what,
                    branch = branch,
                    files = tr!("merge-conflict-files", n = n),
                    name = name
                ),
            };
        }
        let id = self.after.short();
        match pick {
            Pick::CherryPick => {
                tr!("merge-cherry-picked", what = what, branch = branch, id = id)
            }
            Pick::Revert => tr!("merge-reverted", what = what, branch = branch, id = id),
        }
    }
}

#[cfg(test)]
#[path = "pick_tests.rs"]
mod tests;
