//! Cherry-pick e revert di un commit (S31): `commit.cherry_pick`, `commit.revert`,
//! `cherry_pick.continue`, `cherry_pick.abort`, `revert.continue`, `revert.abort`. Tipi e
//! controlli comuni ai backend in `git/pick.rs`; contratto in `impl-docs/git/cherry-pick.md`.
//!
//! Come il merge di S29: un'operazione conclusa (subito o con `*.continue`) entra nel diario e
//! `edit.undo` la annulla con `reset --keep`; ferma in conflitto **resta in corso** e non entra
//! nel diario; `*.abort` chiede sempre `"confirm": true` e prima salva indice e worktree.

use super::merge::{abort_confirm, check_message, finish, label};
use super::{Out, to_json};
use crate::commands::parse_params;
use crate::{
    CommitRequest, EngineError, HeadState, JournalEntry, LogQuery, PickKind, PickRequest, Session,
    branch_ref, check_pick_state, check_picking, clean_merge_message, conflicted_paths,
    pick_message, pick_parent,
};
use project_domain::Head;
use serde::Deserialize;
use serde_json::{Value, json};

/// Parametri di `commit.cherry_pick` e `commit.revert`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartParams {
    /// Il commit: un id (anche abbreviato), un branch, un tag o un'altra rev.
    rev: String,
    /// Per un commit di merge: il genitore (da 1) rispetto a cui prendere le modifiche.
    #[serde(default)]
    mainline: Option<usize>,
}

/// Parametri di `*.continue`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinueParams {
    #[serde(default)]
    message: Option<String>,
}

/// Parametri di `*.abort`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AbortParams {
    #[serde(default)]
    confirm: bool,
}

pub(super) fn commit_cherry_pick(s: &mut Session, p: Value) -> Out {
    pick_start(s, p, PickKind::CherryPick)
}

pub(super) fn commit_revert(s: &mut Session, p: Value) -> Out {
    pick_start(s, p, PickKind::Revert)
}

pub(super) fn cherry_pick_continue(s: &mut Session, p: Value) -> Out {
    pick_continue(s, p, PickKind::CherryPick)
}

pub(super) fn revert_continue(s: &mut Session, p: Value) -> Out {
    pick_continue(s, p, PickKind::Revert)
}

pub(super) fn cherry_pick_abort(s: &mut Session, p: Value) -> Out {
    pick_abort(s, p, PickKind::CherryPick)
}

pub(super) fn revert_abort(s: &mut Session, p: Value) -> Out {
    pick_abort(s, p, PickKind::Revert)
}

