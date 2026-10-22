//! Catalogo dei comandi predefiniti.

use crate::commands::parse_params;
use crate::{CommandSpec, EngineError, LogQuery, Registry, Session};
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
        r
    }
}

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
    use project_domain::{Commit, RefInfo, Status};
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
        assert_eq!(ids, ["repo.log", "repo.open", "repo.refs", "repo.status"]);
        let ids: Vec<_> = r.search("CRONOLOGIA").map(|c| c.id).collect();
        assert_eq!(ids, ["repo.log"]);
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
        for id in ["repo.status", "repo.refs", "repo.log"] {
            let err = r.run(id, &mut s, json!({})).unwrap_err();
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
        ] {
            let err = r.run(id, &mut s, params).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{id}: {err}"
            );
        }
        let err = r
            .run("repo.open", &mut s, json!({ "path": "altrove" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::RepoNotFound { .. }), "{err}");
        assert_eq!(s.path(), Some(MemoryOpener::SAMPLE_PATH));
    }
}
