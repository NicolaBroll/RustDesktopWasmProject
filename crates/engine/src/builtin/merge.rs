//! Merge e stato "in corso" (S29): `merge.start`, `merge.abort`, `merge.continue` e la lettura
//! `repo.operation` per il banner della UI. Tipi e controlli comuni ai backend in
//! `git/merge.rs`; contratto in `impl-docs/git/merge.md`.
//!
//! Un merge concluso (fast-forward o commit di merge, subito o con `merge.continue`) entra nel
//! diario e `edit.undo` lo annulla come un pull del branch attivo. Un merge fermo in conflitto
//! **resta in corso** e non entra nel diario (il branch non si è mosso); `merge.abort` chiede
//! sempre `"confirm": true` e prima salva indice e worktree in un backup.

use super::{Out, journal_state, to_json};
use crate::commands::parse_params;
use crate::{
    CommitRequest, EngineError, GitBackend, HeadState, JournalEntry, LogQuery, MergeMode,
    MergeRequest, Session, branch_ref, check_merge_state, check_merging, clean_merge_message,
    conflicted_paths, merge_source,
};
use project_domain::{Head, Oid, Operation, RefInfo, RefKind, RepoPath};
use serde::Deserialize;
use serde_json::{Value, json};

/// Parametri di `merge.start`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartParams {
    /// Cosa unire: un branch locale o remoto, un tag, un id o un'altra rev.
    rev: String,
    #[serde(default)]
    no_ff: bool,
    #[serde(default)]
    ff_only: bool,
    #[serde(default)]
    message: Option<String>,
}

/// Parametri di `merge.abort`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AbortParams {
    #[serde(default)]
    confirm: bool,
}

/// Parametri di `merge.continue`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinueParams {
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParams {}

/// Il messaggio facoltativo di `merge.start`/`merge.continue`: non vuoto e senza NUL (come
/// `commit.create`), altrimenti [`EngineError::InvalidParams`] con l'id del comando.
fn check_message(id: &'static str, message: &Option<String>) -> Result<(), EngineError> {
    let Some(message) = message else {
        return Ok(());
    };
    CommitRequest::new(message.clone())
        .check_message()
        .map_err(|e| match e {
            EngineError::InvalidParams { reason, .. } => EngineError::InvalidParams { id, reason },
            other => other,
        })
}

/// Aggiunge a `out` lo stato e i ref riletti (`null` con `*_error` se non si rileggono:
/// l'operazione è fatta e non va ripetuta) e lo stato del diario.
fn finish(out: &mut Value, repo: &dyn GitBackend, journal: &crate::Journal) {
    let state = journal_state(journal);
    for key in ["can_undo", "can_redo", "next_undo", "next_redo"] {
        out[key] = state[key].clone();
    }
    match repo.status() {
        Ok(status) => out["status"] = to_json(status),
        Err(e) => {
            out["status"] = Value::Null;
            out["status_error"] = Value::String(e.to_string());
        }
    }
    match repo.refs() {
        Ok(refs) => out["refs"] = to_json(refs),
        Err(e) => {
            out["refs"] = Value::Null;
            out["refs_error"] = Value::String(e.to_string());
        }
    }
}

/// Il nome breve del primo branch locale, branch remoto o tag che punta a `oid` (in
/// quest'ordine), altrimenti l'id abbreviato: per descrivere un commit in ingresso.
fn label(oid: &Oid, refs: &[RefInfo]) -> String {
    for (kind, prefix) in [
        (RefKind::LocalBranch, "refs/heads/"),
        (RefKind::RemoteBranch, "refs/remotes/"),
        (RefKind::Tag, "refs/tags/"),
    ] {
        if let Some(r) = refs.iter().find(|r| r.kind == kind && r.target == *oid) {
            return r.name.strip_prefix(prefix).unwrap_or(&r.name).to_owned();
        }
    }
    crate::git::short(oid)
}

