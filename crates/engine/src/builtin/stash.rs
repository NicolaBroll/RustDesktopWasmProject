//! Comandi degli stash (S28): `stash.list`, `stash.push`, `stash.apply`, `stash.pop`,
//! `stash.drop`. Tipi, controlli comuni ai backend e scelte in `git/stash.rs`; qui i comandi.
//! `stash.push` e `stash.drop` entrano nel diario, `stash.apply` e `stash.pop` no (§7).

use super::{Out, check_path_list, journal_state, to_json};
use crate::commands::parse_params;
use crate::{
    EngineError, GitBackend, HeadState, JournalEntry, Session, StashEntry, StashPush,
    check_stash_apply, pop_stash,
};
use project_domain::Oid;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {}

/// Parametri di `stash.apply` e `stash.pop`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyParams {
    /// La posizione: `n` di `stash@{n}` (0 se manca).
    #[serde(default)]
    index: usize,
    /// L'id completo dello stash visto da chi chiama.
    #[serde(default)]
    expected: Option<String>,
    #[serde(default)]
    restore_index: bool,
}

/// Parametri di `stash.drop`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DropParams {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    expected: Option<String>,
    #[serde(default)]
    confirm: bool,
}

/// L'elenco come JSON, o `null` con `stashes_error` se non si rilegge (l'operazione è fatta).
fn with_stashes(out: &mut Value, repo: &dyn GitBackend) {
    match repo.stashes() {
        Ok(list) => out["stashes"] = to_json(list),
        Err(e) => {
            out["stashes"] = Value::Null;
            out["stashes_error"] = Value::String(e.to_string());
        }
    }
}

/// Lo stato come JSON, o `null` con `status_error` se non si rilegge.
fn with_status(out: &mut Value, repo: &dyn GitBackend) {
    match repo.status() {
        Ok(status) => out["status"] = to_json(status),
        Err(e) => {
            out["status"] = Value::Null;
            out["status_error"] = Value::String(e.to_string());
        }
    }
}

/// `expected` come id completo (40 cifre), o [`EngineError::InvalidParams`].
fn full_id(id: &'static str, hex: Option<String>) -> Result<Option<Oid>, EngineError> {
    hex.map(|hex| {
        let full = hex.len() == Oid::HEX_LEN && hex.bytes().all(|b| b.is_ascii_hexdigit());
        full.then(|| hex.to_ascii_lowercase().parse::<Oid>().ok())
            .flatten()
            .ok_or_else(|| EngineError::InvalidParams {
                id,
                reason: format!(
                    "`expected` deve essere l'id completo ({} cifre esadecimali) dello stash: \
                     `{}`",
                    Oid::HEX_LEN,
                    hex.escape_debug()
                ),
            })
    })
    .transpose()
}

/// Lo stash in posizione `index`, che deve essere `expected` se dato: altrimenti
/// [`EngineError::NotAllowed`] (non c'è, o l'elenco è cambiato da quando è stato visto).
fn pick(
    repo: &dyn GitBackend,
    index: usize,
    expected: Option<Oid>,
) -> Result<StashEntry, EngineError> {
    let list = repo.stashes()?;
    let Some(entry) = list.get(index) else {
        return Err(EngineError::NotAllowed(match list.len() {
            0 => "non ci sono stash, niente modificato".to_owned(),
            n => format!(
                "non c'è `stash@{{{index}}}`: gli stash sono {n} (da `stash@{{0}}` a \
                 `stash@{{{}}}`), niente modificato",
                n - 1
            ),
        }));
    };
    if let Some(seen) = expected
        && seen != entry.id
    {
        let now = match list.iter().find(|s| s.id == seen) {
            Some(s) => format!("ora è `{}`", s.name),
            None => "non è più nell'elenco".to_owned(),
        };
        return Err(EngineError::NotAllowed(format!(
            "l'elenco degli stash è cambiato da quando è stato visto: `stash@{{{index}}}` è {}, \
             lo stash {seen} {now}. Niente modificato; rileggere gli stash (`stash.list`) e \
             riprovare",
            entry.id
        )));
    }
    Ok(entry.clone())
}

