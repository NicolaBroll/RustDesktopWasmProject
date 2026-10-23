//! Comandi dei branch locali (S24): `branch.create`, `branch.checkout`, `branch.delete`,
//! `branch.rename`. I controlli che non dipendono dal backend sono qui; scritture e controlli
//! sotto lock nei backend ([`GitBackend`](crate::GitBackend)). Ogni comando riuscito che cambia
//! qualcosa entra nel diario: `edit.undo` lo annulla (§7).

use super::{Out, journal_state, to_json};
use crate::commands::parse_params;
use crate::{
    EngineError, HeadChange, HeadState, JournalEntry, LogQuery, Session, branch_ref,
    check_branch_name, check_new_branch_name, check_no_operation,
};
use project_domain::{Head, Oid, RefInfo};
use serde::Deserialize;
use serde_json::{Value, json};

/// Parametri di `branch.create`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateParams {
    name: String,
    #[serde(default)]
    start: Option<String>,
}

/// Parametri di `branch.checkout`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameParams {
    name: String,
}

/// Parametri di `branch.delete`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteParams {
    name: String,
    #[serde(default)]
    confirm: bool,
    /// L'id completo del commit su cui chi chiede ha visto il branch.
    #[serde(default)]
    expected: Option<String>,
}

/// Parametri di `branch.rename`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameParams {
    name: String,
    new_name: String,
}

/// Il branch locale `full` fra i ref, `None` se non c'è.
fn find<'a>(refs: &'a [RefInfo], full: &str) -> Option<&'a RefInfo> {
    refs.iter().find(|r| r.name == full)
}

fn missing(name: &str) -> EngineError {
    EngineError::NotAllowed(format!(
        "non esiste un branch locale `{}`, niente modificato",
        name.escape_debug()
    ))
}

fn taken(name: &str) -> EngineError {
    EngineError::NotAllowed(format!(
        "esiste già un branch `{}`, niente modificato (un branch non si sovrascrive)",
        name.escape_debug()
    ))
}

/// Aggiunge a `out` i ref aggiornati (come `repo.refs`) o, se non si rileggono, `refs: null`
/// e `refs_error`: l'operazione è comunque fatta.
fn with_refs(out: &mut Value, refs: Result<Vec<RefInfo>, EngineError>) {
    match refs {
        Ok(refs) => out["refs"] = to_json(refs),
        Err(e) => {
            out["refs"] = Value::Null;
            out["refs_error"] = Value::String(e.to_string());
        }
    }
}

/// Crea il branch `name` su `start` (una rev: id, anche abbreviato, nome di branch o tag;
/// HEAD se manca), senza cambiare branch (vedi
/// [`GitBackend::create_branch`](crate::GitBackend::create_branch)). Un branch esistente non si
/// sovrascrive mai. L'operazione va nel diario: `edit.undo` elimina il branch, solo se nel
/// frattempo non è stato spostato.
///
/// Risultato: `{"branch": "refs/heads/<name>", "target": oid, "refs": [RefInfo], "can_undo",
/// "can_redo", "next_undo", "next_redo"}`.
pub(super) fn branch_create(s: &mut Session, p: Value) -> Out {
    const ID: &str = "branch.create";
    let CreateParams { name, start } = parse_params(ID, p)?;
    check_new_branch_name(ID, &name)?;
    let (repo, journal) = s.repo_and_journal()?;
    let head = repo.head()?;
    let target: Oid = match &start {
        Some(rev) => {
            let commits = repo.log(&LogQuery::page(0, 1).from_rev(rev.clone()))?;
            commits
                .first()
                .map(|c| c.id)
                .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?
        }
        None => head.target().ok_or_else(|| {
            EngineError::NotAllowed(
                "HEAD non ha ancora commit: indicare `start`, o creare prima un commit".to_owned(),
            )
        })?,
    };
    let full = branch_ref(&name);
    if find(&repo.refs()?, &full).is_some() {
        return Err(taken(&name));
    }
    repo.create_branch(&name, &target)?;
    journal.record(JournalEntry::branch_create(
        &full,
        target,
        HeadState::from_head(&head),
    ));
    let mut out = journal_state(journal);
    out["branch"] = Value::String(full);
    out["target"] = Value::String(target.to_string());
    with_refs(&mut out, repo.refs());
    Ok(out)
}

