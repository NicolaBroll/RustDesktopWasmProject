//! Catalogo dei comandi predefiniti.

use crate::commands::parse_params;
use crate::{CommandSpec, DiffSpec, EngineError, LogQuery, Registry, Session};
use project_domain::RepoPath;
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
        r
    }
}

/// I comandi che modificano il repository ([`CommandSpec::writes`]).
const WRITES: &[&str] = &["index.stage", "index.unstage"];

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

/// Il [`Diff`](project_domain::Diff) del worktree rispetto all'indice (modifiche non in
/// stage): `{"files": [...]}`. I file non tracciati non compaiono.
fn diff_worktree(s: &mut Session, p: Value) -> Out {
    let DiffParams { path } = parse_params("diff.worktree", p)?;
    let path = check_opt_path("diff.worktree", path)?;
    Ok(to_json(s.repo()?.diff(&DiffSpec::Worktree { path })?))
}

/// Il [`Diff`](project_domain::Diff) dell'indice rispetto a HEAD (modifiche in stage).
fn diff_index(s: &mut Session, p: Value) -> Out {
    let DiffParams { path } = parse_params("diff.index", p)?;
    let path = check_opt_path("diff.index", path)?;
    Ok(to_json(s.repo()?.diff(&DiffSpec::Index { path })?))
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
                "commit.show",
                "diff.commit",
                "diff.index",
                "diff.worktree",
                "index.stage",
                "index.unstage",
                "repo.log",
                "repo.open",
                "repo.refs",
                "repo.status"
            ]
        );
        let ids: Vec<_> = r.search("CRONOLOGIA").map(|c| c.id).collect();
        assert_eq!(ids, ["repo.log"]);
        let writes: Vec<_> = r.list().filter(|c| c.writes).map(|c| c.id).collect();
        assert_eq!(writes, ["index.stage", "index.unstage"]);
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
        ] {
            let params = if matches!(id, "commit.show" | "diff.commit") {
                json!({ "id": "HEAD" })
            } else if id.starts_with("index.") {
                json!({ "paths": ["README.md"] })
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