/// L'elenco degli stash, dal più recente: `{"stashes": [StashEntry]}` (vedi
/// [`StashEntry`]: `index`, `name`, `id`, `base`, `message`, `branch`, `time`, `untracked`).
pub(super) fn stash_list(s: &mut Session, p: Value) -> Out {
    let ListParams {} = parse_params("stash.list", p)?;
    Ok(json!({ "stashes": to_json(s.repo()?.stashes()?) }))
}

/// Mette da parte le modifiche in un nuovo stash (vedi
/// [`GitBackend::stash_push`](crate::GitBackend::stash_push) e
/// [`check_stash_push`](crate::check_stash_push)): niente si perde, le modifiche sono nello
/// stash. Va nel diario: `edit.undo` lo riapplica (con l'indice) e lo toglie dall'elenco, solo
/// se HEAD non si è spostata.
///
/// Risultato: `{"stash": StashEntry, "stashes": [StashEntry], "status": Status, "can_undo",
/// "can_redo", "next_undo", "next_redo"}`.
pub(super) fn stash_push(s: &mut Session, p: Value) -> Out {
    const ID: &str = "stash.push";
    let mut request: StashPush = parse_params(ID, p)?;
    request.check()?;
    if let Some(paths) = request.paths.take() {
        request.paths = Some(check_path_list(ID, paths)?);
    }
    let (repo, journal) = s.repo_and_journal()?;
    let head = HeadState::from_head(&repo.head()?);
    let stash = repo.stash_push(&request)?;
    journal.record(JournalEntry::stash_push(&stash, head, request));
    let mut out = journal_state(journal);
    out["stash"] = to_json(stash);
    with_stashes(&mut out, repo);
    with_status(&mut out, repo);
    Ok(out)
}

/// `stash.apply` e `stash.pop`: applica lo stash `index` (controllato con `expected`) e, per
/// `pop`, se non restano conflitti lo toglie dall'elenco dopo averne salvato il commit in
/// `refs/project/backup/stash-...`. Non entrano nel diario.
fn apply(id: &'static str, pop: bool, s: &mut Session, p: Value) -> Out {
    let ApplyParams {
        index,
        expected,
        restore_index,
    } = parse_params(id, p)?;
    let expected = full_id(id, expected)?;
    let repo = s.repo_mut()?;
    let entry = pick(repo, index, expected)?;
    check_stash_apply(id, &repo.status()?)?;
    let (applied, dropped, drop_error) = if pop {
        let popped = pop_stash(repo, id, entry.id, restore_index, Some("stash"))?;
        (popped.applied, popped.dropped, popped.drop_error)
    } else {
        let applied = repo.stash_apply(&crate::StashApply {
            command: id,
            id: entry.id,
            restore_index,
        })?;
        (applied, None, None)
    };
    let was_dropped = dropped.is_some();
    let mut warnings = applied.warnings;
    if let Some(e) = drop_error {
        // L'apply è fatto: non va ripetuto, lo stash è solo rimasto nell'elenco.
        warnings.push(format!(
            "lo stash `{}` è stato applicato ma non tolto dall'elenco ({e}): le modifiche sono \
             nei file; eliminarlo con `stash.drop` (non ripetere il pop)",
            entry.name
        ));
    } else if pop && !was_dropped {
        warnings.push(format!(
            "lo stash `{}` resta nell'elenco perché ci sono conflitti: risolverli (`index.stage` \
             dei file) e poi eliminarlo con `stash.drop`",
            entry.name
        ));
    }
    let backup = dropped.as_ref().and_then(|d| d.backup.clone());
    warnings.extend(dropped.into_iter().flat_map(|d| d.warnings));
    let mut out = json!({
        "stash": to_json(&entry),
        "conflicts": to_json(&applied.conflicts),
        "dropped": was_dropped,
        "backup": backup,
        "warnings": warnings,
    });
    with_stashes(&mut out, repo);
    with_status(&mut out, repo);
    Ok(out)
}

