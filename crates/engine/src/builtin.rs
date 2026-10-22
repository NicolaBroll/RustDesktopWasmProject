//! Catalogo dei comandi predefiniti.

use crate::commands::parse_params;
use crate::{
    CommandSpec, CommitRequest, DiffSpec, EngineError, HeadState, Journal, JournalEntry,
    LineSelection, LogQuery, Registry, RestoreOutcome, Session, discard_plan, file_digest,
};
use project_domain::{Diff, LineRef, PatchDirection, RepoPath};
use serde::Deserialize;
use serde_json::{Value, json};

/// Commit restituiti da `repo.log` quando `limit` manca.
pub const DEFAULT_LOG_LIMIT: usize = 200;

impl Registry {
    /// Registro con tutti i comandi predefiniti.
    pub fn builtin() -> Self {
        let mut r = Self::default();
        let mut add = |id, title, params_help, handler| {
            r.register(
                CommandSpec {
                    id,
                    title,
                    params_help,
                    writes: WRITES.contains(&id),
                },
                handler,
            )
        };
        add(
            "repo.open",
            "Apri repository",
            r#"{"path": string}"#,
            repo_open,
        );
        add("repo.status", "Stato del repository", "{}", repo_status);
        add("repo.refs", "Branch, tag e stash", "{}", repo_refs);
        add(
            "repo.log",
            "Cronologia dei commit",
            r#"{"skip"?: number, "limit"?: number (200), "rev"?: string}"#,
            repo_log,
        );
        add(
            "commit.show",
            "Dettagli del commit",
            r#"{"id": string}"#,
            commit_show,
        );
        add(
            "diff.worktree",
            "Diff delle modifiche non in stage",
            r#"{"path"?: percorso}"#,
            diff_worktree,
        );
        add(
            "diff.index",
            "Diff delle modifiche in stage",
            r#"{"path"?: percorso}"#,
            diff_index,
        );
        add(
            "diff.commit",
            "Diff del commit",
            r#"{"id": string, "path"?: percorso}"#,
            diff_commit,
        );
        add(
            "index.stage",
            "Metti in stage i file",
            r#"{"paths": [percorso, ...]}"#,
            index_stage,
        );
        add(
            "index.unstage",
            "Togli i file dallo stage",
            r#"{"paths": [percorso, ...]}"#,
            index_unstage,
        );
        add(
            "index.stage_lines",
            "Metti in stage le righe scelte",
            r#"{"path": percorso, "lines": [{"old": n} | {"new": n}, ...], "digest": string (da diff.worktree)}"#,
            index_stage_lines,
        );
        add(
            "index.unstage_lines",
            "Togli dallo stage le righe scelte",
            r#"{"path": percorso, "lines": [{"old": n} | {"new": n}, ...], "digest": string (da diff.index)}"#,
            index_unstage_lines,
        );
        add(
            "commit.create",
            "Crea un commit con le modifiche in stage",
            r#"{"message": string, "amend"?: bool, "confirm"?: bool (amend di un commit pubblicato)}"#,
            commit_create,
        );
        add(
            "worktree.discard",
            "Scarta le modifiche non in stage",
            r#"{"paths": [percorso, ...], "confirm": true}"#,
            worktree_discard,
        );
        add("edit.undo", "Annulla l'ultima operazione", "{}", edit_undo);
        add("edit.redo", "Rifai l'operazione annullata", "{}", edit_redo);
        add(
            "edit.history",
            "Diario delle operazioni",
            "{}",
            edit_history,
        );
        r
    }
}

/// I comandi che modificano il repository ([`CommandSpec::writes`]).
const WRITES: &[&str] = &[
    "commit.create",
    "edit.redo",
    "edit.undo",
    "index.stage",
    "index.unstage",
    "index.stage_lines",
    "index.unstage_lines",
    "worktree.discard",
];