/// Unisce `rev` nel branch attivo (`git merge`): fast-forward se il branch non ha commit che
/// `rev` non ha (e non c'è `no_ff`), altrimenti un commit di merge con `message` o il messaggio
/// di default di git (`Merge branch 'x'`, ...). `ff_only`: solo fast-forward (divergenti →
/// [`EngineError::NotAllowed`]); `no_ff` e `ff_only` insieme → [`EngineError::InvalidParams`].
///
/// Rifiutati prima di scrivere ([`EngineError::NotAllowed`], niente cambia): HEAD staccata o
/// branch senza commit, operazione in corso, file in conflitto, per un commit di merge qualunque
/// modifica ai file tracciati ([`check_merge_state`]); una rev che non c'è →
/// [`EngineError::UnknownRev`]. Niente da fare se il branch contiene già `rev` (`up_to_date`).
/// Prima di scrivere il backend salva il branch in `refs/project/backup/merge-...`.
///
/// **Conflitti**: il merge resta in corso (`in_progress`, `conflicts`), da concludere con
/// `merge.continue` dopo aver risolto i file (`index.stage`) o da annullare con `merge.abort`;
/// niente nel diario. Un merge concluso entra nel diario: `edit.undo` riporta il branch, con
/// indice e worktree, dov'era.
///
/// Risultato: `{"rev", "source": {"kind", "name", "full"}, "target", "branch", "strategy":
/// "fast_forward" | "merge" | "up_to_date", "before", "after", "ahead", "behind",
/// "in_progress", "conflicts": [percorso], "message", "backup": string | null, "warnings",
/// "status", "refs", "can_undo", "can_redo", "next_undo", "next_redo"}`.
pub(super) fn merge_start(s: &mut Session, p: Value) -> Out {
    const ID: &str = "merge.start";
    let StartParams {
        rev,
        no_ff,
        ff_only,
        message,
    } = parse_params(ID, p)?;
    if no_ff && ff_only {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: "`no_ff` (sempre un commit di merge) e `ff_only` (solo fast-forward) si \
                     escludono"
                .to_owned(),
        });
    }
    if rev.trim().is_empty() || rev.contains('\0') {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: "`rev` deve essere un nome di branch, di tag o un id, non vuoto".to_owned(),
        });
    }
    check_message(ID, &message)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let (branch, before) = match &status.head {
        Head::Branch {
            name,
            target: Some(target),
        } => (name.clone(), *target),
        Head::Branch { name, target: None } => {
            return Err(EngineError::NotAllowed(format!(
                "il branch `{}` non ha ancora commit: niente da unire, niente modificato",
                name.escape_debug()
            )));
        }
        Head::Detached(_) => {
            return Err(EngineError::NotAllowed(
                "HEAD è staccata: il merge va fatto su un branch, niente modificato. Passare a \
                 un branch (`branch.checkout`) o crearne uno qui (`branch.create`) e riprovare"
                    .to_owned(),
            ));
        }
    };
    // Prima di tutto il resto: un'operazione in corso o un conflitto fermano comunque.
    check_merge_state(&status, &MergeMode::FastForward)?;
    let target = repo
        .log(&LogQuery::page(0, 1).from_rev(rev.clone()))?
        .first()
        .map(|c| c.id)
        .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?;
    let refs = repo.refs()?;
    let source = merge_source(&rev, &refs, &target);
    let behind = repo.unmerged_count(&target, &[before])?;
    let ahead = repo.unmerged_count(&before, &[target])?;
    let mut out = json!({
        "rev": rev,
        "source": to_json(&source),
        "target": target.to_string(),
        "branch": branch,
        "before": before.to_string(),
        "ahead": ahead,
        "behind": behind,
        "in_progress": false,
        "conflicts": [],
        "message": "",
        "warnings": [],
    });
    if behind == 0 {
        out["strategy"] = json!("up_to_date");
        out["after"] = json!(before.to_string());
        out["backup"] = Value::Null;
        finish(&mut out, repo, journal);
        return Ok(out);
    }
    let mode = if ahead == 0 && !no_ff {
        MergeMode::FastForward
    } else if ff_only {
        return Err(EngineError::NotAllowed(format!(
            "`{}` e `{}` sono divergenti ({ahead} commit solo nel branch, {behind} solo in \
             `{0}`): con `ff_only` si può solo fare fast-forward, niente modificato",
            source.name.escape_debug(),
            branch.escape_debug()
        )));
    } else {
        MergeMode::Merge { no_ff }
    };
    check_merge_state(&status, &mode)?;
    let message = message.unwrap_or_else(|| source.message(&branch));
    let outcome = repo.merge(&MergeRequest {
        branch: branch.clone(),
        expected: before,
        target,
        target_name: source.name.clone(),
        mode,
        message,
    })?;
    let mut warnings = outcome.warnings.clone();
    // Nel diario solo se il risultato è quello del merge: il commit del fast-forward, o un
    // commit di merge sopra il branch di prima. Altrimenti un altro processo (o un hook) ha
    // spostato il branch nel frattempo, e un undo riporterebbe indietro anche il suo lavoro.
    let as_expected = !outcome.in_progress
        && outcome.after != before
        && match mode {
            MergeMode::FastForward => outcome.after == target,
            MergeMode::Merge { .. } => repo
                .log(&LogQuery::page(0, 1).from_rev(outcome.after.to_string()))
                .ok()
                .and_then(|c| c.first().map(|c| c.parents.first() == Some(&before)))
                .unwrap_or(false),
        };
    if !outcome.in_progress && outcome.after != before && !as_expected {
        warnings.push(format!(
            "il branch è a {} ma non è il risultato atteso del merge (un hook o un altro \
             processo l'ha spostato?): l'operazione non entra nel diario, il branch com'era è \
             in `{}`",
            outcome.after, outcome.backup
        ));
    }
    if as_expected {
        // Il branch è spostato: la voce va nel diario anche se dopo qualcosa non si rilegge.
        journal.record(JournalEntry::merge(
            ID,
            &branch_ref(&branch),
            before,
            outcome.after,
            HeadState::from_head(&status.head),
            mode.name(),
            &source.name,
            Some(outcome.backup.clone()),
        ));
    }
    out["strategy"] = json!(mode.name());
    out["after"] = json!(outcome.after.to_string());
    out["backup"] = json!(outcome.backup);
    out["in_progress"] = json!(outcome.in_progress);
    out["conflicts"] = to_json(&outcome.conflicts);
    out["message"] = json!(outcome.message);
    out["warnings"] = to_json(&warnings);
    finish(&mut out, repo, journal);
    Ok(out)
}

