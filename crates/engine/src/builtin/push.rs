//! `remote.push` (S26): pubblica un branch locale sul remoto (`impl-docs/ARCHITETTURA-git.md`
//! §4–§7).

use super::remote::{changes, unexpected};
use super::{Out, to_json};
use crate::commands::parse_params;
use crate::git::short;
use crate::{
    EngineError, PushRequest, Session, branch_ref, check_branch_name, check_remote_name,
    default_remote, tracking_ref, unknown_remote,
};
use project_domain::{Head, Oid};
use serde::Deserialize;
use serde_json::{Value, json};

const ID: &str = "remote.push";

/// Commit elencati al più nella conferma di un force.
const LOST_LIMIT: usize = 10;

/// Parametri di `remote.push`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PushParams {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    expected: Option<String>,
}

/// Un valore booleano vero di git config.
fn config_true(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "" | "true" | "yes" | "on" | "1"
    )
}

/// Pubblica il branch `branch` (o quello di HEAD) sul remoto, con un refspec esplicito
/// `refs/heads/<branch>:refs/heads/<branch>`: mai tag né altri branch, qualunque sia
/// `push.default`, `remote.<nome>.push` o `push.followTags`.
///
/// Remoto, come `git push`: `remote`, poi `branch.<nome>.pushRemote`, `remote.pushDefault`, il
/// remoto dell'upstream, `origin` o l'unico remoto (più remoti senza `origin`:
/// [`EngineError::NotAllowed`] che chiede `remote`); `remote_source` nel risultato dice quale.
/// Un remoto della configurazione che non è configurato, o un remoto mirror
/// (`remote.<nome>.mirror`) → [`EngineError::NotAllowed`]. Destinazione: sul remoto
/// dell'upstream il branch dell'upstream (con un nome diverso dal branch è rifiutato, come
/// `push.default=simple`), altrove lo stesso nome, con il branch remoto locale ricavato dai
/// refspec di fetch ([`tracking_ref`]). **Primo push** (nessun upstream): l'upstream si imposta
/// (`--set-upstream`) se il push riesce; un upstream esistente non cambia mai.
///
/// Force: solo con `"force": true`, e sempre come `--force-with-lease=refs/heads/<b>:<oid>`
/// (mai `--force` né `+refspec`): se il remoto si è spostato nel frattempo il push è
/// rifiutato. Se il force toglierebbe dal remoto commit che il branch locale non ha serve anche
/// `"confirm": true` con `"expected"`, l'id completo del branch remoto locale mostrato dalla
/// conferma ([`EngineError::ConfirmRequired`] con i commit e la frase `"expected": "<id>"`);
/// se `expected` non è il valore letto ora, di nuovo `ConfirmRequired` e niente pubblicato. Il
/// lease usa `expected` (o il valore letto ora). Prima del push il branch remoto locale si salva
/// in `refs/project/backup/push-...`. `confirm` senza `expected`, `confirm`/`expected` senza
/// `force` → [`EngineError::InvalidParams`].
///
/// Non va nel diario: un push non si annulla da qui (il remoto è di altri); il risultato dice
/// cosa è cambiato. Comando lento come il fetch.
///
/// Risultato: `{"remote", "remote_source": "param" | "push_remote" | "push_default" |
/// "upstream" | "origin" | "only", "branch", "remote_branch", "kind": "new" | "fast_forward" | "forced"
/// | "up_to_date", "summary", "before": oid | null, "after": oid, "set_upstream": bool,
/// "forced": bool, "lost": [Commit], "backup": string | null, "updated": [{"name", "before",
/// "after"}] | null, "messages": [string], "warnings": [string], "status": Status | null,
/// "refs": [RefInfo] | null}`: `before` è il branch remoto locale prima del push, `after` il
/// commit pubblicato, `lost` i commit tolti dal remoto da un force, `updated` i ref locali
/// cambiati (il branch remoto locale).
pub(super) fn remote_push(s: &mut Session, p: Value) -> Out {
    let PushParams {
        remote,
        branch,
        force,
        confirm,
        expected,
    } = parse_params(ID, p)?;
    if let Some(remote) = &remote {
        check_remote_name(ID, remote)?;
    }
    if let Some(branch) = &branch {
        check_branch_name(ID, branch)?;
    }
    if (confirm || expected.is_some()) && !force {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: "`confirm` ed `expected` servono solo con `\"force\": true`".to_owned(),
        });
    }
    if confirm && expected.is_none() {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: "con `\"confirm\": true` serve `expected`: l'id completo del branch remoto \
                     locale mostrato nella conferma (la conferma vale solo per quel commit)"
                .to_owned(),
        });
    }
    let expected = expected
        .map(|hex| {
            let full = hex.len() == Oid::HEX_LEN && hex.bytes().all(|b| b.is_ascii_hexdigit());
            full.then(|| hex.to_ascii_lowercase().parse::<Oid>().ok())
                .flatten()
                .ok_or_else(|| EngineError::InvalidParams {
                    id: ID,
                    reason: format!(
                        "`expected` deve essere l'id completo ({} cifre esadecimali) del branch \
                         remoto locale mostrato nella conferma: `{}`",
                        Oid::HEX_LEN,
                        hex.escape_debug()
                    ),
                })
        })
        .transpose()?;
    let job = s.job().clone();
    let repo = s.repo_mut()?;
    let status = repo.status()?;
    let branch = match branch {
        Some(branch) => branch,
        None => match &status.head {
            Head::Branch { name, .. } => name.clone(),
            Head::Detached(_) => {
                return Err(EngineError::NotAllowed(
                    "HEAD è staccata: niente pubblicato. Passare a un branch \
                     (`branch.checkout`) o indicare `branch`"
                        .to_owned(),
                ));
            }
        },
    };
    let full = branch_ref(&branch);
    let refs_before = repo.refs()?;
    let local = refs_before
        .iter()
        .find(|r| r.name == full)
        .map(|r| r.target)
        .ok_or_else(|| {
            EngineError::NotAllowed(format!(
                "non esiste un branch locale `{}` con commit: niente pubblicato",
                branch.escape_debug()
            ))
        })?;
    let remotes = repo.remotes()?;
    let upstream = repo.upstream(&branch)?;

    // Il remoto, come lo sceglie `git push`: `remote`, poi `branch.<nome>.pushRemote`, poi
    // `remote.pushDefault`, poi il remoto dell'upstream, poi `origin` o l'unico remoto.
    let last = |values: Vec<String>| values.into_iter().last();
    let (remote, remote_source) = match remote {
        Some(r) => (r, "param"),
        None => {
            if let Some(r) = last(repo.config_values(&format!("branch.{branch}.pushRemote"))?) {
                (r, "push_remote")
            } else if let Some(r) = last(repo.config_values("remote.pushDefault")?) {
                (r, "push_default")
            } else {
                match &upstream {
                    Some(up) if up.remote == "." => {
                        return Err(EngineError::NotAllowed(format!(
                            "l'upstream di `{}` è un branch locale (`{}`): indicare `remote`, \
                             niente pubblicato",
                            branch.escape_debug(),
                            up.merge
                        )));
                    }
                    Some(up) => (up.remote.clone(), "upstream"),
                    None => {
                        let without = project_domain::Status {
                            upstream: None,
                            ..status.clone()
                        };
                        let r = default_remote(ID, &without, &remotes)?;
                        let source = if r == "origin" { "origin" } else { "only" };
                        (r, source)
                    }
                }
            }
        }
    };
    if remote_source == "push_remote" || remote_source == "push_default" {
        let key = if remote_source == "push_remote" {
            format!("branch.{branch}.pushRemote")
        } else {
            "remote.pushDefault".to_owned()
        };
        if check_remote_name(ID, &remote).is_err() || !remotes.contains(&remote) {
            return Err(EngineError::NotAllowed(format!(
                "`{key}` indica `{}`, che non è un remoto configurato ({}): niente pubblicato; \
                 correggere la configurazione o indicare `remote`",
                remote.escape_debug(),
                remotes
                    .iter()
                    .map(|r| format!("`{r}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    if !remotes.contains(&remote) {
        return Err(unknown_remote(&remote, &remotes));
    }
    let mirror = last(repo.config_values(&format!("remote.{remote}.mirror"))?);
    if mirror.as_deref().is_some_and(config_true) {
        return Err(EngineError::NotAllowed(format!(
            "il remoto `{}` è un mirror (`remote.{0}.mirror=true`): git ne pubblicherebbe tutti \
             i ref insieme, e qui si pubblica solo un branch. Niente pubblicato; usare git da \
             terminale per il mirror",
            remote.escape_debug()
        )));
    }
    // Branch sul remoto, se impostare l'upstream e il branch remoto locale che lo rispecchia.
    let (remote_branch, set_upstream, tracking) = match &upstream {
        Some(up) if up.remote == remote => {
            let target = up.merge_branch().ok_or_else(|| {
                EngineError::NotAllowed(format!(
                    "l'upstream di `{}` non è un branch (`{}`): niente pubblicato",
                    branch.escape_debug(),
                    up.merge
                ))
            })?;
            (target.to_owned(), false, up.tracking.clone())
        }
        _ => {
            let refspecs = repo.config_values(&format!("remote.{remote}.fetch"))?;
            (
                branch.clone(),
                upstream.is_none(),
                tracking_ref(&refspecs, &full),
            )
        }
    };
    if remote_branch != branch {
        return Err(EngineError::NotAllowed(format!(
            "l'upstream di `{}` è `{}/{}`, con un nome diverso: per sicurezza (come \
             `push.default=simple`) non si pubblica su un altro branch, niente pubblicato. \
             Cambiare l'upstream o rinominare il branch",
            branch.escape_debug(),
            remote.escape_debug(),
            remote_branch.escape_debug()
        )));
    }
    let remote_value = tracking
        .as_ref()
        .and_then(|t| refs_before.iter().find(|r| r.name == *t))
        .map(|r| r.target);

    // I commit del remoto (come li conosce il branch remoto locale) che un force toglierebbe.
    let (lost, lost_count) = match (force, remote_value) {
        (true, Some(theirs)) => {
            let count = repo.unmerged_count(&theirs, &[local])?;
            let commits = if count > 0 {
                repo.missing_commits(&theirs, &[local], LOST_LIMIT)?
            } else {
                Vec::new()
            };
            (commits, count)
        }
        _ => (Vec::new(), 0),
    };
    let shown = tracking
        .as_deref()
        .unwrap_or("(nessun branch remoto locale)");
    let describe_lost = || {
        let mut names: Vec<String> = lost
            .iter()
            .map(|c| format!("{} «{}»", short(&c.id), c.summary))
            .collect();
        if lost_count > lost.len() {
            names.push(format!("e altri {}", lost_count - lost.len()));
        }
        names.join(", ")
    };
    // La conferma vale solo per il commit visto: `expected` dev'essere il branch remoto locale
    // letto ora.
    if let Some(expected) = expected
        && Some(expected) != remote_value
    {
        let now = remote_value.map_or_else(|| "assente".to_owned(), |o| o.to_string());
        let confirm_with = remote_value.map_or_else(String::new, |o| {
            format!("; per confermare: \"confirm\": true, \"expected\": \"{o}\"")
        });
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: format!(
                "`{shown}` si è spostato da quando è stato visto (atteso {expected}, ora {now}): \
                 il force non è più quello confermato. Il remoto perderebbe {lost_count} \
                 commit che `{branch}` non ha ({}){confirm_with}",
                describe_lost()
            ),
            expected: remote_value.map(|o| o.to_string()),
        });
    }
    if force && lost_count > 0 && !confirm {
        let theirs = remote_value.expect("lost_count > 0 solo con un branch remoto locale");
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: format!(
                "il force sovrascriverebbe `{remote}/{remote_branch}` ({shown} = {theirs}): il \
                 remoto perderebbe {lost_count} commit che `{branch}` non ha ({}). Prima si \
                 salverebbe `{shown}` in `refs/project/backup/push-...`; con \
                 `--force-with-lease`, se nel frattempo il remoto si è spostato il push è \
                 rifiutato; per confermare: \"confirm\": true, \"expected\": \"{theirs}\"",
                describe_lost()
            ),
            expected: Some(theirs.to_string()),
        });
    }
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    let request = PushRequest {
        remote: remote.clone(),
        branch: branch.clone(),
        remote_branch: remote_branch.clone(),
        set_upstream,
        lease: force.then_some(expected.or(remote_value)),
        backup: (force && lost_count > 0).then_some(remote_value).flatten(),
    };
    let outcome = repo.push(&request, &job)?;
    let mut out = json!({
        "remote": remote,
        "remote_source": remote_source,
        "branch": branch,
        "remote_branch": remote_branch,
        "kind": to_json(outcome.kind),
        "summary": outcome.summary,
        "before": remote_value.map(|o| o.to_string()),
        "after": local.to_string(),
        "set_upstream": set_upstream,
        "forced": force,
        "lost": to_json(&lost),
        "backup": outcome.backup,
        "messages": outcome.messages,
    });
    match repo.refs() {
        Ok(after) => {
            let updated = changes(&refs_before, &after);
            let warnings: Vec<String> = updated.iter().filter_map(unexpected).collect();
            out["updated"] = to_json(updated);
            out["warnings"] = to_json(warnings);
            out["refs"] = to_json(after);
        }
        Err(e) => {
            out["updated"] = Value::Null;
            out["warnings"] = json!([]);
            out["refs"] = Value::Null;
            out["refs_error"] = Value::String(e.to_string());
        }
    }
    match repo.status() {
        Ok(status) => out["status"] = to_json(status),
        Err(e) => {
            out["status"] = Value::Null;
            out["status_error"] = Value::String(e.to_string());
        }
    }
    Ok(out)
}