type Out = Result<Value, EngineError>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParams {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathParam {
    path: String,
}

fn to_json(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("i tipi del dominio sono serializzabili")
}

/// `{"path", "head"}` del repo appena aperto.
fn repo_open(s: &mut Session, p: Value) -> Out {
    let PathParam { path } = parse_params("repo.open", p)?;
    s.open(&path)?;
    let head = s.repo()?.head()?;
    Ok(json!({ "path": path, "head": to_json(head) }))
}

/// Lo [`Status`](project_domain::Status) del repo aperto.
fn repo_status(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("repo.status", p)?;
    Ok(to_json(s.repo()?.status()?))
}

/// I [`RefInfo`](project_domain::RefInfo) del repo aperto, in ordine di nome.
fn repo_refs(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("repo.refs", p)?;
    Ok(to_json(s.repo()?.refs()?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdParam {
    id: String,
}

/// I [`CommitDetails`](project_domain::CommitDetails) del commit `id` (anche abbreviato, o un
/// nome di ref): `{"commit", "compared_to", "files"}`, con i file confrontati con il primo
/// genitore.
fn commit_show(s: &mut Session, p: Value) -> Out {
    let IdParam { id } = parse_params("commit.show", p)?;
    Ok(to_json(s.repo()?.show_commit(&id)?))
}

/// Parametri di `diff.worktree` e `diff.index`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffParams {
    #[serde(default)]
    path: Option<RepoPath>,
}

/// Parametri di `diff.commit`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitDiffParams {
    id: String,
    #[serde(default)]
    path: Option<RepoPath>,
}

/// Come [`check_path`], per un percorso facoltativo.
fn check_opt_path(
    id: &'static str,
    path: Option<RepoPath>,
) -> Result<Option<RepoPath>, EngineError> {
    path.map(|p| check_path(id, p)).transpose()
}

/// Un percorso di file come lo scrive git: relativo alla radice, con `/` come separatore, senza
/// componenti vuoti, `.` o `..` e senza NUL. Altrimenti [`EngineError::InvalidParams`].
fn check_path(id: &'static str, path: RepoPath) -> Result<RepoPath, EngineError> {
    let bytes = path.as_bytes();
    let reason = if bytes.is_empty() {
        Some("il percorso è vuoto")
    } else if bytes.contains(&0) {
        Some("il percorso contiene un byte NUL")
    } else if bytes
        .split(|&b| b == b'/')
        .any(|c| c.is_empty() || c == b"." || c == b"..")
    {
        Some(
            "il percorso deve essere relativo alla radice del repo, con `/` come separatore e \
             senza componenti vuoti, `.` o `..`",
        )
    } else {
        None
    };
    match reason {
        Some(reason) => Err(EngineError::InvalidParams {
            id,
            reason: format!("{reason}: `{path}`"),
        }),
        None => Ok(path),
    }
}

/// Il JSON di un [`Diff`] con in più, in ogni file, il campo `digest` ([`file_digest`]) da
/// passare a `index.stage_lines`/`index.unstage_lines`. Chi legge il risultato come
/// [`Diff`] lo ignora.
fn diff_with_digests(diff: &Diff) -> Value {
    let mut out = to_json(diff);
    if let Some(files) = out["files"].as_array_mut() {
        for (json, file) in files.iter_mut().zip(&diff.files) {
            json["digest"] = Value::String(file_digest(file));
        }
    }
    out
}

/// Il [`Diff`] del worktree rispetto all'indice (modifiche non in stage): `{"files": [...]}`,
/// ogni file con il suo `digest`. I file non tracciati non compaiono.
fn diff_worktree(s: &mut Session, p: Value) -> Out {
    let DiffParams { path } = parse_params("diff.worktree", p)?;
    let path = check_opt_path("diff.worktree", path)?;
    Ok(diff_with_digests(
        &s.repo()?.diff(&DiffSpec::Worktree { path })?,
    ))
}

/// Il [`Diff`] dell'indice rispetto a HEAD (modifiche in stage), ogni file con il suo
/// `digest`.
fn diff_index(s: &mut Session, p: Value) -> Out {
    let DiffParams { path } = parse_params("diff.index", p)?;
    let path = check_opt_path("diff.index", path)?;
    Ok(diff_with_digests(
        &s.repo()?.diff(&DiffSpec::Index { path })?,
    ))
}

/// Il [`Diff`](project_domain::Diff) del commit `id` rispetto al primo genitore (come
/// `commit.show`).
fn diff_commit(s: &mut Session, p: Value) -> Out {
    let CommitDiffParams { id, path } = parse_params("diff.commit", p)?;
    let path = check_opt_path("diff.commit", path)?;
    Ok(to_json(
        s.repo()?.diff(&DiffSpec::Commit { rev: id, path })?,
    ))
}

/// Parametri di `index.stage` e `index.unstage`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathsParams {
    paths: Vec<RepoPath>,
}

/// I percorsi di `index.stage`/`index.unstage`, validati come in `diff.*` e senza doppioni
/// (nell'ordine dato). Una lista vuota è un errore: per git "nessun percorso" vorrebbe dire
/// "tutto", ed è meglio che non si possa chiedere per sbaglio.
fn check_paths(id: &'static str, p: Value) -> Result<Vec<RepoPath>, EngineError> {
    let PathsParams { paths } = parse_params(id, p)?;
    check_path_list(id, paths)
}

/// Come [`check_paths`], per una lista già letta.
fn check_path_list(id: &'static str, paths: Vec<RepoPath>) -> Result<Vec<RepoPath>, EngineError> {
    if paths.is_empty() {
        return Err(EngineError::InvalidParams {
            id,
            reason: "`paths` è vuoto: indicare almeno un percorso".to_owned(),
        });
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let path = check_path(id, path)?;
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    Ok(out)
}

/// Mette in stage `paths` (vedi [`GitBackend::stage`](crate::GitBackend::stage)).
/// Risultato: `{"status": Status, "backup": string | null}`, con il nuovo
/// [`Status`](project_domain::Status) (chi chiama aggiorna la vista senza un secondo comando) e
/// il ref del backup dell'indice, se prima di scrivere sono state salvate versioni in stage
/// che esistevano solo nell'indice.
fn index_stage(s: &mut Session, p: Value) -> Out {
    let paths = check_paths("index.stage", p)?;
    let repo = s.repo_mut()?;
    let backup = repo.stage(&paths)?;
    Ok(json!({ "status": to_json(repo.status()?), "backup": backup }))
}

/// Toglie dallo stage `paths` (vedi [`GitBackend::unstage`](crate::GitBackend::unstage)),
/// senza toccare il worktree. Risultato come `index.stage`.
fn index_unstage(s: &mut Session, p: Value) -> Out {
    let paths = check_paths("index.unstage", p)?;
    let repo = s.repo_mut()?;
    let backup = repo.unstage(&paths)?;
    Ok(json!({ "status": to_json(repo.status()?), "backup": backup }))
}

/// Parametri di `index.stage_lines` e `index.unstage_lines`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinesParams {
    path: RepoPath,
    lines: Vec<LineRef>,
    digest: String,
}

/// Le righe `lines` del file `path` in stage o fuori, con il diff visto identificato da
/// `digest` (vedi [`GitBackend::apply_lines`](crate::GitBackend::apply_lines)). Una selezione
/// vuota è un errore, non un'operazione che non fa niente: è quasi sempre uno sbaglio di chi
/// chiama. Risultato come `index.stage`.
fn index_lines(id: &'static str, direction: PatchDirection, s: &mut Session, p: Value) -> Out {
    let LinesParams {
        path,
        lines,
        digest,
    } = parse_params(id, p)?;
    let path = check_path(id, path)?;
    if lines.is_empty() {
        return Err(EngineError::InvalidParams {
            id,
            reason: "`lines` è vuoto: indicare almeno una riga cambiata del diff".to_owned(),
        });
    }
    let repo = s.repo_mut()?;
    let backup = repo.apply_lines(&LineSelection {
        path,
        lines,
        digest,
        direction,
    })?;
    Ok(json!({ "status": to_json(repo.status()?), "backup": backup }))
}

/// Mette in stage le righe scelte del diff del worktree.
fn index_stage_lines(s: &mut Session, p: Value) -> Out {
    index_lines("index.stage_lines", PatchDirection::Stage, s, p)
}

/// Toglie dallo stage le righe scelte del diff dell'indice.
fn index_unstage_lines(s: &mut Session, p: Value) -> Out {
    index_lines("index.unstage_lines", PatchDirection::Unstage, s, p)
}

/// Crea un commit con quello che è in stage, o con `amend` riscrive l'ultimo (vedi
/// [`GitBackend::commit`](crate::GitBackend::commit)). `confirm` serve solo per `amend` di un
/// commit già pubblicato.
///
/// Risultato: `{"commit": Commit, "previous": string | null, "branch": string | null, "backup":
/// string | null, "also_on": [string], "status": Status}`: il commit creato (come in `repo.log`), HEAD prima
/// (`null` per il primo commit), il branch spostato con il nome completo (`null` con HEAD
/// staccata), il ref di backup del commit riscritto (solo con `amend`), gli altri branch e tag
/// che restano sul commit riscritto (solo con `amend`) e il nuovo stato. Un errore dopo un
/// commit riuscito è [`EngineError::PostCommit`].
fn commit_create(s: &mut Session, p: Value) -> Out {
    let request: CommitRequest = parse_params("commit.create", p)?;
    request.check_message()?;
    let (repo, journal) = s.repo_and_journal()?;
    let outcome = repo.commit(&request)?;
    // Il commit è fatto: va nel diario (per `edit.undo`) anche se poi lo stato non si rilegge.
    if let Some(entry) = JournalEntry::commit(&outcome, request.amend) {
        journal.record(entry);
    }
    // Il commit è fatto: un errore da qui in poi non deve sembrare un commit fallito.
    let status = repo.status().map_err(|e| EngineError::PostCommit {
        commit: Some(outcome.commit.id),
        backup: outcome.backup.clone(),
        reason: format!("lo stato non si rilegge ({e})"),
    })?;
    let mut out = to_json(outcome);
    out["status"] = to_json(status);
    Ok(out)
}

/// Parametri di `worktree.discard`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscardParams {
    paths: Vec<RepoPath>,
    #[serde(default)]
    confirm: bool,
}