/// Passa al branch locale `name` (`git switch`, vedi
/// [`GitBackend::checkout`](crate::GitBackend::checkout)): indice e worktree diventano quelli
/// del branch, le modifiche locali che non toccano i file diversi restano. Non forza mai: se
/// una modifica locale (o un file non tracciato o ignorato) verrebbe sovrascritta,
/// [`EngineError::CheckoutConflict`] con i file e niente cambia. Rifiutato
/// ([`EngineError::NotAllowed`]) durante merge, rebase, cherry-pick, revert, bisect o `git am`,
/// e con file in conflitto. Il branch già attivo: nessun cambiamento (`changed: false`), niente
/// nel diario.
///
/// Va nel diario (tranne quando si lascia un branch senza commit, dove tornare vorrebbe dire
/// svuotare l'indice): `edit.undo` torna a HEAD di prima (branch o commit staccato) con un
/// altro checkout altrettanto sicuro, solo se HEAD è ancora sul branch.
///
/// Risultato: `{"branch": "refs/heads/<name>", "previous": HeadState, "changed": bool,
/// "warnings": [string], "status": Status | null, "can_undo", "can_redo", "next_undo",
/// "next_redo"}`.
pub(super) fn branch_checkout(s: &mut Session, p: Value) -> Out {
    const ID: &str = "branch.checkout";
    let NameParams { name } = parse_params(ID, p)?;
    check_branch_name(ID, &name)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    check_no_operation(ID, "cambierebbe branch", status.operation)?;
    let full = branch_ref(&name);
    if find(&repo.refs()?, &full).is_none() {
        return Err(missing(&name));
    }
    let before = HeadState::from_head(&status.head);
    let target = HeadState::Branch(full.clone());
    let mut warnings = Vec::new();
    let changed = before != target;
    if changed {
        let outcome = repo.checkout(&target)?;
        warnings = outcome.warnings;
        // Da un branch senza commit non si torna indietro senza svuotare l'indice: niente voce.
        let unborn = matches!(status.head, Head::Branch { target: None, .. });
        if !unborn {
            journal.record(JournalEntry::checkout(before.clone(), target));
        }
    }
    let mut out = journal_state(journal);
    out["branch"] = Value::String(full);
    out["previous"] = to_json(&before);
    out["changed"] = Value::Bool(changed);
    out["warnings"] = to_json(warnings);
    match repo.status() {
        Ok(status) => out["status"] = to_json(status),
        Err(e) => {
            out["status"] = Value::Null;
            out["status_error"] = Value::String(e.to_string());
        }
    }
    Ok(out)
}

