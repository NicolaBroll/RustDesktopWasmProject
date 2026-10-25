//! Commands of the tags: `tag.create`. The checks that do not depend on the backend are here; the
//! write is in the backends ([`GitBackend::create_tag`](crate::GitBackend::create_tag)). The
//! creation enters the journal: `edit.undo` deletes the tag (§7).

use super::{Out, journal_state, to_json};
use crate::commands::parse_params;
use crate::{EngineError, HeadState, JournalEntry, LogQuery, Session, check_new_tag_name, tag_ref};
use project_domain::Oid;
use serde::Deserialize;
use serde_json::Value;

/// Parameters of `tag.create`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateParams {
    name: String,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Creates the tag `name` on `target` (a rev: id, also abbreviated, branch, remote branch or tag;
/// HEAD if missing): annotated with `message` (not empty after trimming), lightweight without (see
/// [`GitBackend::create_tag`](crate::GitBackend::create_tag)). An existing tag is never
/// overwritten. The operation goes into the journal: `edit.undo` deletes the tag, only if in the
/// meantime it was not moved.
///
/// Result: `{"tag": "refs/tags/<name>", "target": oid, "annotated": bool, "refs": [RefInfo],
/// "can_undo", "can_redo", "next_undo", "next_redo"}`.
pub(super) fn tag_create(s: &mut Session, p: Value) -> Out {
    const ID: &str = "tag.create";
    let CreateParams {
        name,
        target,
        message,
    } = parse_params(ID, p)?;
    check_new_tag_name(ID, &name)?;
    let message = message.filter(|m| !m.trim().is_empty());
    let (repo, journal) = s.repo_and_journal()?;
    let head = repo.head()?;
    let target: Oid = match &target {
        Some(rev) => {
            let commits = repo.log(&LogQuery::page(0, 1).from_rev(rev.clone()))?;
            commits
                .first()
                .map(|c| c.id)
                .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?
        }
        None => head
            .target()
            .ok_or_else(|| EngineError::NotAllowed(msg!("engine-tag-unborn-target")))?,
    };
    let full = tag_ref(&name);
    if repo.refs()?.iter().any(|r| r.name == full) {
        return Err(EngineError::NotAllowed(msg!(
            "engine-tag-exists",
            name = name.escape_debug()
        )));
    }
    let value = repo.create_tag(&name, &target, message.as_deref())?;
    journal.record(JournalEntry::tag_create(
        &full,
        value,
        target,
        HeadState::from_head(&head),
    ));
    let mut out = journal_state(journal);
    out["tag"] = Value::String(full);
    out["target"] = Value::String(target.to_string());
    out["annotated"] = Value::Bool(message.is_some());
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
mod tests {
    use crate::{EngineError, MemoryBackend, MemoryOpener, Registry, Session};
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn sample() -> (Registry, Session) {
        let mut opener = MemoryOpener::default();
        opener.insert("r", MemoryBackend::sample());
        let mut s = Session::new(Arc::new(opener));
        let r = Registry::builtin();
        r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
        (r, s)
    }

    fn tag<'a>(out: &'a Value, name: &str) -> Option<&'a Value> {
        out["refs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
    }

    /// The refs in the same shape as the result of `tag.create`.
    fn refs(r: &Registry, s: &mut Session) -> Value {
        json!({ "refs": r.run("repo.refs", s, json!({})).unwrap() })
    }

    #[test]
    fn lightweight_and_annotated_with_undo_and_redo() {
        let (r, mut s) = sample();
        let head = r.run("repo.status", &mut s, json!({})).unwrap();
        let out = r
            .run("tag.create", &mut s, json!({ "name": "v2" }))
            .unwrap();
        assert_eq!(out["tag"], "refs/tags/v2");
        assert_eq!(out["annotated"], false);
        assert_eq!(out["can_undo"], true);
        let v2 = tag(&out, "refs/tags/v2").unwrap();
        assert_eq!(v2["tag_object"], Value::Null);
        assert_eq!(head["head"]["Branch"]["target"], v2["target"]);

        let out = r
            .run(
                "tag.create",
                &mut s,
                json!({ "name": "release/v3", "target": "origin/main", "message": "Version 3" }),
            )
            .unwrap();
        assert_eq!(out["annotated"], true);
        let v3 = tag(&out, "refs/tags/release/v3").unwrap().clone();
        assert!(v3["tag_object"].is_string(), "{v3}");

        r.run("edit.undo", &mut s, json!({})).unwrap();
        assert!(tag(&refs(&r, &mut s), "refs/tags/release/v3").is_none());
        r.run("edit.redo", &mut s, json!({})).unwrap();
        assert_eq!(tag(&refs(&r, &mut s), "refs/tags/release/v3"), Some(&v3));
        r.run("edit.undo", &mut s, json!({})).unwrap();
        r.run("edit.undo", &mut s, json!({})).unwrap();
        assert!(tag(&refs(&r, &mut s), "refs/tags/v2").is_none());
    }

    #[test]
    fn never_overwrites_and_checks_the_name() {
        let (r, mut s) = sample();
        let before = refs(&r, &mut s);
        let err = r
            .run("tag.create", &mut s, json!({ "name": "v0.1.0" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        for name in ["", "-x", "a b", "refs/tags/x"] {
            let err = r
                .run("tag.create", &mut s, json!({ "name": name }))
                .unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{name}: {err}"
            );
        }
        let err = r
            .run(
                "tag.create",
                &mut s,
                json!({ "name": "x", "target": "nothing" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
        assert_eq!(refs(&r, &mut s), before);
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(history["can_undo"], false);
    }
}
