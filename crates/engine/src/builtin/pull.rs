//! `remote.pull` (S26): fetch del branch dell'upstream, poi fast-forward, merge o rebase in
//! locale, guidati dal motore (`impl-docs/ARCHITETTURA-git.md` §4, §6, §7).
//!
//! Perché non `git pull`: con due passi separati il comando sa cosa è arrivato prima di
//! scrivere, decide la strategia con la configurazione dell'utente, controlla lo stato del repo
//! (niente `--autostash`, niente merge o rebase con modifiche locali), salva il backup, e il
//! fetch resta annullabile mentre l'integrazione non si interrompe a metà.

use super::remote::{changes, unexpected};
use super::{Out, journal_state, to_json};
use crate::commands::parse_params;
use crate::{
    EngineError, FetchRequest, HeadState, JournalEntry, PullMode, PullPolicy, PullRequest,
    PullStrategy, Session, StrategySource, branch_ref, check_branch_name, check_pull_state,
    check_remote_name, merge_message, pull_mode, unknown_remote,
};
use project_domain::Head;
use serde::Deserialize;
use serde_json::{Value, json};

const ID: &str = "remote.pull";

/// Parametri di `remote.pull`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PullParams {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    strategy: Option<PullStrategy>,
}

/// [`EngineError::NotAllowed`] per un branch senza upstream, con come impostarlo.
fn no_upstream(branch: &str) -> EngineError {
    let b = branch.escape_debug();
    EngineError::NotAllowed(format!(
        "il branch `{b}` non ha un upstream (`branch.{b}.remote`/`branch.{b}.merge`): niente \
         scaricato né modificato. Impostarlo da terminale con `git branch \
         --set-upstream-to=<remoto>/<branch> -- {b}`, oppure pubblicare il branch con \
         `remote.push`, che al primo push imposta l'upstream"
    ))
}

