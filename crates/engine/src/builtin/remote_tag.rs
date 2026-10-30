//! `remote.push_tag` and `remote.delete_tag`: tags on the remote (`impl-docs/git/sync.md`).
//!
//! Like `remote.push` and `remote.delete_branch` they are slow, write (to the remote) and do not go
//! into the journal: the remote belongs to others.

use super::{Out, reread, to_json};
use crate::{
    EngineError, Message, RemoteTagDelete, Session, TagPush, check_remote_name, check_tag_name,
    tag_ref, unknown_remote,
};
use project_domain::{Oid, RefInfo};
use serde::{Deserialize, Serialize};

/// How `remote.push_tag` updated the remote (`kind` of [`RemotePushTagResult`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagPushKind {
    /// The remote did not have the tag: created.
    New,
    /// The remote already had the tag with the same value: nothing published.
    UpToDate,
}

/// Result of `remote.push_tag`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemotePushTagResult {
    /// The remote pushed to.
    pub remote: String,
    /// The full name of the tag (`refs/tags/<name>`).
    pub tag: String,
    /// The tag name.
    pub name: String,
    /// The commit the tag points to.
    pub target: Oid,
    /// The tag is annotated.
    pub annotated: bool,
    /// The value as stored: the tag object of an annotated tag, otherwise the commit.
    pub value: Oid,
    /// What changed on the remote.
    pub kind: TagPushKind,
    /// The git summary (`[new tag]`, ...).
    pub summary: String,
    /// The git lines that are not progress (`To <url>`, warnings, hook output).
    pub messages: Vec<String>,
    /// The refs re-read after the push; `null` if they could not be re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
}

/// Result of `remote.delete_tag`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteDeleteTagResult {
    /// The remote the tag was deleted from.
    pub remote: String,
    /// The full name of the tag (`refs/tags/<name>`).
    pub tag: String,
    /// The tag name.
    pub name: String,
    /// The value the tag had on the remote (the tag object of an annotated tag).
    pub deleted: Oid,
    /// The backup ref that keeps the value (`refs/project/backup/push-delete-...`).
    pub backup: String,
    /// The git summary (`[deleted]`).
    pub summary: String,
    /// The git lines that are not progress (`To <url>`, warnings, hook output).
    pub messages: Vec<String>,
    /// Deleted only on some of the push URLs: the tag may still be on the remote.
    pub incomplete: bool,
    /// The warning of an incomplete deletion.
    pub warnings: Vec<Message>,
    /// The refs re-read after the deletion; `null` if they could not be re-read (see
    /// `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
}

/// The remote `remote` must be configured and not a mirror (`remote.<r>.mirror=true`: git
/// refuses an explicit refspec toward a mirror, which publishes all the refs together).
fn check_push_remote(s: &mut Session, remote: &str) -> Result<(), EngineError> {
    let repo = s.repo()?;
    let remotes = repo.remotes()?;
    if !remotes.iter().any(|r| r == remote) {
        return Err(unknown_remote(remote, &remotes));
    }
    let mirror = repo
        .config_values(&format!("remote.{remote}.mirror"))?
        .into_iter()
        .last();
    if mirror.as_deref().is_some_and(|v| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "" | "true" | "yes" | "on" | "1"
        )
    }) {
        return Err(EngineError::NotAllowed(msg!(
            "engine-remote-tag-mirror",
            remote = remote.escape_debug()
        )));
    }
    Ok(())
}

/// The refs re-read after the command, as `refs` and `refs_error`.
fn refs(s: &Session) -> (Option<Vec<RefInfo>>, Option<String>) {
    reread(s.repo().and_then(|repo| repo.refs()))
}

/// Parameters of `remote.push_tag`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PushParams {
    remote: String,
    name: String,
}

