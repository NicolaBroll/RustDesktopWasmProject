//! Commands of the local tags: `tag.create`, `tag.delete`. The checks that do not depend on the
//! backend are here; the writes are in the backends
//! ([`GitBackend::create_tag`](crate::GitBackend::create_tag),
//! [`GitBackend::delete_tag`](crate::GitBackend::delete_tag)). Both enter the journal: `edit.undo`
//! deletes the created tag or recreates the deleted one on the same value (§7).

use super::{JournalState, Out, reread, to_json};
use crate::{
    EngineError, HeadState, JournalEntry, LogQuery, Session, check_new_tag_name, check_tag_name,
    tag_ref,
};
use project_domain::{Oid, RefInfo};
use serde::{Deserialize, Serialize};

/// Parameters of `tag.create`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateParams {
    name: String,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Result of `tag.create`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagCreateResult {
    /// The full name of the new tag, `refs/tags/<name>`.
    pub tag: String,
    /// The commit the tag points to.
    pub target: Oid,
    /// `true` for an annotated tag (created with a message), `false` for a lightweight one.
    pub annotated: bool,
    /// The refs re-read after the creation; `null` if they could not be re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Result of `tag.delete`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagDeleteResult {
    /// The full name of the deleted tag, `refs/tags/<name>`.
    pub tag: String,
    /// The commit the tag pointed to (peeled, also for an annotated tag).
    pub target: Oid,
    /// The tag was annotated (it had a tag object).
    pub annotated: bool,
    /// The tag object of an annotated tag, `null` for a lightweight one.
    pub tag_object: Option<Oid>,
    /// The backup ref that keeps the deleted value (`refs/project/backup/tag-delete-...`).
    pub backup: String,
    /// The refs re-read after the deletion; `null` if they could not be re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Creates the tag `name` on `target` (a rev: id, also abbreviated, branch, remote branch or tag;
/// HEAD if missing): annotated with `message` (not empty after trimming), lightweight without (see
/// [`GitBackend::create_tag`](crate::GitBackend::create_tag)). An existing tag is never
/// overwritten. The operation goes into the journal: `edit.undo` deletes the tag, only if in the
/// meantime it was not moved.
///
/// Result: [`TagCreateResult`].
pub(super) fn tag_create(
    s: &mut Session,
    CreateParams {
        name,
        target,
        message,
    }: CreateParams,
) -> Out {
    const ID: &str = "tag.create";
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
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(TagCreateResult {
        tag: full,
        target,
        annotated: message.is_some(),
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
}

/// Parameters of `tag.delete`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteParams {
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
/// Result: [`TagDeleteResult`].
pub(super) fn tag_delete(
    s: &mut Session,
    DeleteParams {
        name,
        confirm,
        expected,
    }: DeleteParams,
) -> Out {
    const ID: &str = "tag.delete";
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
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(TagDeleteResult {
        tag: full,
        target,
        annotated: tag_object.is_some(),
        tag_object,
        backup,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
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

    /// The typed results read back the JSON of the commands and give the same JSON again: the
    /// UI deserializes into them.
    #[test]
    fn results_round_trip() {
        use super::{TagCreateResult, TagDeleteResult};
        fn same<T: serde::de::DeserializeOwned + serde::Serialize>(out: Value) {
            let typed: T = serde_json::from_value(out.clone()).unwrap();
            assert_eq!(serde_json::to_value(&typed).unwrap(), out);
        }
        let (r, mut s) = sample();
        let out = r
            .run("tag.create", &mut s, json!({ "name": "v2" }))
            .unwrap();
        assert!(out.get("refs_error").is_none());
        same::<TagCreateResult>(out);
        let out = r
            .run(
                "tag.create",
                &mut s,
                json!({ "name": "v3", "message": "Version 3" }),
            )
            .unwrap();
        same::<TagCreateResult>(out);
        // Annotated (with a tag object) and lightweight (`tag_object: null`).
        for name in ["v0.1.0", "v2"] {
            let out = r
                .run(
                    "tag.delete",
                    &mut s,
                    json!({ "name": name, "confirm": true }),
                )
                .unwrap();
            assert!(out.get("refs_error").is_none());
            same::<TagDeleteResult>(out);
        }
    }

    /// A re-read that fails gives `null` and `refs_error`.
    #[test]
    fn reread_error_fields() {
        use super::super::{JournalState, reread};
        use super::TagDeleteResult;
        use project_domain::{Oid, RefInfo};
        let (refs, refs_error) = reread::<Vec<RefInfo>>(Err(EngineError::NoRepo));
        let result = TagDeleteResult {
            tag: "refs/tags/x".into(),
            target: Oid::from_bytes([1; 20]),
            annotated: false,
            tag_object: None,
            backup: "refs/project/backup/tag-delete-1".into(),
            refs,
            refs_error,
            journal: JournalState::default(),
        };
        let out = serde_json::to_value(&result).unwrap();
        assert_eq!(out["refs"], Value::Null);
        assert_eq!(out["tag_object"], Value::Null);
        assert_eq!(out["refs_error"], EngineError::NoRepo.to_string());
        let back: TagDeleteResult = serde_json::from_value(out).unwrap();
        assert_eq!(back, result);
    }
}