/// Elimina il branch locale `name` (vedi
/// [`GitBackend::delete_branch`](crate::GitBackend::delete_branch)). Rifiutato
/// ([`EngineError::NotAllowed`]) per il branch di HEAD, un branch aperto in un altro worktree e
/// durante un'operazione in corso. Se il branch ha commit che non sono né in HEAD né nel suo
/// upstream (se c'è), cioè non è unito, serve `"confirm": true`. Con `expected` (id completo del
/// commit su cui il branch è stato mostrato) il branch deve puntare ancora lì, controllato prima
/// della conferma e dell'eliminazione: altrimenti [`EngineError::NotAllowed`] e niente cambia,
/// così una conferma vale solo per il commit visto: senza,
/// [`EngineError::ConfirmRequired`] con il numero di commit e niente cambia. Prima di eliminare
/// salva sempre il commit in `refs/project/backup/delete-...`; l'operazione va nel diario e
/// `edit.undo` ricrea il branch sullo stesso commit. La configurazione `branch.<name>.*`
/// (upstream, descrizione, ...) si toglie come con `git branch -d`, ma resta nella voce del
/// diario: l'undo la riscrive (se nel frattempo non ne è comparsa una nuova), il redo la toglie.
///
/// Risultato: `{"branch": "refs/heads/<name>", "target": oid, "unmerged": n, "backup": string,
/// "removed_config": [{"key", "value"}], "warnings": [string], "refs": [RefInfo], "can_undo", "can_redo", "next_undo",
/// "next_redo"}`.
pub(super) fn branch_delete(s: &mut Session, p: Value) -> Out {
    const ID: &str = "branch.delete";
    let DeleteParams {
        name,
        confirm,
        expected,
    } = parse_params(ID, p)?;
    check_branch_name(ID, &name)?;
    let expected = expected
        .map(|hex| {
            let full = hex.len() == Oid::HEX_LEN && hex.bytes().all(|b| b.is_ascii_hexdigit());
            full.then(|| hex.to_ascii_lowercase().parse::<Oid>().ok())
                .flatten()
                .ok_or_else(|| EngineError::InvalidParams {
                    id: ID,
                    reason: format!(
                        "`expected` deve essere l'id completo ({} cifre esadecimali) del commit \
                         del branch: `{}`",
                        Oid::HEX_LEN,
                        hex.escape_debug()
                    ),
                })
        })
        .transpose()?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let full = branch_ref(&name);
    let head = HeadState::from_head(&status.head);
    if head == HeadState::Branch(full.clone()) {
        return Err(EngineError::NotAllowed(format!(
            "`{}` è il branch attivo (HEAD), niente eliminato: passare prima a un altro branch",
            name.escape_debug()
        )));
    }
    check_no_operation(ID, "eliminerebbe un branch", status.operation)?;
    let refs = repo.refs()?;
    let branch = find(&refs, &full).ok_or_else(|| missing(&name))?;
    let target = branch.target;
    if let Some(seen) = expected
        && seen != target
    {
        return Err(EngineError::NotAllowed(format!(
            "il branch `{}` si è spostato da quando è stato visto: era {seen}, ora punta a \
             {target}. Niente eliminato; rileggere i branch e riprovare",
            name.escape_debug()
        )));
    }
    // Unito: raggiungibile da HEAD o dal suo upstream, come per `git branch -d`.
    let upstream = branch
        .upstream
        .as_ref()
        .and_then(|up| find(&refs, &up.name));
    let into: Vec<Oid> = status
        .head
        .target()
        .into_iter()
        .chain(upstream.map(|r| r.target))
        .collect();
    let unmerged = repo.unmerged_count(&target, &into)?;
    if unmerged > 0 && !confirm {
        let reference = match upstream {
            Some(up) => format!("né in HEAD né nell'upstream `{}`", up.name),
            None => "in HEAD".to_owned(),
        };
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: format!(
                "il branch `{}` non è unito: {unmerged} commit (fino a {}) non sono {reference}. \
                 Eliminandolo resterebbero solo nel backup `refs/project/backup/delete-...` \
                 salvato prima, e `edit.undo` lo ricreerebbe",
                name.escape_debug(),
                crate::git::short(&target),
            ),
        });
    }
    let deleted = repo.delete_branch(&name, &target)?;
    journal.record(JournalEntry::branch_delete(
        &full,
        target,
        head,
        deleted.backup.clone(),
        deleted.config.clone(),
    ));
    let mut out = journal_state(journal);
    out["branch"] = Value::String(full);
    out["target"] = Value::String(target.to_string());
    out["unmerged"] = json!(unmerged);
    out["backup"] = Value::String(deleted.backup);
    out["removed_config"] = to_json(deleted.config);
    out["warnings"] = to_json(deleted.warnings);
    with_refs(&mut out, repo.refs());
    Ok(out)
}