/// Applica uno stash a indice e worktree senza toglierlo dall'elenco (vedi
/// [`GitBackend::stash_apply`](crate::GitBackend::stash_apply)): mai sopra modifiche locali ai
/// file che cambia. I conflitti restano nei file (`conflicts`), lo stash resta.
///
/// Risultato: `{"stash": StashEntry, "conflicts": [string], "dropped": false, "backup": null,
/// "warnings": [string], "stashes": [StashEntry], "status": Status}`.
pub(super) fn stash_apply(s: &mut Session, p: Value) -> Out {
    apply("stash.apply", false, s, p)
}

/// Come `stash.apply`, poi, se non restano conflitti, toglie lo stash dall'elenco (prima un
/// backup del suo commit: `backup`). Con conflitti lo stash resta (`dropped: false`).
/// Risultato come `stash.apply`.
pub(super) fn stash_pop(s: &mut Session, p: Value) -> Out {
    apply("stash.pop", true, s, p)
}

/// Elimina lo stash `index`: serve `"confirm": true` **con `"expected"`**, l'id completo dello
/// stash confermato (senza `confirm`, [`EngineError::ConfirmRequired`] che dice cosa si
/// perderebbe e ha l'id in `expected`; `confirm` senza `expected`,
/// [`EngineError::InvalidParams`]): la conferma vale solo per lo stash visto. Prima un backup del commit
/// in `refs/project/backup/stash-...`; va nel diario, e `edit.undo` lo rimette in cima
/// all'elenco.
///
/// Risultato: `{"stash": StashEntry, "backup": string, "warnings": [string], "stashes":
/// [StashEntry], "can_undo", "can_redo", "next_undo", "next_redo"}`.
pub(super) fn stash_drop(s: &mut Session, p: Value) -> Out {
    const ID: &str = "stash.drop";
    let DropParams {
        index,
        expected,
        confirm,
    } = parse_params(ID, p)?;
    let expected = full_id(ID, expected)?;
    if confirm && expected.is_none() {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: "`confirm` va insieme a `expected`, l'id completo dello stash da eliminare \
                     (quello dato dalla conferma o da `stash.list`): la conferma vale solo per \
                     lo stash visto"
                .to_owned(),
        });
    }
    let (repo, journal) = s.repo_and_journal()?;
    let entry = pick(repo, index, expected)?;
    if !confirm {
        let files = repo
            .show_commit(&entry.id.to_string())
            .map(|d| d.files.len())
            .ok();
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: format!(
                "lo stash `{}` ({} «{}»{}{}) verrebbe eliminato. Prima si salverebbe il suo \
                 commit in `refs/project/backup/stash-...`, e `edit.undo` lo rimetterebbe in \
                 cima all'elenco. Per eliminarlo ripetere con `\"expected\": \"{}\"`",
                entry.name,
                entry.id,
                entry.message,
                files
                    .map(|n| format!(", {n} file tracciati"))
                    .unwrap_or_default(),
                if entry.untracked {
                    ", con file non tracciati"
                } else {
                    ""
                },
                entry.id
            ),
            expected: Some(entry.id.to_string()),
        });
    }
    let head = HeadState::from_head(&repo.head()?);
    let dropped = repo.stash_drop(&crate::StashDrop {
        command: ID,
        id: entry.id,
        backup: Some("stash"),
    })?;
    journal.record(JournalEntry::stash_drop(
        &entry,
        head,
        dropped.backup.clone(),
    ));
    let mut out = journal_state(journal);
    out["stash"] = to_json(&entry);
    out["backup"] = to_json(&dropped.backup);
    out["warnings"] = to_json(&dropped.warnings);
    with_stashes(&mut out, repo);
    Ok(out)
}

#[cfg(test)]
#[path = "stash_tests.rs"]
mod tests;