/// Scarta le modifiche non in stage dei file `paths` (file o cartelle): il worktree torna
/// come l'indice, l'indice non si tocca (vedi
/// [`GitBackend::discard`](crate::GitBackend::discard) e [`discard_plan`]: i non tracciati non
/// si toccano mai). Serve `"confirm": true`: senza, [`EngineError::ConfirmRequired`] con i file
/// che si perderebbero e niente cambia (i rifiuti di [`discard_plan`] vengono prima). Prima di
/// scrivere il worktree si salva con `git stash create` in `refs/project/backup/discard-...`;
/// l'operazione va nel diario e `edit.undo` rimette i file com'erano.
///
/// Risultato: `{"files": [{"path", "change", "mode", "oid"}], "backup": string, "stash":
/// string, "status": Status | null, "can_undo", "can_redo", "next_undo", "next_redo"}`; lo
/// stato è `null` (con `status_error`) se non si rilegge: lo scarto è comunque fatto.
fn worktree_discard(s: &mut Session, p: Value) -> Out {
    const ID: &str = "worktree.discard";
    let DiscardParams { paths, confirm } = parse_params(ID, p)?;
    let paths = check_path_list(ID, paths)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let files = discard_plan(&status, &paths)?;
    if !confirm {
        let names: Vec<String> = files.iter().map(|f| format!("`{}`", f.path)).collect();
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: format!(
                "le modifiche non in stage di {} file andrebbero perse ({}): il worktree \
                 tornerebbe come l'indice. Prima si salverebbe un backup in \
                 `refs/project/backup/discard-...`, e `edit.undo` le rimetterebbe",
                files.len(),
                names.join(", ")
            ),
        });
    }
    // Il backend rilegge lo stato e rifiuta se i file non sono più questi.
    let outcome = repo.discard(&paths, &files)?;
    // I file sono scartati: la voce va nel diario anche se poi lo stato non si rilegge.
    journal.record(JournalEntry::discard(
        &outcome,
        HeadState::from_head(&status.head),
    ));
    let mut out = journal_state(journal);
    out["files"] = to_json(&outcome.files);
    out["backup"] = Value::String(outcome.backup);
    out["stash"] = Value::String(outcome.stash.to_string());
    match repo.status() {
        Ok(status) => out["status"] = to_json(status),
        Err(e) => {
            out["status"] = Value::Null;
            out["status_error"] = Value::String(e.to_string());
        }
    }
    Ok(out)
}

/// `{"can_undo", "can_redo", "next_undo", "next_redo"}`: se `edit.undo`/`edit.redo` hanno
/// qualcosa da fare e la descrizione della voce che toccherebbero (`null` se nessuna).
fn journal_state(journal: &Journal) -> Value {
    json!({
        "can_undo": journal.can_undo(),
        "can_redo": journal.can_redo(),
        "next_undo": journal.done().last().map(|e| e.description.clone()),
        "next_redo": journal.undone().last().map(|e| e.description.clone()),
    })
}

/// Risultato di `edit.undo`/`edit.redo`: `{<key>: voce, "backups": [string], "warnings":
/// [string], "status": Status | null, "can_undo", "can_redo", "next_undo", "next_redo"}`.
/// `warnings` segnala qualcosa di inatteso scoperto dopo lo spostamento riuscito (di solito
/// vuoto). Lo stato è `null` (con
/// `status_error`) se non si rilegge: l'operazione è comunque fatta e non va ripetuta (un
/// secondo undo annullerebbe la voce precedente).
fn undo_result(s: &mut Session, key: &str, entry: JournalEntry, outcome: RestoreOutcome) -> Out {
    let mut out = journal_state(s.journal()?);
    out[key] = to_json(entry);
    out["backups"] = to_json(outcome.backups);
    out["warnings"] = to_json(outcome.warnings);
    match s.repo()?.status() {
        Ok(status) => out["status"] = to_json(status),
        Err(e) => {
            out["status"] = Value::Null;
            out["status_error"] = Value::String(e.to_string());
        }
    }
    Ok(out)
}

/// Annulla l'ultima operazione del diario (vedi [`Journal::undo`]): i ref tornano ai valori di
/// prima, solo se nessuno li ha spostati nel frattempo ([`EngineError::UndoConflict`]
/// altrimenti, e niente cambia). Indice e worktree non si toccano. Risultato: vedi
/// [`undo_result`], con la voce in `undone`.
fn edit_undo(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("edit.undo", p)?;
    let (repo, journal) = s.repo_and_journal()?;
    let (entry, outcome) = journal.undo(repo)?;
    undo_result(s, "undone", entry, outcome)
}

/// Rifà l'ultima operazione annullata (vedi [`Journal::redo`]), con lo stesso controllo.
/// Risultato: vedi [`undo_result`], con la voce in `redone`.
fn edit_redo(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("edit.redo", p)?;
    let (repo, journal) = s.repo_and_journal()?;
    let (entry, outcome) = journal.redo(repo)?;
    undo_result(s, "redone", entry, outcome)
}