/// Rinomina il branch locale `name` in `new_name` (vedi
/// [`GitBackend::rename_branch`](crate::GitBackend::rename_branch)): mai sopra un branch
/// esistente; reflog, upstream e HEAD (se era il branch attivo) seguono. Rifiutato durante
/// un'operazione in corso. Va nel diario: `edit.undo` rinomina di nuovo, solo se nel frattempo
/// nessuno ha toccato i due nomi.
///
/// Risultato: `{"branch": "refs/heads/<new_name>", "previous": "refs/heads/<name>", "target":
/// oid, "head": bool (era il branch attivo), "warnings": [string], "refs": [RefInfo],
/// "can_undo", "can_redo", "next_undo", "next_redo"}`.
pub(super) fn branch_rename(s: &mut Session, p: Value) -> Out {
    const ID: &str = "branch.rename";
    let RenameParams { name, new_name } = parse_params(ID, p)?;
    check_branch_name(ID, &name)?;
    check_new_branch_name(ID, &new_name)?;
    if name == new_name {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: "`new_name` è uguale a `name`".to_owned(),
        });
    }
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    check_no_operation(ID, "rinominerebbe un branch", status.operation)?;
    let (from, to) = (branch_ref(&name), branch_ref(&new_name));
    let refs = repo.refs()?;
    let target = find(&refs, &from).ok_or_else(|| missing(&name))?.target;
    if find(&refs, &to).is_some() {
        return Err(taken(&new_name));
    }
    let before = HeadState::from_head(&status.head);
    let is_head = before == HeadState::Branch(from.clone());
    let outcome = repo.rename_branch(&name, &new_name, &target)?;
    let after = if is_head {
        HeadState::Branch(to.clone())
    } else {
        before.clone()
    };
    journal.record(JournalEntry::branch_rename(
        &from,
        &to,
        target,
        HeadChange { before, after },
    ));
    let mut out = journal_state(journal);
    out["branch"] = Value::String(to);
    out["previous"] = Value::String(from);
    out["target"] = Value::String(target.to_string());
    out["head"] = Value::Bool(is_head);
    out["warnings"] = to_json(outcome.warnings);
    with_refs(&mut out, repo.refs());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::{
        EngineError, GitBackend, HeadState, MemoryBackend, MemoryOpener, Registry, Session,
    };
    use project_domain::{Change, EntryKind, Head, Oid, RepoPath, StatusEntry};
    use serde_json::{Value, json};
    use std::sync::Arc;

    const FILE: u32 = 0o100644;

    /// Il repo di esempio con il worktree pulito (nessuna modifica, nessun non tracciato).
    fn clean_sample() -> MemoryBackend {
        let mut repo = MemoryBackend::sample();
        repo.set_entries(Vec::new());
        repo
    }

    fn open(repo: MemoryBackend) -> (Registry, Session) {
        let mut opener = MemoryOpener::new();
        opener.insert("r", repo);
        let r = Registry::builtin();
        let mut s = Session::new(Arc::new(opener));
        r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
        (r, s)
    }

    fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
        r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    fn branch(s: &Session, name: &str) -> Option<Oid> {
        s.repo()
            .unwrap()
            .refs()
            .unwrap()
            .into_iter()
            .find(|r| r.name == format!("refs/heads/{name}"))
            .map(|r| r.target)
    }

    fn head(s: &Session) -> Head {
        s.repo().unwrap().head().unwrap()
    }

    /// Il nome breve del branch di HEAD, `None` se staccata.
    fn on(s: &Session) -> Option<String> {
        match head(s) {
            Head::Branch { name, .. } => Some(name),
            Head::Detached(_) => None,
        }
    }

    fn head_oid(s: &Session) -> Oid {
        head(s).target().unwrap()
    }

    #[test]
    fn create_from_head_and_from_a_rev_then_undo_and_redo() {
        let (r, mut s) = open(MemoryBackend::sample());
        let main = head_oid(&s);
        let out = run(&r, &mut s, "branch.create", json!({ "name": "nuovo" }));
        assert_eq!(out["branch"], "refs/heads/nuovo");
        assert_eq!(out["target"], main.to_string());
        assert_eq!(out["can_undo"], true);
        assert!(
            out["next_undo"]
                .as_str()
                .unwrap()
                .starts_with("Crea branch `nuovo` su ")
        );
        assert!(
            out["refs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "refs/heads/nuovo")
        );
        assert_eq!(branch(&s, "nuovo"), Some(main));
        // HEAD non cambia.
        assert_eq!(on(&s).as_deref(), Some("main"));

        let grafo = branch(&s, "feature/grafo").unwrap();
        let out = run(
            &r,
            &mut s,
            "branch.create",
            json!({ "name": "da-grafo", "start": "feature/grafo" }),
        );
        assert_eq!(out["target"], grafo.to_string());
        let short = &grafo.to_string()[..8];
        let out = run(
            &r,
            &mut s,
            "branch.create",
            json!({ "name": "da-id", "start": short }),
        );
        assert_eq!(out["target"], grafo.to_string());

        run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(branch(&s, "da-id"), None);
        run(&r, &mut s, "edit.redo", json!({}));
        assert_eq!(branch(&s, "da-id"), Some(grafo));
    }

    #[test]
    fn create_refuses_existing_and_invalid_names() {
        let (r, mut s) = open(MemoryBackend::sample());
        let refs = s.repo().unwrap().refs().unwrap();
        for (params, want_invalid) in [
            (json!({ "name": "main" }), false),
            (json!({ "name": "feature" }), false), // `feature/grafo` esiste
            (json!({ "name": "main/sotto" }), false),
            (json!({ "name": "-x" }), true),
            (json!({ "name": "--force" }), true),
            (json!({ "name": "HEAD" }), true),
            (json!({ "name": "a b" }), true),
            (json!({ "name": "a..b" }), true),
            (json!({ "name": "refs/heads/x" }), true),
            (json!({ "name": "" }), true),
            (json!({ "name": "x", "altro": 1 }), true),
        ] {
            let err = r.run("branch.create", &mut s, params.clone()).unwrap_err();
            if want_invalid {
                assert!(
                    matches!(err, EngineError::InvalidParams { .. }),
                    "{params}: {err}"
                );
            } else {
                assert!(matches!(err, EngineError::NotAllowed(_)), "{params}: {err}");
            }
        }
        let err = r
            .run(
                "branch.create",
                &mut s,
                json!({ "name": "x", "start": "nessuno" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
        assert_eq!(s.repo().unwrap().refs().unwrap(), refs);
        assert!(!s.journal().unwrap().can_undo());

        // Senza commit serve `start`.
        let (r, mut s) = open(MemoryBackend::new());
        let err = r
            .run("branch.create", &mut s, json!({ "name": "x" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    }

    #[test]
    fn checkout_switches_and_undo_comes_back() {
        let (r, mut s) = open(clean_sample());
        let grafo = branch(&s, "feature/grafo").unwrap();
        let out = run(
            &r,
            &mut s,
            "branch.checkout",
            json!({ "name": "feature/grafo" }),
        );
        assert_eq!(out["branch"], "refs/heads/feature/grafo");
        assert_eq!(out["previous"], json!({ "branch": "refs/heads/main" }));
        assert_eq!(out["changed"], true);
        assert_eq!(out["warnings"], json!([]));
        assert_eq!(out["status"]["head"]["Branch"]["name"], "feature/grafo");
        assert_eq!(out["next_undo"], "Checkout di `feature/grafo` (da `main`)");
        assert_eq!(head_oid(&s), grafo);
        assert!(s.repo().unwrap().status().unwrap().entries.is_empty());

        // Già sul branch: niente cambia, niente nel diario.
        let out = run(
            &r,
            &mut s,
            "branch.checkout",
            json!({ "name": "feature/grafo" }),
        );
        assert_eq!(out["changed"], false);
        assert_eq!(s.journal().unwrap().done().len(), 1);

        let out = run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(out["undone"]["command"], "branch.checkout");
        assert_eq!(on(&s).as_deref(), Some("main"));
        run(&r, &mut s, "edit.redo", json!({}));
        assert_eq!(on(&s).as_deref(), Some("feature/grafo"));

        // Undo rifiutato se HEAD è cambiata da fuori.
        let main = branch(&s, "main").unwrap();
        s.repo_mut()
            .unwrap()
            .checkout(&HeadState::Detached(main))
            .unwrap();
        let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    }

    #[test]
    fn checkout_with_conflicting_changes_is_refused() {
        let (r, mut s) = open(MemoryBackend::sample());
        let status = s.repo().unwrap().status().unwrap();
        let err = r
            .run(
                "branch.checkout",
                &mut s,
                json!({ "name": "feature/grafo" }),
            )
            .unwrap_err();
        let EngineError::CheckoutConflict { target, files, .. } = &err else {
            panic!("{err}");
        };
        assert_eq!(target, "feature/grafo");
        assert!(!files.is_empty());
        assert!(err.to_string().contains("Niente modificato"), "{err}");
        assert_eq!(s.repo().unwrap().status().unwrap(), status);
        assert!(!s.journal().unwrap().can_undo());

        for (params, invalid) in [
            (json!({ "name": "nessuno" }), false),
            (json!({ "name": "a b" }), true),
            (json!({}), true),
        ] {
            let err = r
                .run("branch.checkout", &mut s, params.clone())
                .unwrap_err();
            assert_eq!(
                matches!(err, EngineError::InvalidParams { .. }),
                invalid,
                "{params}: {err}"
            );
        }
    }

    #[test]
    fn checkout_keeps_changes_to_files_that_do_not_differ() {
        let mut repo = clean_sample();
        let main = repo.head().unwrap().target().unwrap();
        // Un branch sullo stesso commit: nessun file diverso, le modifiche restano.
        repo.set_branch("gemello", main);
        let sample = MemoryBackend::sample();
        let entries = sample.status().unwrap().entries;
        repo.set_entries(entries.clone());
        let (r, mut s) = open(repo);
        run(&r, &mut s, "branch.checkout", json!({ "name": "gemello" }));
        let status = s.repo().unwrap().status().unwrap();
        assert_eq!(status.entries, entries);
        assert!(matches!(&status.head, Head::Branch { name, .. } if name == "gemello"));
    }

    #[test]
    fn checkout_refused_during_an_operation_or_with_conflicts() {
        let mut repo = clean_sample();
        repo.set_operation(Some(project_domain::Operation::Rebase));
        let (r, mut s) = open(repo);
        let err = r
            .run(
                "branch.checkout",
                &mut s,
                json!({ "name": "feature/grafo" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

        let mut repo = clean_sample();
        repo.set_entries(vec![StatusEntry {
            path: RepoPath::from("x"),
            index: Change::Unmerged,
            worktree: Change::Unmerged,
            kind: EntryKind::Unmerged(project_domain::UnmergedInfo {
                conflict: project_domain::Conflict::BothModified,
                submodule: None,
                mode_base: FILE,
                mode_ours: FILE,
                mode_theirs: FILE,
                mode_worktree: FILE,
                oid_base: MemoryBackend::fake_oid(1),
                oid_ours: MemoryBackend::fake_oid(2),
                oid_theirs: MemoryBackend::fake_oid(3),
            }),
        }]);
        let (r, mut s) = open(repo);
        let err = r
            .run(
                "branch.checkout",
                &mut s,
                json!({ "name": "feature/grafo" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    }

    #[test]
    fn delete_needs_confirm_when_unmerged_and_undo_recreates() {
        let mut repo = MemoryBackend::sample();
        let main = repo.head().unwrap().target().unwrap();
        // `feature/grafo` (commit 9) è nell'upstream e in `main`? Un branch con un commit
        // tutto suo non lo è.
        let tip = repo
            .commit(&crate::CommitRequest::new("Solo qui"))
            .unwrap()
            .commit
            .id;
        repo.set_branch("solo", tip);
        repo.set_branch("main", main);
        repo.set_head_branch("main");
        let (r, mut s) = open(repo);
        let refs = s.repo().unwrap().refs().unwrap();

        let err = r
            .run("branch.delete", &mut s, json!({ "name": "solo" }))
            .unwrap_err();
        let EngineError::ConfirmRequired { command, reason } = &err else {
            panic!("{err}");
        };
        assert_eq!(*command, "branch.delete");
        assert!(reason.contains("1 commit"), "{reason}");
        assert_eq!(s.repo().unwrap().refs().unwrap(), refs);
        assert!(!s.journal().unwrap().can_undo());

        // `expected` diverso dalla punta (anche con la conferma), abbreviato o non valido:
        // niente cambia.
        let short = &tip.to_string()[..10];
        for (params, invalid) in [
            (
                json!({ "name": "solo", "confirm": true, "expected": main.to_string() }),
                false,
            ),
            (
                json!({ "name": "solo", "expected": main.to_string() }),
                false,
            ),
            (
                json!({ "name": "solo", "confirm": true, "expected": short }),
                true,
            ),
            (
                json!({ "name": "solo", "confirm": true, "expected": "x".repeat(40) }),
                true,
            ),
        ] {
            let err = r.run("branch.delete", &mut s, params.clone()).unwrap_err();
            if invalid {
                assert!(
                    matches!(err, EngineError::InvalidParams { .. }),
                    "{params}: {err}"
                );
            } else {
                assert!(
                    matches!(&err, EngineError::NotAllowed(m) if m.contains(&tip.to_string())),
                    "{params}: {err}"
                );
            }
        }
        assert_eq!(s.repo().unwrap().refs().unwrap(), refs);
        assert!(!s.journal().unwrap().can_undo());
        // Senza conferma, con `expected` giusto: resta la richiesta di conferma.
        let err = r
            .run(
                "branch.delete",
                &mut s,
                json!({ "name": "solo", "expected": tip.to_string() }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");

        let out = run(
            &r,
            &mut s,
            "branch.delete",
            json!({ "name": "solo", "confirm": true, "expected": tip.to_string().to_uppercase() }),
        );
        assert_eq!(out["branch"], "refs/heads/solo");
        assert_eq!(out["target"], tip.to_string());
        assert_eq!(out["unmerged"], 1);
        assert_eq!(out["backup"], "refs/project/backup/delete-1");
        assert_eq!(branch(&s, "solo"), None);

        let out = run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(out["undone"]["command"], "branch.delete");
        assert_eq!(out["undone"]["backup"], "refs/project/backup/delete-1");
        assert_eq!(branch(&s, "solo"), Some(tip));
        run(&r, &mut s, "edit.redo", json!({}));
        assert_eq!(branch(&s, "solo"), None);
        run(&r, &mut s, "edit.undo", json!({}));

        // Un branch con lo stesso nome creato nel frattempo: l'undo non lo sovrascrive.
        run(
            &r,
            &mut s,
            "branch.delete",
            json!({ "name": "solo", "confirm": true }),
        );
        let (repo, _) = s.repo_and_journal().unwrap();
        repo.create_branch("solo", &main).unwrap();
        let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
        assert_eq!(branch(&s, "solo"), Some(main));
    }

    #[test]
    fn delete_of_a_merged_branch_needs_no_confirm_but_the_current_is_refused() {
        let (r, mut s) = open(MemoryBackend::sample());
        let grafo = branch(&s, "feature/grafo").unwrap();
        let out = run(
            &r,
            &mut s,
            "branch.delete",
            json!({ "name": "feature/grafo" }),
        );
        assert_eq!(out["unmerged"], 0);
        assert_eq!(out["target"], grafo.to_string());
        assert_eq!(branch(&s, "feature/grafo"), None);
        assert_eq!(
            out["removed_config"],
            json!([
                { "key": "branch.feature/grafo.remote", "value": "origin" },
                { "key": "branch.feature/grafo.merge", "value": "refs/heads/feature/grafo" }
            ])
        );
        // L'undo rimette branch e upstream, il redo li toglie.
        let upstream = |s: &Session| {
            s.repo()
                .unwrap()
                .refs()
                .unwrap()
                .into_iter()
                .find(|r| r.name == "refs/heads/feature/grafo")
                .and_then(|r| r.upstream)
                .map(|u| u.name)
        };
        let out = run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(out["warnings"], json!([]));
        assert_eq!(
            upstream(&s).as_deref(),
            Some("refs/remotes/origin/feature/grafo")
        );
        run(&r, &mut s, "edit.redo", json!({}));
        assert_eq!(branch(&s, "feature/grafo"), None);
        run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(
            upstream(&s).as_deref(),
            Some("refs/remotes/origin/feature/grafo")
        );
        run(&r, &mut s, "edit.redo", json!({}));

        let refs = s.repo().unwrap().refs().unwrap();
        for (params, invalid) in [
            (json!({ "name": "main", "confirm": true }), false),
            (json!({ "name": "nessuno", "confirm": true }), false),
            (json!({ "name": "a b" }), true),
        ] {
            let err = r.run("branch.delete", &mut s, params.clone()).unwrap_err();
            assert_eq!(
                matches!(err, EngineError::InvalidParams { .. }),
                invalid,
                "{params}: {err}"
            );
        }
        assert_eq!(s.repo().unwrap().refs().unwrap(), refs);
    }

    #[test]
    fn rename_and_undo() {
        let (r, mut s) = open(MemoryBackend::sample());
        let main = branch(&s, "main").unwrap();
        let out = run(
            &r,
            &mut s,
            "branch.rename",
            json!({ "name": "main", "new_name": "principale" }),
        );
        assert_eq!(out["branch"], "refs/heads/principale");
        assert_eq!(out["previous"], "refs/heads/main");
        assert_eq!(out["head"], true);
        assert_eq!(out["next_undo"], "Rinomina branch `main` in `principale`");
        assert_eq!(branch(&s, "main"), None);
        assert_eq!(branch(&s, "principale"), Some(main));
        assert_eq!(on(&s).as_deref(), Some("principale"));
        // L'upstream segue il branch.
        let refs = s.repo().unwrap().refs().unwrap();
        let renamed = refs
            .iter()
            .find(|r| r.name == "refs/heads/principale")
            .unwrap();
        assert_eq!(
            renamed.upstream.as_ref().unwrap().name,
            "refs/remotes/origin/main"
        );

        run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(branch(&s, "main"), Some(main));
        assert_eq!(branch(&s, "principale"), None);
        assert_eq!(on(&s).as_deref(), Some("main"));
        run(&r, &mut s, "edit.redo", json!({}));
        assert_eq!(on(&s).as_deref(), Some("principale"));

        // Rifiuti: nome già preso, uguale, non valido, branch assente.
        for (params, invalid) in [
            (
                json!({ "name": "principale", "new_name": "feature/grafo" }),
                false,
            ),
            (json!({ "name": "nessuno", "new_name": "x" }), false),
            (
                json!({ "name": "principale", "new_name": "principale" }),
                true,
            ),
            (json!({ "name": "principale", "new_name": "-x" }), true),
            (json!({ "name": "principale" }), true),
        ] {
            let err = r.run("branch.rename", &mut s, params.clone()).unwrap_err();
            assert_eq!(
                matches!(err, EngineError::InvalidParams { .. }),
                invalid,
                "{params}: {err}"
            );
        }

        // Undo rifiutato se il nome è stato ripreso nel frattempo.
        let (repo, _) = s.repo_and_journal().unwrap();
        repo.create_branch("main", &main).unwrap();
        let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
        assert_eq!(on(&s).as_deref(), Some("principale"));
    }

    #[test]
    fn branch_commands_need_an_open_repo() {
        let r = Registry::builtin();
        let mut s = Session::new(Arc::new(MemoryOpener::sample()));
        for (id, params) in [
            ("branch.create", json!({ "name": "x" })),
            ("branch.checkout", json!({ "name": "x" })),
            ("branch.delete", json!({ "name": "x" })),
            ("branch.rename", json!({ "name": "x", "new_name": "y" })),
        ] {
            let err = r.run(id, &mut s, params).unwrap_err();
            assert!(matches!(err, EngineError::NoRepo), "{id}: {err}");
        }
    }
}