/// Applica (`commit.cherry_pick`) o annulla (`commit.revert`) il commit `rev` sul branch attivo
/// con un commit nuovo: per il cherry-pick con l'autore e il messaggio originali, per il revert
/// con `Revert "<oggetto>"` e `This reverts commit <id>.`. Un commit di merge vuole `mainline`
/// (da 1); `mainline` su un altro commit o fuori dai genitori → [`EngineError::InvalidParams`].
///
/// Rifiutati prima di scrivere ([`EngineError::NotAllowed`], niente cambia): HEAD staccata o
/// branch senza commit, operazione in corso, file in conflitto, qualunque modifica ai file
/// tracciati ([`check_pick_state`]); una rev che non c'è → [`EngineError::UnknownRev`]; un
/// risultato vuoto (commit già applicato o già annullato) → [`EngineError::NotAllowed`], niente
/// in corso. Prima di scrivere il backend salva il branch in
/// `refs/project/backup/<cherry-pick|revert>-...`.
///
/// **Conflitti**: l'operazione resta in corso (`in_progress`, `conflicts`), da concludere con
/// `*.continue` o annullare con `*.abort`; niente nel diario. Conclusa entra nel diario:
/// `edit.undo` riporta il branch, con indice e worktree, dov'era.
///
/// Risultato: `{"rev", "commit", "summary", "mainline", "branch", "before", "after",
/// "in_progress", "conflicts", "message", "backup", "warnings", "status", "refs", "can_undo",
/// "can_redo", "next_undo", "next_redo"}`.
fn pick_start(s: &mut Session, p: Value, kind: PickKind) -> Out {
    let id = kind.command();
    let StartParams { rev, mainline } = parse_params(id, p)?;
    if rev.trim().is_empty() || rev.contains('\0') {
        return Err(EngineError::InvalidParams {
            id,
            reason: "`rev` deve essere un id di commit, un branch o un tag, non vuoto".to_owned(),
        });
    }
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let (branch, before) = match &status.head {
        Head::Branch {
            name,
            target: Some(target),
        } => (name.clone(), *target),
        Head::Branch { name, target: None } => {
            return Err(EngineError::NotAllowed(format!(
                "il branch `{}` non ha ancora commit: `{id}` ha bisogno di un commit su cui \
                 applicare, niente modificato",
                name.escape_debug()
            )));
        }
        Head::Detached(_) => {
            return Err(EngineError::NotAllowed(format!(
                "HEAD è staccata: `{id}` va fatto su un branch, niente modificato. Passare a un \
                 branch (`branch.checkout`) o crearne uno qui (`branch.create`) e riprovare"
            )));
        }
    };
    check_pick_state(&status, kind)?;
    let commit = repo
        .log(&LogQuery::page(0, 1).from_rev(rev.clone()))?
        .into_iter()
        .next()
        .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?;
    let parent = pick_parent(id, &commit, mainline)?;
    let message = pick_message(kind, &commit, parent.as_ref(), commit.parents.len() > 1);
    let refs = repo.refs()?;
    let source = label(&commit.id, &refs);
    let outcome = repo.pick(&PickRequest {
        kind,
        branch: branch.clone(),
        expected: before,
        commit: commit.id,
        parent,
        mainline,
        message,
    })?;
    let mut warnings = outcome.warnings.clone();
    // Nel diario solo il risultato atteso: un commit nuovo sopra il branch di prima.
    let as_expected = !outcome.in_progress
        && outcome.after != before
        && repo
            .log(&LogQuery::page(0, 1).from_rev(outcome.after.to_string()))
            .ok()
            .and_then(|c| c.first().map(|c| c.parents == [before]))
            .unwrap_or(false);
    if !outcome.in_progress && !as_expected {
        warnings.push(format!(
            "il branch è a {} ma non è il risultato atteso di `{id}` (un hook o un altro \
             processo l'ha spostato?): l'operazione non entra nel diario, il branch com'era è \
             in `{}`",
            outcome.after, outcome.backup
        ));
    }
    if as_expected {
        journal.record(JournalEntry::pick(
            id,
            kind,
            &branch_ref(&branch),
            before,
            outcome.after,
            HeadState::from_head(&status.head),
            &source,
            Some(outcome.backup.clone()),
        ));
    }
    let mut out = json!({
        "rev": rev,
        "commit": commit.id.to_string(),
        "summary": commit.summary,
        "mainline": mainline,
        "branch": branch,
        "before": before.to_string(),
        "after": outcome.after.to_string(),
        "in_progress": outcome.in_progress,
        "conflicts": to_json(&outcome.conflicts),
        "message": outcome.message,
        "backup": outcome.backup,
        "warnings": to_json(&warnings),
    });
    finish(&mut out, repo, journal);
    Ok(out)
}