/// Il diario del repo aperto: `{"done": [voce], "undone": [voce], "can_undo", "can_redo",
/// "next_undo", "next_redo"}`. In `done` l'ultima voce è quella che `edit.undo` annulla, in
/// `undone` l'ultima è quella che `edit.redo` rifà.
fn edit_history(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("edit.history", p)?;
    let journal = s.journal()?;
    let mut out = journal_state(journal);
    out["done"] = to_json(journal.done());
    out["undone"] = to_json(journal.undone());
    Ok(out)
}

/// Una pagina del log: `{"commits": [...], "has_more": bool}`. `has_more` dice se dopo
/// l'ultimo commit restituito ce ne sono altri (la pagina seguente parte da `skip + limit`).
fn repo_log(s: &mut Session, p: Value) -> Out {
    let query: LogQuery = parse_params("repo.log", p)?;
    let limit = query.limit.unwrap_or(DEFAULT_LOG_LIMIT);
    // Un commit in più per sapere se la pagina è l'ultima.
    let probe = LogQuery {
        limit: Some(limit.saturating_add(1)),
        ..query
    };
    let mut commits = s.repo()?.log(&probe)?;
    let has_more = commits.len() > limit;
    commits.truncate(limit);
    Ok(json!({ "commits": to_json(commits), "has_more": has_more }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitBackend, MemoryBackend, MemoryOpener};
    use project_domain::{Commit, CommitDetails, Diff, RefInfo, Status};
    use std::sync::Arc;

    fn session() -> Session {
        Session::new(Arc::new(MemoryOpener::sample()))
    }

    fn opened() -> (Registry, Session) {
        let r = Registry::builtin();
        let mut s = session();
        r.run(
            "repo.open",
            &mut s,
            json!({ "path": MemoryOpener::SAMPLE_PATH }),
        )
        .unwrap();
        (r, s)
    }

    fn log_page(r: &Registry, s: &mut Session, params: Value) -> (Vec<Commit>, bool) {
        let out = r.run("repo.log", s, params).unwrap();
        let commits = serde_json::from_value(out["commits"].clone()).unwrap();
        (commits, out["has_more"].as_bool().unwrap())
    }

    #[test]
    fn catalog() {
        let r = Registry::builtin();
        let ids: Vec<_> = r.list().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "commit.create",
                "commit.show",
                "diff.commit",
                "diff.index",
                "diff.worktree",
                "edit.history",
                "edit.redo",
                "edit.undo",
                "index.stage",
                "index.stage_lines",
                "index.unstage",
                "index.unstage_lines",
                "repo.log",
                "repo.open",
                "repo.refs",
                "repo.status",
                "worktree.discard"
            ]
        );
        let ids: Vec<_> = r.search("CRONOLOGIA").map(|c| c.id).collect();
        assert_eq!(ids, ["repo.log"]);
        let writes: Vec<_> = r.list().filter(|c| c.writes).map(|c| c.id).collect();
        assert_eq!(
            writes,
            [
                "commit.create",
                "edit.redo",
                "edit.undo",
                "index.stage",
                "index.stage_lines",
                "index.unstage",
                "index.unstage_lines",
                "worktree.discard"
            ]
        );
    }

    #[test]
    fn open_returns_path_and_head() {
        let r = Registry::builtin();
        let mut s = session();
        let out = r
            .run(
                "repo.open",
                &mut s,
                json!({ "path": MemoryOpener::SAMPLE_PATH }),
            )
            .unwrap();
        assert_eq!(out["path"], MemoryOpener::SAMPLE_PATH);
        let head = MemoryBackend::sample().head().unwrap();
        assert_eq!(out["head"], to_json(head));
        assert_eq!(s.path(), Some(MemoryOpener::SAMPLE_PATH));
    }

    #[test]
    fn reads_need_an_open_repo() {
        let r = Registry::builtin();
        let mut s = session();
        for id in [
            "repo.status",
            "repo.refs",
            "repo.log",
            "commit.show",
            "diff.worktree",
            "diff.index",
            "diff.commit",
            "index.stage",
            "index.unstage",
            "index.stage_lines",
            "index.unstage_lines",
            "commit.create",
            "worktree.discard",
        ] {
            let params = if matches!(id, "commit.show" | "diff.commit") {
                json!({ "id": "HEAD" })
            } else if id.ends_with("_lines") {
                json!({ "path": "README.md", "lines": [{ "new": 1 }], "digest": "x" })
            } else if id.starts_with("index.") || id == "worktree.discard" {
                json!({ "paths": ["README.md"] })
            } else if id == "commit.create" {
                json!({ "message": "x" })
            } else {
                json!({})
            };
            let err = r.run(id, &mut s, params).unwrap_err();
            assert!(matches!(err, EngineError::NoRepo), "{id}: {err}");
        }
    }

    #[test]
    fn status_and_refs_match_the_backend() {
        let (r, mut s) = opened();
        let sample = MemoryBackend::sample();

        let status: Status =
            serde_json::from_value(r.run("repo.status", &mut s, Value::Null).unwrap()).unwrap();
        assert_eq!(status, sample.status().unwrap());

        let refs: Vec<RefInfo> =
            serde_json::from_value(r.run("repo.refs", &mut s, json!({})).unwrap()).unwrap();
        assert_eq!(refs, sample.refs().unwrap());
    }

    #[test]
    fn log_pages_cover_the_whole_history() {
        let (r, mut s) = opened();
        let all = MemoryBackend::sample().log(&LogQuery::all()).unwrap();
        assert!(all.len() > 3, "il repo di esempio ha una dozzina di commit");

        let (default, more) = log_page(&r, &mut s, json!({}));
        assert_eq!((default.as_slice(), more), (all.as_slice(), false));

        let mut pages = Vec::new();
        let mut skip = 0;
        loop {
            let (page, more) = log_page(&r, &mut s, json!({ "skip": skip, "limit": 3 }));
            assert!(page.len() <= 3);
            skip += page.len();
            pages.extend(page);
            if !more {
                break;
            }
        }
        assert_eq!(pages, all);

        let (exact, more) = log_page(&r, &mut s, json!({ "limit": all.len() }));
        assert_eq!((exact.len(), more), (all.len(), false));
        let (empty, more) = log_page(&r, &mut s, json!({ "limit": 0 }));
        assert_eq!((empty.len(), more), (0, true));
    }

    #[test]
    fn log_from_rev() {
        let (r, mut s) = opened();
        let first = MemoryBackend::sample().log(&LogQuery::all()).unwrap()[1].id;
        let (page, _) = log_page(&r, &mut s, json!({ "rev": first.to_string(), "limit": 1 }));
        assert_eq!(page[0].id, first);

        let err = r
            .run("repo.log", &mut s, json!({ "rev": "nessuno" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    }

    #[test]
    fn commit_show_returns_metadata_and_files() {
        let (r, mut s) = opened();
        let sample = MemoryBackend::sample();
        let head = sample.head().unwrap().target().unwrap();

        let out = r
            .run("commit.show", &mut s, json!({ "id": "HEAD" }))
            .unwrap();
        let details: CommitDetails = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(details, sample.show_commit("HEAD").unwrap());
        assert_eq!(details.commit.id, head);
        assert_eq!(details.compared_to, Some(details.commit.parents[0]));
        assert!(!details.files.is_empty());

        // Forma del JSON che legge la UI.
        assert_eq!(out["commit"]["id"], head.to_string());
        assert_eq!(out["compared_to"], details.commit.parents[0].to_string());
        let renamed = out["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["change"] == "Renamed")
            .expect("il commit 12 dell'esempio ha una rinomina");
        assert_eq!(renamed["original"], "src/main.rs");
        assert_eq!(renamed["path"], "src/bin/app.rs");
        assert_eq!(renamed["score"], 92);

        // Id abbreviato e nome di ref portano allo stesso commit.
        let short = head.to_string()[..8].to_owned();
        for id in [short.as_str(), "main", "refs/heads/main"] {
            let out = r.run("commit.show", &mut s, json!({ "id": id })).unwrap();
            assert_eq!(out["commit"]["id"], head.to_string(), "{id}");
        }

        let err = r
            .run("commit.show", &mut s, json!({ "id": "nessuno" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    }

    #[test]
    fn diff_commands_return_the_backend_diff() {
        let (r, mut s) = opened();
        let sample = MemoryBackend::sample();
        let path = |p: &str| Some(RepoPath::from(p));
        for (id, params, spec) in [
            (
                "diff.worktree",
                json!({}),
                DiffSpec::Worktree { path: None },
            ),
            ("diff.index", Value::Null, DiffSpec::Index { path: None }),
            (
                "diff.index",
                json!({ "path": "README.md" }),
                DiffSpec::Index {
                    path: path("README.md"),
                },
            ),
            (
                "diff.worktree",
                json!({ "path": null }),
                DiffSpec::Worktree { path: None },
            ),
            (
                "diff.commit",
                json!({ "id": "main" }),
                DiffSpec::Commit {
                    rev: "HEAD".into(),
                    path: None,
                },
            ),
            (
                "diff.commit",
                json!({ "id": "HEAD", "path": "src/main.rs" }),
                DiffSpec::Commit {
                    rev: "HEAD".into(),
                    path: path("src/main.rs"),
                },
            ),
        ] {
            let out = r.run(id, &mut s, params.clone()).unwrap();
            let diff: Diff = serde_json::from_value(out).unwrap();
            assert_eq!(diff, sample.diff(&spec).unwrap(), "{id} {params}");
            assert!(!diff.files.is_empty(), "{id} {params}");
        }

        // Forma del JSON che legge la UI.
        let out = r
            .run(
                "diff.commit",
                &mut s,
                json!({ "id": "HEAD", "path": "src/bin/app.rs" }),
            )
            .unwrap();
        let files = out["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f["change"], "Renamed");
        assert_eq!(f["old_path"], "src/main.rs");
        assert_eq!(f["new_path"], "src/bin/app.rs");
        assert_eq!(f["similarity"], 92);
        assert_eq!(f["old_mode"], 0o100644);
        assert_eq!(f["binary"], false);
        assert!(f["old_oid"].is_string());
        let hunk = &f["hunks"][0];
        assert_eq!(hunk["old_start"], 1);
        assert_eq!(hunk["section"], "");
        let line = &hunk["lines"][1];
        assert_eq!(line["kind"], "Removed");
        assert_eq!(line["old_line"], 2);
        assert_eq!(line["new_line"], Value::Null);
        assert_eq!(line["content"], "// ultima modifica: commit 3");
        assert_eq!(line["no_newline"], false);

        // Un percorso non UTF-8 si passa come array di byte; qui non è cambiato.
        let out = r
            .run("diff.index", &mut s, json!({ "path": [99, 97, 255] }))
            .unwrap();
        assert_eq!(out, json!({ "files": [] }));

        let err = r
            .run("diff.commit", &mut s, json!({ "id": "nessuno" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    }

    #[test]
    fn stage_and_unstage_return_the_new_status() {
        let (r, mut s) = opened();
        let original = MemoryBackend::sample();

        let out = r
            .run(
                "index.stage",
                &mut s,
                json!({ "paths": ["note.txt", "src/lib.rs", "note.txt"] }),
            )
            .unwrap();
        assert_eq!(
            out["status"],
            r.run("repo.status", &mut s, json!({})).unwrap()
        );
        // Niente a rischio: `note.txt` e `src/lib.rs` non avevano versioni solo in stage.
        assert_eq!(out["backup"], Value::Null);
        let status: Status = serde_json::from_value(out["status"].clone()).unwrap();
        let staged: Vec<_> = status
            .entries
            .iter()
            .map(|e| (e.path.to_string(), e.index.letter(), e.worktree.letter()))
            .collect();
        assert_eq!(
            staged,
            [
                ("README.md".to_owned(), 'M', 'M'),
                ("docs/guida.md".to_owned(), 'A', '.'),
                ("note.txt".to_owned(), 'A', '.'),
                ("src/lib.rs".to_owned(), 'M', '.'),
            ]
        );
        // Forma del JSON che legge la UI: `status` come `repo.status`.
        assert_eq!(out["status"]["entries"][2]["path"], "note.txt");
        assert_eq!(out["status"]["entries"][2]["index"], "Added");
        assert_eq!(out["status"]["head"]["Branch"]["name"], "main");
        // I diff vedono il nuovo indice.
        let diff = r
            .run("diff.index", &mut s, json!({ "path": "note.txt" }))
            .unwrap();
        assert_eq!(diff["files"][0]["change"], "Added");

        let out = r
            .run(
                "index.unstage",
                &mut s,
                json!({ "paths": ["note.txt", "src/lib.rs"] }),
            )
            .unwrap();
        let status: Status = serde_json::from_value(out["status"].clone()).unwrap();
        assert_eq!(status, original.status().unwrap());

        // README.md è `MM`: la versione in stage esiste solo nell'indice, prima un backup.
        let out = r
            .run("index.unstage", &mut s, json!({ "paths": ["README.md"] }))
            .unwrap();
        assert_eq!(out["backup"], "refs/project/backup/index-1");
        assert_eq!(out["status"]["entries"][0]["index"], "Unmodified");

        // Un percorso non UTF-8 si passa come array di byte; qui non esiste.
        let err = r
            .run("index.stage", &mut s, json!({ "paths": [[99, 97, 255]] }))
            .unwrap_err();
        assert!(matches!(err, EngineError::Git { .. }), "{err}");
    }

    #[test]
    fn stage_and_unstage_lines() {
        let (r, mut s) = opened();
        let original = r.run("repo.status", &mut s, json!({})).unwrap();

        // `diff.worktree` e `diff.index` danno il digest di ogni file.
        let diff = r
            .run("diff.worktree", &mut s, json!({ "path": "src/lib.rs" }))
            .unwrap();
        let file = &diff["files"][0];
        let parsed: project_domain::FileDiff = serde_json::from_value(file.clone()).unwrap();
        assert_eq!(file["digest"], file_digest(&parsed));
        let all = r.run("diff.index", &mut s, json!({})).unwrap();
        assert!(
            all["files"]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| f["digest"].is_string())
        );
        let first = parsed.line_refs()[0];

        let out = r
            .run(
                "index.stage_lines",
                &mut s,
                json!({ "path": "src/lib.rs", "lines": [first], "digest": file["digest"] }),
            )
            .unwrap();
        assert_eq!(out["backup"], Value::Null);
        assert_eq!(
            out["status"],
            r.run("repo.status", &mut s, json!({})).unwrap()
        );
        let lib = out["status"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"] == "src/lib.rs")
            .unwrap();
        assert_eq!(
            (&lib["index"], &lib["worktree"]),
            (&json!("Modified"), &json!("Modified"))
        );

        // Lo stesso digest ora è vecchio: niente cambia.
        let err = r
            .run(
                "index.stage_lines",
                &mut s,
                json!({ "path": "src/lib.rs", "lines": [first], "digest": file["digest"] }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");

        // Righe che non esistono nel diff.
        let diff = r
            .run("diff.index", &mut s, json!({ "path": "src/lib.rs" }))
            .unwrap();
        let digest = diff["files"][0]["digest"].clone();
        let err = r
            .run(
                "index.unstage_lines",
                &mut s,
                json!({ "path": "src/lib.rs", "lines": [{ "old": 999 }], "digest": digest }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");

        // Unstage di tutte le righe in stage: si torna allo stato iniziale, con un backup
        // perché il file era `MM`.
        let parsed: project_domain::FileDiff =
            serde_json::from_value(diff["files"][0].clone()).unwrap();
        let out = r
            .run(
                "index.unstage_lines",
                &mut s,
                json!({ "path": "src/lib.rs", "lines": parsed.line_refs(), "digest": digest }),
            )
            .unwrap();
        assert_eq!(out["backup"], "refs/project/backup/index-1");
        assert_eq!(out["status"], original);
    }

    #[test]
    fn commit_create_returns_commit_previous_branch_and_status() {
        let (r, mut s) = opened();
        let head = MemoryBackend::sample().head().unwrap().target().unwrap();
        let out = r
            .run(
                "commit.create",
                &mut s,
                json!({ "message": "Aggiorna README\n\nCon la guida." }),
            )
            .unwrap();
        let commit: Commit = serde_json::from_value(out["commit"].clone()).unwrap();
        assert_eq!(commit.summary, "Aggiorna README");
        assert_eq!(commit.body, "Con la guida.");
        assert_eq!(commit.parents, [head]);
        assert_eq!(out["previous"], head.to_string());
        assert_eq!(out["branch"], "refs/heads/main");
        assert_eq!(out["backup"], Value::Null);
        assert_eq!(
            out["status"],
            r.run("repo.status", &mut s, json!({})).unwrap()
        );
        assert_eq!(
            out["status"]["head"]["Branch"]["target"],
            commit.id.to_string()
        );
        // Lo stesso commit è in cima a `repo.log`.
        let (log, _) = log_page(&r, &mut s, json!({ "limit": 1 }));
        assert_eq!(log, std::slice::from_ref(&commit));

        // Niente più in stage.
        let err = r
            .run("commit.create", &mut s, json!({ "message": "ancora" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NothingToCommit), "{err}");

        // `amend` di un commit non pubblicato: senza conferma, con il backup.
        let out = r
            .run(
                "commit.create",
                &mut s,
                json!({ "message": "Aggiorna README e guida", "amend": true }),
            )
            .unwrap();
        assert_eq!(out["previous"], commit.id.to_string());
        assert_eq!(out["backup"], "refs/project/backup/amend-1");
        assert_eq!(out["commit"]["parents"], json!([head.to_string()]));

        // HEAD pubblicato: `amend` chiede `confirm`.
        let mut repo = MemoryBackend::sample();
        repo.set_remote_branch("origin/main", head);
        let mut opener = MemoryOpener::new();
        opener.insert("pubblicato", repo);
        let mut s = Session::new(Arc::new(opener));
        r.run("repo.open", &mut s, json!({ "path": "pubblicato" }))
            .unwrap();
        let err = r
            .run(
                "commit.create",
                &mut s,
                json!({ "message": "x", "amend": true }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
        let out = r
            .run(
                "commit.create",
                &mut s,
                json!({ "message": "x", "amend": true, "confirm": true }),
            )
            .unwrap();
        assert_eq!(out["previous"], head.to_string());
    }

    #[test]
    fn undo_redo_and_history() {
        let (r, mut s) = opened();
        let start = MemoryBackend::sample().head().unwrap().target().unwrap();
        let original = r.run("repo.status", &mut s, json!({})).unwrap();
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(
            history,
            json!({
                "done": [], "undone": [],
                "can_undo": false, "can_redo": false,
                "next_undo": null, "next_redo": null
            })
        );
        let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        let err = r.run("edit.redo", &mut s, Value::Null).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

        let first = r
            .run("commit.create", &mut s, json!({ "message": "Uno" }))
            .unwrap();
        let first_id = first["commit"]["id"].as_str().unwrap().to_owned();
        // Stage e unstage non vanno nel diario.
        r.run("index.stage", &mut s, json!({ "paths": ["src/lib.rs"] }))
            .unwrap();
        let second = r
            .run("commit.create", &mut s, json!({ "message": "Due" }))
            .unwrap();
        let second_id = second["commit"]["id"].as_str().unwrap().to_owned();
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        let done = history["done"].as_array().unwrap();
        assert_eq!(done.len(), 2);
        assert_eq!(done[0]["id"], 1);
        assert_eq!(done[1]["command"], "commit.create");
        assert_eq!(
            done[1]["refs"],
            json!([{ "name": "refs/heads/main", "before": first_id, "after": second_id }])
        );
        assert_eq!(
            history["next_undo"],
            format!("Commit {} «Due»", &second_id[..7])
        );

        // Due undo di seguito.
        let out = r.run("edit.undo", &mut s, json!({})).unwrap();
        assert_eq!(out["undone"]["description"], done[1]["description"]);
        assert_eq!(out["status"]["head"]["Branch"]["target"], first_id);
        assert_eq!(out["can_undo"], true);
        assert_eq!(out["can_redo"], true);
        assert_eq!(out["backups"], json!(["refs/project/backup/undo-1"]));
        assert_eq!(out["warnings"], json!([]));
        assert_eq!(
            out["status"],
            r.run("repo.status", &mut s, json!({})).unwrap()
        );
        let out = r.run("edit.undo", &mut s, json!({})).unwrap();
        // Indice e worktree non si toccano: le modifiche dei due commit sono in stage
        // (anche `src/lib.rs`, messo in stage prima del secondo).
        assert_eq!(out["status"]["entries"][0], original["entries"][0]);
        assert_eq!(out["status"]["entries"][1], original["entries"][1]);
        assert_eq!(out["status"]["entries"][2]["path"], "src/lib.rs");
        assert_eq!(out["status"]["entries"][2]["index"], "Modified");
        assert_eq!(out["status"]["entries"][2]["worktree"], "Unmodified");
        assert_eq!(out["status"]["head"]["Branch"]["target"], start.to_string());
        assert_eq!(out["can_undo"], false);
        assert_eq!(out["next_redo"], format!("Commit {} «Uno»", &first_id[..7]));

        // Redo dell'ultimo annullato (il primo commit).
        let out = r.run("edit.redo", &mut s, json!({})).unwrap();
        assert_eq!(out["redone"]["id"], 1);
        assert_eq!(out["status"]["head"]["Branch"]["target"], first_id);
        assert_eq!(out["can_redo"], true);

        // Una nuova operazione svuota il redo.
        r.run("commit.create", &mut s, json!({ "message": "Altro" }))
            .unwrap();
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(history["can_redo"], false);
        assert_eq!(history["undone"], json!([]));
        assert_eq!(history["done"].as_array().unwrap().len(), 2);
        let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

        // Riaprire il repo azzera il diario.
        r.run(
            "repo.open",
            &mut s,
            json!({ "path": MemoryOpener::SAMPLE_PATH }),
        )
        .unwrap();
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(history["done"], json!([]));

        // Senza repo.
        let mut empty = session();
        for id in ["edit.undo", "edit.redo", "edit.history"] {
            let err = r.run(id, &mut empty, json!({})).unwrap_err();
            assert!(matches!(err, EngineError::NoRepo), "{id}: {err}");
        }
    }

    #[test]
    fn discard_needs_confirm_and_undo_restores_the_files() {
        let (r, mut s) = opened();
        let sample = MemoryBackend::sample();
        let before = r.run("repo.status", &mut s, json!({})).unwrap();
        let diff_before = r.run("diff.worktree", &mut s, json!({})).unwrap();

        // Senza `confirm`: niente cambia, l'errore dice cosa si perderebbe.
        for params in [
            json!({ "paths": ["src", "README.md"] }),
            json!({ "paths": ["src", "README.md"], "confirm": false }),
        ] {
            let err = r.run("worktree.discard", &mut s, params).unwrap_err();
            let EngineError::ConfirmRequired { command, reason } = &err else {
                panic!("{err}");
            };
            assert_eq!(*command, "worktree.discard");
            assert!(reason.contains("`README.md`, `src/lib.rs`"), "{reason}");
            assert!(err.to_string().contains("\"confirm\": true"), "{err}");
        }
        assert_eq!(r.run("repo.status", &mut s, json!({})).unwrap(), before);
        assert_eq!(
            r.run("edit.history", &mut s, json!({})).unwrap()["done"],
            json!([])
        );
        // Un rifiuto viene prima della conferma.
        let err = r
            .run("worktree.discard", &mut s, json!({ "paths": ["note.txt"] }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

        let out = r
            .run(
                "worktree.discard",
                &mut s,
                json!({ "paths": ["src", "README.md"], "confirm": true }),
            )
            .unwrap();
        assert_eq!(out["backup"], "refs/project/backup/discard-1");
        assert_eq!(out["files"][0]["path"], "README.md");
        assert_eq!(out["files"][0]["change"], "Modified");
        assert_eq!(out["files"][1]["path"], "src/lib.rs");
        assert_eq!(out["stash"].as_str().unwrap().len(), 40);
        assert_eq!(out["can_undo"], true);
        assert_eq!(
            out["next_undo"],
            "Scarta le modifiche di 2 file («README.md», «src/lib.rs»)"
        );
        assert_eq!(
            out["status"],
            r.run("repo.status", &mut s, json!({})).unwrap()
        );
        let diff = r.run("diff.worktree", &mut s, json!({})).unwrap();
        assert_eq!(diff["files"], json!([]));
        // Il diff in stage non cambia.
        assert_eq!(
            r.run("diff.index", &mut s, json!({})).unwrap(),
            to_json_diff(&sample, DiffSpec::Index { path: None })
        );

        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        let entry = &history["done"][0];
        assert_eq!(entry["command"], "worktree.discard");
        assert_eq!(entry["refs"], json!([]));
        assert_eq!(entry["backup"], "refs/project/backup/discard-1");
        assert_eq!(entry["worktree"]["stash"], out["stash"]);
        assert_eq!(entry["worktree"]["files"], out["files"]);

        let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
        assert_eq!(undo["undone"]["command"], "worktree.discard");
        assert_eq!(undo["status"], before);
        assert_eq!(
            r.run("diff.worktree", &mut s, json!({})).unwrap(),
            diff_before
        );
        let redo = r.run("edit.redo", &mut s, json!({})).unwrap();
        assert_eq!(redo["status"], out["status"]);
    }

    fn to_json_diff(repo: &MemoryBackend, spec: DiffSpec) -> Value {
        diff_with_digests(&repo.diff(&spec).unwrap())
    }

    #[test]
    fn undo_of_an_amend() {
        let (r, mut s) = opened();
        let out = r
            .run("commit.create", &mut s, json!({ "message": "Uno" }))
            .unwrap();
        let out2 = r
            .run(
                "commit.create",
                &mut s,
                json!({ "message": "Uno bis", "amend": true }),
            )
            .unwrap();
        assert_eq!(out2["previous"], out["commit"]["id"]);
        let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
        assert_eq!(
            undo["status"]["head"]["Branch"]["target"],
            out["commit"]["id"]
        );
        // Il commit di `amend` annullato è salvato; quello riscritto era nel backup di amend.
        assert_eq!(undo["backups"], json!(["refs/project/backup/undo-1"]));
        assert_eq!(
            undo["undone"]["backup"],
            json!("refs/project/backup/amend-1")
        );
    }

    #[test]
    fn errors() {
        let (r, mut s) = opened();
        assert!(matches!(
            r.run("nope", &mut s, json!({})),
            Err(EngineError::UnknownCommand(_))
        ));
        for (id, params) in [
            ("repo.open", json!({})),
            ("repo.open", json!({ "path": "x", "altro": 1 })),
            ("repo.status", json!({ "x": 1 })),
            ("repo.log", json!({ "limit": -1 })),
            ("repo.log", json!({ "page": 2 })),
            ("commit.show", json!({})),
            ("commit.show", json!({ "id": 3 })),
            ("commit.show", json!({ "id": "HEAD", "path": "a" })),
            ("diff.worktree", json!({ "id": "HEAD" })),
            ("diff.index", json!({ "paths": ["a"] })),
            ("diff.index", json!({ "path": 3 })),
            ("diff.commit", json!({})),
            ("diff.commit", json!({ "path": "a" })),
            ("diff.commit", json!({ "id": "HEAD", "rev": "HEAD" })),
            ("index.stage", json!({})),
            ("index.stage", json!({ "paths": [] })),
            ("index.stage", json!({ "paths": "README.md" })),
            ("index.stage", json!({ "path": "README.md" })),
            (
                "index.unstage",
                json!({ "paths": ["README.md"], "confirm": true }),
            ),
            ("index.unstage", json!({ "paths": [3] })),
            ("index.stage_lines", json!({})),
            (
                "index.stage_lines",
                json!({ "path": "src/lib.rs", "lines": [{ "new": 2 }] }),
            ),
            (
                "index.stage_lines",
                json!({ "path": "src/lib.rs", "lines": [], "digest": "x" }),
            ),
            (
                "index.stage_lines",
                json!({ "path": "src/lib.rs", "lines": [2], "digest": "x" }),
            ),
            (
                "index.stage_lines",
                json!({ "path": "src/lib.rs", "lines": [{ "riga": 2 }], "digest": "x" }),
            ),
            (
                "index.stage_lines",
                json!({ "path": "src/lib.rs", "lines": [{ "new": -1 }], "digest": "x" }),
            ),
            (
                "index.unstage_lines",
                json!({ "path": "README.md", "lines": [{ "new": 1 }], "digest": "x", "confirm": true }),
            ),
            (
                "index.unstage_lines",
                json!({ "paths": ["README.md"], "lines": [{ "new": 1 }], "digest": "x" }),
            ),
            ("commit.create", json!({})),
            ("commit.create", json!({ "message": 3 })),
            ("commit.create", json!({ "message": "" })),
            ("commit.create", json!({ "message": " \n\t\n" })),
            ("commit.create", json!({ "message": "x", "amend": "sì" })),
            ("commit.create", json!({ "message": "x", "paths": ["a"] })),
            (
                "commit.create",
                json!({ "message": "x", "allow_empty": true }),
            ),
            (
                "commit.create",
                json!({ "message": "x", "no_verify": true }),
            ),
            ("worktree.discard", json!({})),
            ("worktree.discard", json!({ "confirm": true })),
            ("worktree.discard", json!({ "paths": [], "confirm": true })),
            (
                "worktree.discard",
                json!({ "paths": ["README.md"], "confirm": "sì" }),
            ),
            (
                "worktree.discard",
                json!({ "paths": ["README.md"], "confirm": true, "untracked": true }),
            ),
            (
                "worktree.discard",
                json!({ "path": "README.md", "confirm": true }),
            ),
        ] {
            let err = r.run(id, &mut s, params).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{id}: {err}"
            );
        }
        // Percorsi che non sono percorsi di file del repo.
        for path in [
            json!(""),
            json!("/etc/passwd"),
            json!("../fuori"),
            json!("a/../b"),
            json!("./a"),
            json!("a//b"),
            json!("cartella/"),
            json!([97, 0, 98]),
        ] {
            for (id, params) in [
                ("diff.worktree", json!({ "path": path })),
                ("diff.index", json!({ "path": path })),
                ("diff.commit", json!({ "id": "HEAD", "path": path })),
                ("index.stage", json!({ "paths": ["README.md", path] })),
                ("index.unstage", json!({ "paths": [path] })),
                (
                    "index.stage_lines",
                    json!({ "path": path, "lines": [{ "new": 1 }], "digest": "x" }),
                ),
                (
                    "index.unstage_lines",
                    json!({ "path": path, "lines": [{ "old": 1 }], "digest": "x" }),
                ),
            ] {
                let err = r.run(id, &mut s, params).unwrap_err();
                assert!(
                    matches!(err, EngineError::InvalidParams { .. }),
                    "{id} {path}: {err}"
                );
            }
        }
        let err = r
            .run("repo.open", &mut s, json!({ "path": "altrove" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::RepoNotFound { .. }), "{err}");
        assert_eq!(s.path(), Some(MemoryOpener::SAMPLE_PATH));
    }
}
