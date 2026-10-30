//! Reset from the UI on the `MemoryBackend` sample: previews per mode, `confirm` and `expected`
//! only from the plan shown, refusals that stay in the dialog, detached HEAD.
//!
//! The sample: `main` at `c12`, `origin/main` at `c11`; `README.md` and `src/lib.rs` with local
//! changes, `note.txt` untracked.

use super::*;
use crate::merge::Received;
use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
use serde_json::json;
use std::sync::Arc;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn opened_with(repo: MemoryBackend) -> (Registry, Session) {
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(opener));
    registry
        .run("repo.open", &mut session, json!({ "path": "r" }))
        .unwrap();
    (registry, session)
}

fn split(
    result: Result<Value, project_engine::EngineError>,
) -> (Result<Value, String>, Option<ErrorKind>, Option<String>) {
    let (kind, detail) = match &result {
        Err(e) => (Some(ErrorKind::of(e)), ErrorKind::detail(e)),
        Ok(_) => (None, None),
    };
    (result.map_err(|e| e.text()), kind, detail)
}

/// Runs the preview like the executor and passes it to the box.
fn preview(r: &Registry, s: &mut Session, b: &mut ResetBox, (params, seq): (Value, u64)) {
    let (result, _, _) = split(r.run(PREVIEW, s, params));
    b.receive_preview(seq, result);
}

/// Runs `branch.reset` like the executor.
fn reset(
    r: &Registry,
    s: &mut Session,
    b: &mut ResetBox,
    params: Value,
) -> (Received, Option<(Value, u64)>) {
    let (result, kind, detail) = split(r.run(COMMAND, s, params));
    b.receive(result, kind, detail)
}

fn ready(b: &ResetBox) -> &ResetPreviewResult {
    match &b.dialog().unwrap().phase {
        Phase::Ready(preview) => preview,
        other => panic!("not ready: {other:?}"),
    }
}

#[test]
fn every_mode_reads_its_plan_and_old_answers_are_ignored() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let mut b = ResetBox::default();
    let first = b.open(oid(11), "c11".into(), Some("main".into())).unwrap();
    assert_eq!(
        first.0,
        json!({ "target": oid(11).to_string(), "mode": "mixed" })
    );
    assert!(
        b.open(oid(10), "c10".into(), None).is_none(),
        "one at a time"
    );
    // The mode changes before the first answer arrives: that answer is ignored.
    let hard = b.set_mode(Mode::Hard).unwrap();
    assert!(
        b.set_mode(Mode::Hard).is_none(),
        "same mode, no new preview"
    );
    preview(&r, &mut s, &mut b, first);
    assert_eq!(b.dialog().unwrap().phase, Phase::Loading);
    assert!(!b.dialog().unwrap().can_submit());
    preview(&r, &mut s, &mut b, hard);
    let plan = ready(&b);
    assert!(plan.needs_confirm);
    let files: Vec<String> = plan.files.iter().map(|p| p.to_string()).collect();
    assert!(files.contains(&"README.md".to_owned()), "{files:?}");
    assert!(files.contains(&"src/lib.rs".to_owned()), "{files:?}");
    assert!(
        !files.contains(&"note.txt".to_owned()),
        "untracked not listed"
    );
    // Back to soft: nothing overwritten, no confirmation.
    let soft = b.set_mode(Mode::Soft).unwrap();
    preview(&r, &mut s, &mut b, soft);
    assert!(ready(&b).files.is_empty());
    assert!(!ready(&b).needs_confirm);
    // Cancel closes without writing.
    b.close();
    assert!(b.dialog().is_none());
}