/// Publishes the local tag `name` to the remote `remote`: only
/// `refs/tags/<name>:refs/tags/<name>` (see [`GitBackend::push_tag`](crate::GitBackend::push_tag)),
/// never forced. The remote already having the same value is a success (`"kind": "up_to_date"`);
/// a tag already on the remote with another value is [`EngineError::PushRejected`] with
/// `tag_exists` (nothing changes: deleting it from the remote first is a separate, confirmed
/// command). The local tag must exist ([`EngineError::NotAllowed`] otherwise). Errors as
/// `remote.push`. Slow, does not go into the journal.
///
/// Result: [`RemotePushTagResult`].
pub(super) fn remote_push_tag(s: &mut Session, PushParams { remote, name }: PushParams) -> Out {
    const ID: &str = "remote.push_tag";
    check_remote_name(ID, &remote)?;
    check_tag_name(ID, &name)?;
    let job = s.job().clone();
    check_push_remote(s, &remote)?;
    let full = tag_ref(&name);
    let repo = s.repo_mut()?;
    let tag = repo
        .refs()?
        .into_iter()
        .find(|r| r.name == full)
        .ok_or_else(|| {
            EngineError::NotAllowed(msg!("engine-tag-not-found", name = name.escape_debug()))
        })?;
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    let pushed = repo.push_tag(
        &TagPush {
            remote: remote.clone(),
            name: name.clone(),
        },
        &job,
    )?;
    let (refs, refs_error) = refs(s);
    Ok(to_json(RemotePushTagResult {
        remote,
        tag: full,
        name,
        target: tag.target,
        annotated: tag.tag_object.is_some(),
        value: tag.tag_object.unwrap_or(tag.target),
        kind: if pushed.up_to_date {
            TagPushKind::UpToDate
        } else {
            TagPushKind::New
        },
        summary: pushed.summary,
        messages: pushed.messages,
        refs,
        refs_error,
    }))
}

/// Parameters of `remote.delete_tag`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteParams {
    remote: String,
    name: String,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    expected: Option<String>,
}

