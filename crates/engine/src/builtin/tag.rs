//! Commands of the local tags: `tag.create`, `tag.delete`. The checks that do not depend on the
//! backend are here; the writes are in the backends
//! ([`GitBackend::create_tag`](crate::GitBackend::create_tag),
//! [`GitBackend::delete_tag`](crate::GitBackend::delete_tag)). Both enter the journal: `edit.undo`
//! deletes the created tag or recreates the deleted one on the same value (§7).

use super::{Out, journal_state, to_json};
use crate::commands::parse_params;
use crate::{
    EngineError, HeadState, JournalEntry, LogQuery, Session, check_new_tag_name, check_tag_name,
    tag_ref,
};
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

/// Parameters of `tag.delete`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteParams {
    name: String,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    expected: Option<String>,
}

/// Deletes the local tag `name` (any existing name, even `-x`): **always** needs `"confirm":
/// true`, otherwise [`EngineError::ConfirmRequired`] with the commit and the kind of tag (annotated:
/// its message and author exist only in the tag object) and the value in `expected`. With
/// `expected` (full id of the value seen) a tag that now has another value is
/// [`EngineError::NotAllowed`] and nothing changes. Before deleting, the value as git stores
/// it (the tag object of an annotated tag, the commit of a lightweight one) is saved in
/// `refs/project/backup/tag-delete-...`; if the backup fails nothing is deleted (see
/// [`GitBackend::delete_tag`](crate::GitBackend::delete_tag)). The remote is not touched. The
/// operation goes into the journal: `edit.undo` recreates the tag on the same value, only if in the
/// meantime nobody created one with that name.
///
/// Result: `{"tag": "refs/tags/<name>", "target": oid (the commit), "annotated": bool,
/// "tag_object": oid | null, "backup": string, "refs": [RefInfo], "can_undo", "can_redo",
/// "next_undo", "next_redo"}`.
pub(super) fn tag_delete(s: &mut Session, p: Value) -> Out {
    const ID: &str = "tag.delete";
    let DeleteParams {
        name,
        confirm,
        expected,
    } = parse_params(ID, p)?;
    check_tag_name(ID, &name)?;
    let expected = expected
        .map(|hex| {
            let full = hex.len() == Oid::HEX_LEN && hex.bytes().all(|b| b.is_ascii_hexdigit());
            full.then(|| hex.to_ascii_lowercase().parse::<Oid>().ok())
                .flatten()
                .ok_or_else(|| EngineError::InvalidParams {
                    id: ID,
                    reason: msg!(
                        "engine-tag-expected-hex",
                        len = Oid::HEX_LEN,
                        value = hex.escape_debug()
                    ),
                })
        })
        .transpose()?;
    let (repo, journal) = s.repo_and_journal()?;
    let head = repo.head()?;
    let full = tag_ref(&name);
    let refs = repo.refs()?;
    let tag = refs.iter().find(|r| r.name == full).ok_or_else(|| {
        EngineError::NotAllowed(msg!("engine-tag-not-found", name = name.escape_debug()))
    })?;
    let target = tag.target;
    let tag_object = tag.tag_object;
    let value = tag_object.unwrap_or(target);
    if let Some(seen) = expected
        && seen != value
    {
        return Err(EngineError::NotAllowed(msg!(
            "engine-tag-moved",
            name = name.escape_debug(),
            seen = seen,
            value = value
        )));
    }
    if !confirm {
        let reason = match tag_object {
            Some(object) => msg!(
                "engine-tag-delete-confirm-annotated",
                name = name.escape_debug(),
                target = target,
                object = object
            ),
            None => msg!(
                "engine-tag-delete-confirm",
                name = name.escape_debug(),
                target = target
            ),
        };
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason,
            expected: Some(value.to_string()),
        });
    }
    let backup = repo.delete_tag(&name, &value)?;
    journal.record(JournalEntry::tag_delete(
        &full,
        value,
        target,
        HeadState::from_head(&head),
        backup.clone(),
    ));
    let mut out = journal_state(journal);
    out["tag"] = Value::String(full);
    out["target"] = Value::String(target.to_string());
    out["annotated"] = Value::Bool(tag_object.is_some());
    out["tag_object"] = tag_object.map_or(Value::Null, |o| Value::String(o.to_string()));
    out["backup"] = Value::String(backup);
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

    #[test]
    fn delete_always_asks_and_undo_puts_back_the_same_tag_object() {
        let (r, mut s) = sample();
        let before = refs(&r, &mut s);
        let v1 = tag(&before, "refs/tags/v0.1.0").unwrap().clone();
        let object = v1["tag_object"].as_str().unwrap().to_owned();
        let err = r
            .run("tag.delete", &mut s, json!({ "name": "v0.1.0" }))
            .unwrap_err();
        let EngineError::ConfirmRequired {
            reason, expected, ..
        } = &err
        else {
            panic!("{err}");
        };
        assert!(reason.contains("annotated"), "{reason}");
        assert!(reason.contains(&object), "{reason}");
        assert_eq!(expected.as_deref(), Some(object.as_str()));
        assert_eq!(refs(&r, &mut s), before);

        // An `expected` that is not the value now (the tag moved since it was seen): nothing deleted.
        let moved = MemoryBackend::fake_oid(12).to_string();
        let err = r
            .run(
                "tag.delete",
                &mut s,
                json!({ "name": "v0.1.0", "confirm": true, "expected": moved }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        assert_eq!(refs(&r, &mut s), before);

        let out = r
            .run(
                "tag.delete",
                &mut s,
                json!({ "name": "v0.1.0", "confirm": true, "expected": object }),
            )
            .unwrap();
        assert_eq!(out["tag"], "refs/tags/v0.1.0");
        assert_eq!(out["annotated"], true);
        assert_eq!(out["tag_object"], object.as_str());
        assert_eq!(out["target"], v1["target"]);
        assert_eq!(out["backup"], "refs/project/backup/tag-delete-1");
        assert_eq!(out["can_undo"], true);
        assert!(tag(&out, "refs/tags/v0.1.0").is_none());

        r.run("edit.undo", &mut s, json!({})).unwrap();
        assert_eq!(tag(&refs(&r, &mut s), "refs/tags/v0.1.0"), Some(&v1));
        r.run("edit.redo", &mut s, json!({})).unwrap();
        assert!(tag(&refs(&r, &mut s), "refs/tags/v0.1.0").is_none());
        r.run("edit.undo", &mut s, json!({})).unwrap();
        assert_eq!(refs(&r, &mut s), before);
    }

    #[test]
    fn delete_lightweight_and_refusals() {
        let mut opener = MemoryOpener::default();
        let mut repo = MemoryBackend::sample();
        repo.set_tag("-x", MemoryBackend::fake_oid(11), None);
        opener.insert("r", repo);
        let mut s = Session::new(Arc::new(opener));
        let r = Registry::builtin();
        r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
        let err = r
            .run("tag.delete", &mut s, json!({ "name": "-x" }))
            .unwrap_err();
        let EngineError::ConfirmRequired { reason, .. } = &err else {
            panic!("{err}");
        };
        assert!(reason.contains("lightweight"), "{reason}");
        let out = r
            .run(
                "tag.delete",
                &mut s,
                json!({ "name": "-x", "confirm": true }),
            )
            .unwrap();
        assert_eq!(out["annotated"], false);
        assert_eq!(out["tag_object"], Value::Null);
        assert!(tag(&out, "refs/tags/-x").is_none());
        r.run("edit.undo", &mut s, json!({})).unwrap();
        let x = tag(&refs(&r, &mut s), "refs/tags/-x").unwrap().clone();
        assert_eq!(x["target"], MemoryBackend::fake_oid(11).to_string());

        let err = r
            .run(
                "tag.delete",
                &mut s,
                json!({ "name": "nothing", "confirm": true }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        for name in ["", "a b", "refs/tags/-x"] {
            let err = r
                .run(
                    "tag.delete",
                    &mut s,
                    json!({ "name": name, "confirm": true }),
                )
                .unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{name}: {err}"
            );
        }
        let err = r
            .run(
                "tag.delete",
                &mut s,
                json!({ "name": "-x", "expected": "x" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    }
}