/// Conclude il cherry-pick o il revert in corso con il commit (`git commit` come
/// `commit.create`: hook e firma secondo la configurazione; per il cherry-pick git tiene
/// l'autore del commit originale), con `message` o quello preparato da git (`MERGE_MSG` senza
/// il blocco `# Conflicts:`). Rifiutato ([`EngineError::NotAllowed`], niente cambia) senza
/// l'operazione `kind` in corso, con una sequenza di più commit avviata da terminale o se
/// restano file in conflitto. Entra nel diario come l'operazione conclusa; con HEAD staccata
/// (avviata da terminale) come un commit.
///
/// Risultato: `{"commit": Commit, "previous", "branch", "backup", "also_on", "message",
/// "status", "refs", "can_undo", "can_redo", "next_undo", "next_redo"}`.
fn pick_continue(s: &mut Session, p: Value, kind: PickKind) -> Out {
    let id = kind.continue_command();
    let ContinueParams { message } = parse_params(id, p)?;
    check_message(id, &message)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let state = repo.operation_state()?;
    check_picking(id, kind, &status, &state)?;
    let conflicts = conflicted_paths(&status);
    if !conflicts.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "restano file in conflitto ({}): risolverli e metterli in stage (`index.stage`), \
             poi ripetere `{id}`; niente committato",
            conflicts
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let refs = repo.refs()?;
    let incoming = state.incoming.first().copied();
    let original = match incoming {
        Some(oid) => repo
            .log(&LogQuery::page(0, 1).from_rev(oid.to_string()))
            .ok()
            .and_then(|c| c.into_iter().next()),
        None => None,
    };
    let message = match message {
        Some(message) => message,
        None => state
            .message
            .as_deref()
            .map(clean_merge_message)
            .filter(|m| !m.trim().is_empty())
            .or_else(|| {
                original
                    .as_ref()
                    .map(|c| pick_message(kind, c, c.parents.first(), false))
            })
            .ok_or_else(|| {
                EngineError::NotAllowed(format!(
                    "git non ha preparato un messaggio (`MERGE_MSG`) e il commit in ingresso \
                     non si legge: indicare `message` e ripetere `{id}`; niente committato"
                ))
            })?,
    };
    let outcome = repo
        .commit(&CommitRequest::new(message.clone()).concluding(kind.operation()))
        .map_err(|e| match e {
            // Risolto com'era HEAD: niente da committare, e l'operazione resta in corso.
            EngineError::NothingToCommit => EngineError::NotAllowed(format!(
                "dopo la risoluzione il risultato è uguale a HEAD: niente da committare; \
                 per uscire usare `{}`",
                kind.abort_command()
            )),
            e => e,
        })?;
    let source = incoming.map_or_else(|| kind.head_file().to_owned(), |o| label(&o, &refs));
    match (&outcome.branch, outcome.previous) {
        (Some(branch), Some(previous)) => journal.record(JournalEntry::pick(
            id,
            kind,
            branch,
            previous,
            outcome.commit.id,
            HeadState::from_head(&status.head),
            &source,
            None,
        )),
        _ => match JournalEntry::commit(&outcome, false) {
            Some(entry) => journal.record(entry),
            None => 0,
        },
    };
    let mut out = to_json(&outcome);
    out["message"] = json!(message);
    finish(&mut out, repo, journal);
    Ok(out)
}

/// Annulla il cherry-pick o il revert in corso (`git cherry-pick --abort`, `git revert
/// --abort`): indice e worktree tornano come HEAD per i file dell'operazione. Chiede **sempre**
/// `"confirm": true` (senza: [`EngineError::ConfirmRequired`] con i file, niente cambia) e il
/// backend salva prima indice e worktree in `refs/project/backup/<cherry-pick|revert>-abort-...`
/// (come uno stash). Senza l'operazione `kind` in corso, o con una sequenza di più commit avviata
/// da terminale → [`EngineError::NotAllowed`]. Non entra nel diario.
///
/// Risultato: `{"backup", "files": [percorso], "warnings", "status", "refs", "can_undo",
/// "can_redo", "next_undo", "next_redo"}`.
fn pick_abort(s: &mut Session, p: Value, kind: PickKind) -> Out {
    let id = kind.abort_command();
    let AbortParams { confirm } = parse_params(id, p)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let state = repo.operation_state()?;
    check_picking(id, kind, &status, &state)?;
    if !confirm {
        return Err(abort_confirm(id, &status, kind.git(), kind.git()));
    }
    let aborted = repo.pick_abort(kind)?;
    let mut out = json!({
        "backup": aborted.backup,
        "files": to_json(&aborted.files),
        "warnings": to_json(&aborted.warnings),
    });
    finish(&mut out, repo, journal);
    Ok(out)
}

#[cfg(test)]
#[path = "pick_tests.rs"]
mod tests;