/// Deletes the tag `name` from the remote `remote` (`git push <remote> :refs/tags/<name>`): the tag
/// disappears for everyone who uses the remote, so it **always** needs `"confirm": true` with
/// `"expected"`, the full value of the tag on the remote (the tag object of an annotated tag) shown
/// by the confirmation. There is no remote-tracking ref for tags: the value is read from the remote
/// (`git ls-remote`). Without confirmation [`EngineError::ConfirmRequired`] with that value
/// (in `expected`) and nothing changes; an `expected` that is not the value read now asks again;
/// a tag the remote does not have is [`EngineError::NotAllowed`]. The local tag is not touched.
///
/// Before the push the value is saved in `refs/project/backup/push-delete-...` (fetched from the
/// remote if the object is not here); then the deletion with
/// `--force-with-lease=refs/tags/<name>:<expected>`: if the tag moved on the remote in the meantime
/// git rejects it ([`EngineError::PushRejected`] with `stale_lease`). Backup and errors as
/// `remote.delete_branch` (see
/// [`GitBackend::delete_remote_tag`](crate::GitBackend::delete_remote_tag)). Slow, not in the journal.
///
/// Result: [`RemoteDeleteTagResult`].
pub(super) fn remote_delete_tag(
    s: &mut Session,
    DeleteParams {
        remote,
        name,
        confirm,
        expected,
    }: DeleteParams,
) -> Out {
    const ID: &str = "remote.delete_tag";
    check_remote_name(ID, &remote)?;
    check_tag_name(ID, &name)?;
    if confirm && expected.is_none() {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: msg!("engine-remote-tag-confirm-without-expected"),
        });
    }
    let expected = expected
        .map(|hex| {
            let full = hex.len() == Oid::HEX_LEN && hex.bytes().all(|b| b.is_ascii_hexdigit());
            full.then(|| hex.to_ascii_lowercase().parse::<Oid>().ok())
                .flatten()
                .ok_or_else(|| EngineError::InvalidParams {
                    id: ID,
                    reason: msg!(
                        "engine-remote-tag-expected-hex",
                        len = Oid::HEX_LEN,
                        value = hex.escape_debug()
                    ),
                })
        })
        .transpose()?;
    let job = s.job().clone();
    check_push_remote(s, &remote)?;
    let full = tag_ref(&name);
    let repo = s.repo_mut()?;
    let theirs = repo.remote_tag(&remote, &name, &job)?.ok_or_else(|| {
        EngineError::NotAllowed(msg!(
            "engine-remote-tag-unknown",
            remote = remote.escape_debug(),
            name = name.escape_debug()
        ))
    })?;
    // How the remote value relates to the local tag, for the confirmation.
    let local = || -> Message {
        let tag = repo.refs().ok().and_then(|refs| {
            refs.into_iter()
                .find(|r| r.name == full)
                .map(|r| r.tag_object.unwrap_or(r.target))
        });
        match tag {
            Some(value) if value == theirs => msg!("engine-remote-tag-local-same"),
            Some(value) => msg!("engine-remote-tag-local-other", value = value),
            None => msg!("engine-remote-tag-local-none"),
        }
    };
    // The confirmation holds only for the value seen.
    if let Some(expected) = expected
        && expected != theirs
    {
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: msg!(
                "engine-remote-tag-moved",
                remote = remote.as_str(),
                name = name.as_str(),
                expected = expected,
                now = theirs,
                local = local()
            ),
            expected: Some(theirs.to_string()),
        });
    }
    if !confirm {
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: msg!(
                "engine-remote-tag-confirm",
                remote = remote.as_str(),
                name = name.as_str(),
                theirs = theirs,
                local = local()
            ),
            expected: Some(theirs.to_string()),
        });
    }
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    let deleted = repo.delete_remote_tag(
        &RemoteTagDelete {
            remote: remote.clone(),
            name: name.clone(),
            expected: theirs,
        },
        &job,
    )?;
    let warnings: Vec<Message> = deleted
        .incomplete
        .then(|| {
            msg!(
                "engine-remote-tag-incomplete",
                remote = remote.as_str(),
                name = name.as_str(),
                backup = deleted.backup.as_str()
            )
        })
        .into_iter()
        .collect();
    let (refs, refs_error) = refs(s);
    Ok(to_json(RemoteDeleteTagResult {
        remote,
        tag: full,
        name,
        deleted: theirs,
        backup: deleted.backup,
        summary: deleted.summary,
        messages: deleted.messages,
        incomplete: deleted.incomplete,
        warnings,
        refs,
        refs_error,
    }))
}

#[cfg(test)]
mod tests {
    use crate::{
        EngineError, FetchFailure, MemoryBackend, MemoryOpener, PushRejection, Registry, Session,
    };
    use project_domain::Oid;
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn oid(n: u64) -> Oid {
        MemoryBackend::fake_oid(n)
    }

    /// The tag object of `v0.1.0` in the sample.
    fn sample_tag_object() -> Oid {
        oid(1_000)
    }

    fn opened_with(repo: MemoryBackend) -> (Registry, Session) {
        let mut opener = MemoryOpener::default();
        opener.insert("r", repo);
        let mut s = Session::new(Arc::new(opener));
        let r = Registry::builtin();
        r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
        (r, s)
    }

    fn refs(r: &Registry, s: &mut Session) -> Value {
        r.run("repo.refs", s, json!({})).unwrap()
    }