/// Annulla il merge in corso (`git merge --abort`): indice e worktree tornano come HEAD per i
/// file del merge, e le risoluzioni fatte nei file in conflitto, le modifiche in stage e quelle
/// fatte dopo l'inizio del merge andrebbero perse. Per questo chiede **sempre** `"confirm":
/// true` (senza: [`EngineError::ConfirmRequired`] con i file, niente cambia) e il backend salva
/// prima indice e worktree in `refs/project/backup/merge-abort-...` (come uno stash: `git stash
/// apply <ref>` li rimette). Senza un merge in corso → [`EngineError::NotAllowed`]. Non entra
/// nel diario (il recupero è il backup).
///
/// Risultato: `{"backup", "files": [percorso], "warnings", "status", "refs", "can_undo",
/// "can_redo", "next_undo", "next_redo"}`.
pub(super) fn merge_abort(s: &mut Session, p: Value) -> Out {
    const ID: &str = "merge.abort";
    let AbortParams { confirm } = parse_params(ID, p)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    check_merging(ID, &status)?;
    if !confirm {
        let conflicts = conflicted_paths(&status);
        let changed: Vec<String> = status
            .entries
            .iter()
            .filter(|e| e.is_staged() || e.is_unstaged())
            .map(|e| format!("`{}`", e.path))
            .collect();
        let mut what = Vec::new();
        if !conflicts.is_empty() {
            what.push(format!(
                "i file in conflitto ({}) tornerebbero come in HEAD, con le risoluzioni fatte \
                 finora",
                conflicts
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !changed.is_empty() {
            what.push(format!(
                "le modifiche del merge e quelle fatte dopo, in stage o no, andrebbero perse \
                 ({})",
                changed.join(", ")
            ));
        }
        if what.is_empty() {
            what.push("il merge in corso verrebbe annullato".to_owned());
        }
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: format!(
                "{}. Prima si salverebbero indice e worktree in \
                 `refs/project/backup/merge-abort-...` (si rimettono con `git stash apply \
                 <ref>`); l'annullamento non entra nel diario",
                what.join("; ")
            ),
            expected: None,
        });
    }
    let aborted = repo.merge_abort()?;
    let mut out = json!({
        "backup": aborted.backup,
        "files": to_json(&aborted.files),
        "warnings": to_json(&aborted.warnings),
    });
    finish(&mut out, repo, journal);
    Ok(out)
}

/// Conclude il merge in corso con il commit di merge (genitori HEAD e `MERGE_HEAD`, `git commit`
/// come `commit.create`: hook e firma secondo la configurazione), con `message` o quello
/// preparato da git (`MERGE_MSG`, senza il blocco `# Conflicts:`). Rifiutato
/// ([`EngineError::NotAllowed`], niente cambia) senza un merge in corso o se restano file in
/// conflitto (elencati: vanno risolti e messi in stage con `index.stage`). Entra nel diario come
/// un merge concluso; con HEAD staccata (un merge iniziato da terminale) come un commit.
///
/// Risultato: `{"commit": Commit, "previous", "branch", "message", "status", "refs",
/// "can_undo", "can_redo", "next_undo", "next_redo"}`.
pub(super) fn merge_continue(s: &mut Session, p: Value) -> Out {
    const ID: &str = "merge.continue";
    let ContinueParams { message } = parse_params(ID, p)?;
    check_message(ID, &message)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    check_merging(ID, &status)?;
    let conflicts = conflicted_paths(&status);
    if !conflicts.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "restano file in conflitto ({}): risolverli e metterli in stage (`index.stage`), \
             poi ripetere `merge.continue`; niente committato",
            conflicts
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let state = repo.operation_state()?;
    let refs = repo.refs()?;
    let sources: Vec<String> = state.incoming.iter().map(|o| label(o, &refs)).collect();
    let message = match message {
        Some(message) => message,
        None => state
            .message
            .as_deref()
            .map(clean_merge_message)
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| {
                format!(
                    "Merge {}",
                    state
                        .incoming
                        .iter()
                        .map(|o| format!("commit '{o}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
    };
    let outcome = repo.commit(&CommitRequest::new(message.clone()))?;
    match (&outcome.branch, outcome.previous) {
        (Some(branch), Some(previous)) => journal.record(JournalEntry::merge(
            ID,
            branch,
            previous,
            outcome.commit.id,
            HeadState::from_head(&status.head),
            "merge",
            &if sources.is_empty() {
                "MERGE_HEAD".to_owned()
            } else {
                sources.join(", ")
            },
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

/// L'operazione in corso, per il banner della UI: `{"operation": "Merge" | "Rebase" |
/// "CherryPick" | "Revert" | "Bisect" | "Am" | null, "head": Head, "conflicts": [percorso],
/// "incoming": [{"id", "label", "summary": string | null, "refs": [nome completo]}], "message":
/// string | null, "onto": oid | null, "rebase_branch": string | null, "ready": bool,
/// "continue": id | null, "abort": id | null}`.
///
/// `incoming`: i commit in ingresso (`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`) con i ref
/// che ci puntano; `message`: il messaggio che `merge.continue` userebbe (`MERGE_MSG` senza il
/// blocco dei conflitti); `ready`: niente più file in conflitto; `continue`/`abort`: i comandi
/// che concludono o annullano l'operazione da qui (per ora solo il merge; `null` altrimenti).
pub(super) fn repo_operation(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("repo.operation", p)?;
    let repo = s.repo()?;
    let status = repo.status()?;
    let state = repo.operation_state()?;
    let refs = repo.refs()?;
    let conflicts: Vec<RepoPath> = conflicted_paths(&status);
    let incoming: Vec<Value> = state
        .incoming
        .iter()
        .map(|oid| {
            let summary = repo
                .log(&LogQuery::page(0, 1).from_rev(oid.to_string()))
                .ok()
                .and_then(|c| c.first().map(|c| c.summary.clone()));
            let names: Vec<&str> = refs
                .iter()
                .filter(|r| r.target == *oid && r.kind != RefKind::Stash)
                .map(|r| r.name.as_str())
                .collect();
            json!({
                "id": oid.to_string(),
                "label": label(oid, &refs),
                "summary": summary,
                "refs": names,
            })
        })
        .collect();
    let operation = status.operation;
    let merging = operation == Some(Operation::Merge);
    Ok(json!({
        "operation": to_json(operation),
        "head": to_json(&status.head),
        "conflicts": to_json(&conflicts),
        "incoming": incoming,
        "message": state.message.as_deref().map(clean_merge_message),
        "onto": state.onto.map(|o| o.to_string()),
        "rebase_branch": state.rebase_branch,
        "ready": operation.is_some() && conflicts.is_empty(),
        "continue": merging.then_some("merge.continue"),
        "abort": merging.then_some("merge.abort"),
    }))
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