/// Scarica l'upstream del branch (`branch`, o quello di HEAD) e lo integra: fast-forward,
/// merge o rebase secondo la configurazione (`branch.<nome>.rebase`, `pull.rebase`, `pull.ff`)
/// o il parametro `strategy`; senza configurazione merge (vedi
/// [`PullSettings::policy`](crate::PullSettings::policy)). Un branch che non è quello di HEAD
/// si può solo portare avanti (fast-forward del ref, indice e worktree non si toccano).
///
/// In quest'ordine, e niente cambia nel branch se un passo fallisce:
///
/// 1. HEAD staccata senza `branch`, branch senza commit, operazione in corso, file in
///    conflitto, branch senza upstream (o con upstream locale `.`), `remote` diverso da quello
///    dell'upstream, configurazione non valida → [`EngineError::NotAllowed`], niente scaricato;
/// 2. fetch del solo branch dell'upstream ([`GitBackend::fetch`](crate::GitBackend::fetch):
///    avanzamento e annullamento dal [`JobContext`](crate::JobContext), errori `Auth`,
///    `RemoteUnreachable`, `Cancelled`); un annullamento chiesto durante il fetch ferma il
///    comando anche se il fetch è finito, prima di integrare;
/// 3. la strategia ([`pull_mode`]): niente da fare se il branch contiene già l'upstream;
/// 4. l'integrazione ([`GitBackend::pull`](crate::GitBackend::pull)): controlli sullo stato
///    ([`check_pull_state`]), backup del branch, poi merge/rebase; un conflitto annulla
///    l'operazione e dà [`EngineError::PullConflict`].
///
/// Lo spostamento del branch va nel diario: `edit.undo` lo riporta dov'era (con indice e
/// worktree, come `git reset --keep`, se è il branch attivo). Il fetch no (§7).
///
/// Risultato: `{"branch", "remote", "upstream", "strategy": "fast_forward" | "merge" | "rebase"
/// | "up_to_date", "strategy_source": "param" | "config" | "default", "before", "after",
/// "ahead", "behind", "backup": string | null, "fetch": {"updated", "messages"}, "warnings",
/// "status": Status | null, "refs": [RefInfo] | null, "can_undo", "can_redo", "next_undo",
/// "next_redo"}`.
pub(super) fn remote_pull(s: &mut Session, p: Value) -> Out {
    let PullParams {
        remote,
        branch,
        strategy,
    } = parse_params(ID, p)?;
    if let Some(remote) = &remote {
        check_remote_name(ID, remote)?;
    }
    if let Some(branch) = &branch {
        check_branch_name(ID, branch)?;
    }
    let job = s.job().clone();
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let head_branch = match &status.head {
        Head::Branch { name, .. } => Some(name.clone()),
        Head::Detached(_) => None,
    };
    let branch = match branch {
        Some(branch) => branch,
        None => head_branch.clone().ok_or_else(|| {
            EngineError::NotAllowed(
                "HEAD è staccata: niente scaricato. Passare a un branch (`branch.checkout`) o \
                 indicare `branch`"
                    .to_owned(),
            )
        })?,
    };
    let current = head_branch.as_deref() == Some(branch.as_str());
    let full = branch_ref(&branch);
    let refs = repo.refs()?;
    let before = refs
        .iter()
        .find(|r| r.name == full)
        .map(|r| r.target)
        .ok_or_else(|| {
            EngineError::NotAllowed(format!(
                "non esiste un branch locale `{}` con commit: niente scaricato",
                branch.escape_debug()
            ))
        })?;
    // Prima di scaricare: un'operazione in corso o un conflitto fermerebbero comunque il pull.
    check_pull_state(&status, &PullMode::FastForward)?;

    let upstream = repo
        .upstream(&branch)?
        .ok_or_else(|| no_upstream(&branch))?;
    if upstream.remote == "." {
        return Err(EngineError::NotAllowed(format!(
            "l'upstream di `{}` è un branch locale (`{}`): il pull da un upstream locale non è \
             supportato, niente modificato",
            branch.escape_debug(),
            upstream.merge
        )));
    }
    if let Some(remote) = &remote
        && *remote != upstream.remote
    {
        return Err(EngineError::NotAllowed(format!(
            "`{}` segue il remoto `{}`, non `{}`: il pull integra sempre l'upstream. Niente \
             scaricato; per seguire un altro remoto cambiare l'upstream",
            branch.escape_debug(),
            upstream.remote.escape_debug(),
            remote.escape_debug()
        )));
    }
    let remotes = repo.remotes()?;
    if !remotes.contains(&upstream.remote) {
        return Err(unknown_remote(&upstream.remote, &remotes));
    }
    let merge_branch = upstream.merge_branch().ok_or_else(|| {
        EngineError::NotAllowed(format!(
            "l'upstream di `{}` non è un branch (`{}`): niente scaricato",
            branch.escape_debug(),
            upstream.merge
        ))
    })?;
    let tracking = upstream.tracking.clone().ok_or_else(|| {
        EngineError::NotAllowed(format!(
            "nessun refspec di fetch del remoto `{}` scarica `{}` in un branch remoto locale \
             (`remote.{0}.fetch`): niente scaricato",
            upstream.remote.escape_debug(),
            upstream.merge
        ))
    })?;
    // La strategia si legge prima del fetch: una configurazione non valida non scarica niente.
    let policy = if current {
        repo.pull_settings(&branch)?.policy(strategy)?
    } else {
        PullPolicy {
            strategy: PullStrategy::Merge,
            no_ff: false,
            rebase_merges: false,
            source: StrategySource::Default,
        }
    };

    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    let fetched = repo
        .fetch(
            &FetchRequest {
                remote: upstream.remote.clone(),
                branch: Some(merge_branch.to_owned()),
            },
            &job,
        )
        .map_err(|e| match e {
            EngineError::Cancelled { .. } => EngineError::Cancelled { command: ID },
            other => other,
        })?;
    let after_fetch = repo.refs()?;
    let updated = changes(&refs, &after_fetch);
    let mut warnings: Vec<String> = updated.iter().filter_map(unexpected).collect();
    let fetch = json!({ "updated": to_json(&updated), "messages": fetched.messages });
    let upstream_oid = after_fetch
        .iter()
        .find(|r| r.name == tracking)
        .map(|r| r.target)
        .ok_or_else(|| {
            EngineError::NotAllowed(format!(
                "dopo il fetch `{tracking}` non c'è: niente da integrare, il branch non è stato \
                 toccato"
            ))
        })?;
    // Annullato durante il fetch: niente integrazione.
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }

    let ahead = repo.unmerged_count(&before, &[upstream_oid])?;
    let behind = repo.unmerged_count(&upstream_oid, &[before])?;
    let mode = pull_mode(&policy, &branch, current, ahead, behind)?;
    let mut out = json!({
        "branch": branch,
        "remote": upstream.remote,
        "upstream": tracking,
        "strategy_source": to_json(policy.source),
        "before": before.to_string(),
        "ahead": ahead,
        "behind": behind,
        "fetch": fetch,
    });
    let (after, backup) = match mode {
        None => {
            out["strategy"] = json!("up_to_date");
            (before, None)
        }
        Some(mode) => {
            out["strategy"] = json!(mode.name());
            let outcome = repo.pull(&PullRequest {
                branch: branch.clone(),
                current,
                expected: before,
                upstream: upstream_oid,
                upstream_name: tracking.clone(),
                mode,
                message: merge_message(&tracking, &branch),
            })?;
            // Il branch è spostato: la voce va nel diario anche se dopo qualcosa non si rilegge.
            if outcome.after != before {
                journal.record(JournalEntry::pull(
                    &full,
                    before,
                    outcome.after,
                    HeadState::from_head(&status.head),
                    mode.name(),
                    tracking.strip_prefix("refs/remotes/").unwrap_or(&tracking),
                    outcome.backup.clone(),
                ));
            }
            warnings.extend(outcome.warnings);
            (outcome.after, Some(outcome.backup))
        }
    };
    let state = journal_state(journal);
    out["after"] = json!(after.to_string());
    out["backup"] = json!(backup);
    out["warnings"] = to_json(warnings);
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
    Ok(out)
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