    /// The value the remote has for `name`, read through the confirmation of `remote.delete_tag`.
    fn on_remote(r: &Registry, s: &mut Session, name: &str) -> Option<String> {
        match r.run(
            "remote.delete_tag",
            s,
            json!({ "remote": "origin", "name": name }),
        ) {
            Err(EngineError::ConfirmRequired { expected, .. }) => expected,
            Err(EngineError::NotAllowed(_)) => None,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn push_tag_new_then_up_to_date_never_overwrites() {
        let mut repo = MemoryBackend::sample();
        repo.set_tag("light", oid(11), None);
        let (r, mut s) = opened_with(repo);
        let before = refs(&r, &mut s);
        let out = r
            .run(
                "remote.push_tag",
                &mut s,
                json!({ "remote": "origin", "name": "v0.1.0" }),
            )
            .unwrap();
        assert_eq!(out["kind"], "new");
        assert_eq!(out["tag"], "refs/tags/v0.1.0");
        assert_eq!(out["annotated"], true);
        assert_eq!(out["value"], sample_tag_object().to_string());
        assert_eq!(out["target"], oid(10).to_string());
        assert_eq!(
            on_remote(&r, &mut s, "v0.1.0"),
            Some(sample_tag_object().to_string())
        );
        let out = r
            .run(
                "remote.push_tag",
                &mut s,
                json!({ "remote": "origin", "name": "v0.1.0" }),
            )
            .unwrap();
        assert_eq!(out["kind"], "up_to_date");
        // A push does not touch the local refs nor the journal.
        assert_eq!(refs(&r, &mut s), before);
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(history["can_undo"], false);

        // Another value on the remote: `tag_exists`, the remote keeps its own.
        let mut repo = MemoryBackend::sample();
        repo.set_tag_on_remote("origin", "v0.1.0", oid(9), None);
        let (r, mut s) = opened_with(repo);
        let err = r
            .run(
                "remote.push_tag",
                &mut s,
                json!({ "remote": "origin", "name": "v0.1.0" }),
            )
            .unwrap_err();
        let EngineError::PushRejected { reason, branch, .. } = &err else {
            panic!("{err}");
        };
        assert_eq!(*reason, PushRejection::TagExists);
        assert_eq!(branch, "refs/tags/v0.1.0");
        assert!(err.to_string().contains("never overwritten"), "{err}");
        assert_eq!(on_remote(&r, &mut s, "v0.1.0"), Some(oid(9).to_string()));
    }

    #[test]
    fn push_tag_refusals() {
        let mut repo = MemoryBackend::sample();
        repo.add_remote("mirror", "https://example.com/m.git");
        repo.set_config("remote.mirror.mirror", &["true"]);
        let (r, mut s) = opened_with(repo);
        let cases = [
            json!({ "remote": "origin", "name": "nothing" }),
            json!({ "remote": "nowhere", "name": "v0.1.0" }),
            json!({ "remote": "mirror", "name": "v0.1.0" }),
        ];
        for params in cases {
            let err = r
                .run("remote.push_tag", &mut s, params.clone())
                .unwrap_err();
            assert!(matches!(err, EngineError::NotAllowed(_)), "{params}: {err}");
        }
        for params in [
            json!({ "remote": "origin", "name": "refs/tags/v0.1.0" }),
            json!({ "remote": "origin", "name": "a b" }),
            json!({ "remote": "origin", "name": "v0.1.0", "force": true }),
        ] {
            let err = r
                .run("remote.push_tag", &mut s, params.clone())
                .unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{params}: {err}"
            );
        }
        let mut repo = MemoryBackend::sample();
        repo.fail_fetch("origin", Some(FetchFailure::Auth("denied".into())));
        let (r, mut s) = opened_with(repo);
        let err = r
            .run(
                "remote.push_tag",
                &mut s,
                json!({ "remote": "origin", "name": "v0.1.0" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::Auth { .. }), "{err}");
    }

    #[test]
    fn delete_tag_on_the_remote_needs_the_value_seen() {
        let mut repo = MemoryBackend::sample();
        repo.set_tag_on_remote("origin", "v0.1.0", oid(10), Some(sample_tag_object()));
        repo.set_tag_on_remote("origin", "-x", oid(11), None);
        let (r, mut s) = opened_with(repo);
        let before = refs(&r, &mut s);
        let value = sample_tag_object().to_string();
        let err = r
            .run(
                "remote.delete_tag",
                &mut s,
                json!({ "remote": "origin", "name": "v0.1.0" }),
            )
            .unwrap_err();
        let EngineError::ConfirmRequired {
            reason, expected, ..
        } = &err
        else {
            panic!("{err}");
        };
        assert_eq!(expected.as_deref(), Some(value.as_str()));
        assert!(reason.contains("same value"), "{reason}");
        assert!(
            reason.contains(&format!("\"expected\": \"{value}\"")),
            "{reason}"
        );
        // `confirm` without `expected`, `expected` not full: invalid.
        for params in [
            json!({ "remote": "origin", "name": "v0.1.0", "confirm": true }),
            json!({ "remote": "origin", "name": "v0.1.0", "confirm": true, "expected": "abc" }),
        ] {
            let err = r.run("remote.delete_tag", &mut s, params).unwrap_err();
            assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
        }
        // Another value: asked again with the one read now.
        let err = r
            .run(
                "remote.delete_tag",
                &mut s,
                json!({ "remote": "origin", "name": "v0.1.0", "confirm": true, "expected": oid(10).to_string() }),
            )
            .unwrap_err();
        let EngineError::ConfirmRequired { expected, .. } = &err else {
            panic!("{err}");
        };
        assert_eq!(expected.as_deref(), Some(value.as_str()));
        assert_eq!(on_remote(&r, &mut s, "v0.1.0"), Some(value.clone()));

        let out = r
            .run(
                "remote.delete_tag",
                &mut s,
                json!({ "remote": "origin", "name": "v0.1.0", "confirm": true, "expected": value }),
            )
            .unwrap();
        assert_eq!(out["summary"], "[deleted]");
        assert_eq!(out["deleted"], sample_tag_object().to_string());
        assert_eq!(out["backup"], "refs/project/backup/push-delete-1");
        assert_eq!(out["incomplete"], false);
        assert_eq!(on_remote(&r, &mut s, "v0.1.0"), None);
        // The local tag stays; the journal does not change.
        assert_eq!(refs(&r, &mut s), before);
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(history["can_undo"], false);

        // A name starting with `-`; then gone: NotAllowed.
        let x = json!({ "remote": "origin", "name": "-x", "confirm": true, "expected": oid(11).to_string() });
        let out = r.run("remote.delete_tag", &mut s, x.clone()).unwrap();
        assert_eq!(out["tag"], "refs/tags/-x");
        let err = r.run("remote.delete_tag", &mut s, x).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    }

    #[test]
    fn delete_tag_on_the_remote_fetches_a_missing_object_into_the_backup() {
        let mut repo = MemoryBackend::sample();
        // A tag object only the remote has.
        let object = oid(5_000);
        repo.set_tag_on_remote("origin", "remote-only", oid(10), Some(object));
        let (r, mut s) = opened_with(repo);
        let err = r
            .run(
                "remote.delete_tag",
                &mut s,
                json!({ "remote": "origin", "name": "remote-only" }),
            )
            .unwrap_err();
        let EngineError::ConfirmRequired { reason, .. } = &err else {
            panic!("{err}");
        };
        assert!(reason.contains("no local tag"), "{reason}");
        let out = r
            .run(
                "remote.delete_tag",
                &mut s,
                json!({ "remote": "origin", "name": "remote-only", "confirm": true, "expected": object.to_string() }),
            )
            .unwrap();
        let backup = out["backup"].as_str().unwrap().to_owned();
        assert!(backup.starts_with("refs/project/backup/push-delete-"));
        assert_eq!(on_remote(&r, &mut s, "remote-only"), None);
        // The backup leads to the tagged commit.
        let log = r
            .run("repo.log", &mut s, json!({ "rev": backup, "limit": 1 }))
            .unwrap();
        assert_eq!(log["commits"][0]["id"], oid(10).to_string(), "{log}");
    }
}