#[test]
fn hard_sends_confirm_and_expected_and_reports_the_backup() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let mut b = ResetBox::default();
    b.open(oid(11), "c11".into(), Some("main".into())).unwrap();
    let hard = b.set_mode(Mode::Hard).unwrap();
    preview(&r, &mut s, &mut b, hard);
    let (params, label) = b.submit().unwrap();
    assert_eq!(params["mode"], "hard");
    assert_eq!(params["confirm"], true);
    assert_eq!(params["expected"], oid(12).to_string());
    assert!(
        label.contains("“main”") && label.contains("hard"),
        "{label}"
    );
    // While it runs, the mode cannot change and the dialog does not close.
    assert!(b.set_mode(Mode::Soft).is_none());
    b.close();
    assert!(b.dialog().is_some());
    let (received, reload) = reset(&r, &mut s, &mut b, params);
    assert!(reload.is_none());
    let Received::Done(outcome) = received else {
        panic!("not done: {received:?}");
    };
    assert!(outcome.moved);
    assert!(outcome.history.can_undo);
    assert!(
        outcome.notice.contains("refs/project/backup/reset-"),
        "{}",
        outcome.notice
    );
    assert!(b.dialog().is_none());
}

#[test]
fn mixed_without_confirm_and_the_published_warning() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let mut b = ResetBox::default();
    let mixed = b.open(oid(10), "c10".into(), Some("main".into())).unwrap();
    preview(&r, &mut s, &mut b, mixed);
    let plan = ready(&b);
    // `c11` leaves `main` and is already on `origin/main`.
    assert_eq!(plan.published, 1);
    assert_eq!(plan.upstream.as_deref(), Some("refs/remotes/origin/main"));
    assert_eq!(plan.warnings.len(), 1);
    assert!(
        plan.warnings[0].text().contains("lease"),
        "{:?}",
        plan.warnings
    );
    let (params, _) = b.submit().unwrap();
    assert_eq!(params.get("confirm"), None);
    let (received, _) = reset(&r, &mut s, &mut b, params);
    let Received::Done(outcome) = received else {
        panic!("not done: {received:?}");
    };
    assert!(outcome.warning.is_none());
    assert!(outcome.notice.contains("lease"), "{}", outcome.notice);
}

#[test]
fn head_moved_after_the_preview_is_refused_and_the_plan_reread() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let mut b = ResetBox::default();
    let soft = b.open(oid(10), "c10".into(), Some("main".into())).unwrap();
    preview(&r, &mut s, &mut b, soft);
    let (params, _) = b.submit().unwrap();
    // Another process moves `main` in the meantime.
    r.run(
        COMMAND,
        &mut s,
        json!({ "target": oid(11).to_string(), "mode": "soft" }),
    )
    .unwrap();
    let (received, reload) = reset(&r, &mut s, &mut b, params);
    assert!(matches!(received, Received::Inline), "{received:?}");
    let dialog = b.dialog().unwrap();
    assert!(dialog.error.as_deref().unwrap().contains("nothing changed"));
    assert_eq!(dialog.phase, Phase::Loading);
    preview(&r, &mut s, &mut b, reload.unwrap());
    assert_eq!(ready(&b).expected, oid(11));
    // The error stays visible over the new plan.
    assert!(b.dialog().unwrap().error.is_some());
}

#[test]
fn refusals_of_the_preview_disable_reset_and_detached_head_has_no_branch() {
    let mut repo = MemoryBackend::sample();
    repo.set_entries(Vec::new());
    repo.detach_head(oid(12));
    let (r, mut s) = opened_with(repo);
    let mut b = ResetBox::default();
    let mixed = b.open(oid(10), "c10".into(), None).unwrap();
    assert_eq!(b.dialog().unwrap().moving(), "HEAD (detached)");
    preview(&r, &mut s, &mut b, mixed);
    assert!(b.dialog().unwrap().can_submit());
    b.close();
    // A target that does not exist: the reason, no "Reset".
    let bad = b.open(oid(99), "c99".into(), None).unwrap();
    preview(&r, &mut s, &mut b, bad);
    let dialog = b.dialog().unwrap();
    assert!(
        matches!(dialog.phase, Phase::Refused(_)),
        "{:?}",
        dialog.phase
    );
    assert!(!dialog.can_submit());
    assert!(b.submit().is_none());
}
