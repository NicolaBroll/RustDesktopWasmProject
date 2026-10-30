//! Fetch from the bar button (S25): `remote.fetch` in the background, with progress and
//! "Stop". Data only, no egui: tested with unit tests.
//!
//! One fetch at a time. While it runs, the session stays locked by the job (§6 of the git
//! architecture): `remote.fetch` is a write ([`CommandSpec::writes`](project_engine::CommandSpec)),
//! so the UI disables the other writes and "Refresh" until it is finished. Authentication and
//! network errors are explained in a window with git's text.

use crate::{Action, Done, Request, Tab, params};
use crate::{ErrorKind, Ticket};
use project_domain::RefInfo;
use project_domain::{REMOTES_PREFIX, TAGS_PREFIX};
use project_engine::{Progress, RefUpdate, RemoteFetchParams, RemoteFetchResult};
use project_i18n::tr;
use serde_json::{Value, json};

/// The id of the Registry command.
pub const COMMAND: &str = "remote.fetch";

/// The description of the fetch for the status bar.
pub fn label(remote: Option<&str>) -> String {
    match remote {
        Some(remote) => tr!("fetch-label-remote", remote = remote),
        None => tr!("fetch-label"),
    }
}

/// The fetch in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub ticket: Ticket,
    /// The requested remote; `None` = the one chosen by the engine (upstream, `origin`, the only one).
    pub remote: Option<String>,
    /// True after "Stop", until the command answers.
    pub cancelling: bool,
    /// Started by the timer of the automatic fetch (S50), not by the button.
    pub auto: bool,
}

/// A fetch that failed for a reason to explain: the window with git's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub title: String,
    /// What happened and what to do.
    pub explanation: String,
    /// git's text (stderr), if any.
    pub git: Option<String>,
}

/// The fetch: the one in progress and the last error to explain.
#[derive(Debug, Default)]
pub struct FetchBox {
    running: Option<Running>,
    failure: Option<Failure>,
}

/// What came back from `remote.fetch`.
#[derive(Debug)]
pub enum Received {
    /// Done: what to update in the view.
    Done(Box<Outcome>),
    /// Stopped with "Stop": the remote branches already updated stay, they must be reread.
    Cancelled { notice: String },
    /// Failed or refused: the message for the status bar (the details, if needed,
    /// are in [`FetchBox::failure`]); with `reload` refs and log are reread.
    Failed { message: String, reload: bool },
}

/// The result of a successful fetch, for the view.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub notice: String,
    /// The new refs, if the command reread them.
    pub refs: Option<Vec<RefInfo>>,
    /// True if something changed (or it is unknown): log and status are reread.
    pub changed: bool,
    /// A problem after the successful fetch (refs not reread).
    pub warning: Option<String>,
    /// The remote branches (`refs/remotes/`) the fetch moved, created or removed.
    pub moved: Vec<RefUpdate>,
}

impl FetchBox {
    /// The fetch in progress, if any.
    pub fn running(&self) -> Option<&Running> {
        self.running.as_ref()
    }

    /// The error to explain in the window, if any.
    pub fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }

    /// Closes the error window.
    pub fn dismiss(&mut self) {
        self.failure = None;
    }

    /// The parameters for a fetch from `remote` (`None`: the one chosen by the engine), if there is
    /// not one in progress already. With `prune`: the branches deleted on the remote also disappear
    /// from the view (the engine saves a backup of commits that only they had).
    pub fn request(&self, remote: Option<&str>) -> Option<Value> {
        if self.running.is_some() {
            return None;
        }
        Some(params(RemoteFetchParams {
            remote: remote.map(str::to_owned),
            branch: None,
            prune: true,
        }))
    }

    /// The fetch requested with [`FetchBox::request`] started with `ticket`.
    pub fn started(&mut self, ticket: Ticket, remote: Option<String>) {
        self.failure = None;
        self.running = Some(Running {
            ticket,
            remote,
            cancelling: false,
            auto: false,
        });
    }

    /// The automatic fetch (S50) started with `ticket`, from the remote the engine chooses. It
    /// opens no window: [`FetchBox::receive`] leaves none for it.
    pub fn started_auto(&mut self, ticket: Ticket) {
        self.running = Some(Running {
            ticket,
            remote: None,
            cancelling: false,
            auto: true,
        });
    }

    /// "Stop": the ticket to cancel, only once.
    pub fn cancel(&mut self) -> Option<Ticket> {
        let running = self.running.as_mut().filter(|r| !r.cancelling)?;
        running.cancelling = true;
        Some(running.ticket)
    }

    /// The response of `remote.fetch` for `ticket`.
    pub fn receive(
        &mut self,
        ticket: Ticket,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let (remote, auto) = match self.running.take() {
            Some(running) if running.ticket == ticket => (running.remote, running.auto),
            other => {
                self.running = other;
                return Received::Failed {
                    message: tr!("common-unexpected-response", command = COMMAND),
                    reload: true,
                };
            }
        };
        let who = who(remote.as_deref());
        let label = label(remote.as_deref());
        let error = match result {
            Ok(value) => {
                return match serde_json::from_value::<RemoteFetchResult>(value) {
                    Ok(result) => Received::Done(Box::new(outcome(result))),
                    Err(e) => Received::Failed {
                        message: crate::unexpected_result(COMMAND, e),
                        reload: true,
                    },
                };
            }
            Err(error) => error,
        };
        let received = match kind {
            Some(ErrorKind::Cancelled) => Received::Cancelled {
                notice: tr!("fetch-cancelled", label = label),
            },
            Some(ErrorKind::Auth) => {
                self.failure = Some(auth_failure(&who, detail));
                Received::Failed {
                    message: tr!("fetch-failed-auth", label = label),
                    reload: false,
                }
            }
            Some(ErrorKind::RemoteUnreachable) => {
                self.failure = Some(unreachable_failure(&who, detail));
                Received::Failed {
                    message: tr!("fetch-failed-unreachable", label = label),
                    reload: false,
                }
            }
            // Refused before starting (unknown remote, no remote): nothing has
            // changed.
            Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams) => Received::Failed {
                message: tr!("fetch-not-possible", label = label, error = error),
                reload: false,
            },
            _ => {
                self.failure = Some(Failure {
                    title: tr!("fetch-failed-title"),
                    explanation: error.clone(),
                    git: detail,
                });
                // Git may have updated some remote branch before failing.
                Received::Failed {
                    message: tr!("common-failed", label = label, error = error),
                    reload: true,
                }
            }
        };
        // The automatic fetch explains its errors without a window (see `auto_fetch`).
        if auto {
            self.failure = None;
        }
        received
    }
}

/// The subject of the error windows: "The remote “origin”", or "The remote" without a name.
pub fn who(remote: Option<&str>) -> String {
    match remote {
        Some(remote) => tr!("fetch-who-remote", remote = remote),
        None => tr!("fetch-who"),
    }
}

/// The sentence that closes the explanations of [`auth_failure`] and [`unreachable_failure`].
pub fn nothing_changed() -> String {
    tr!("fetch-nothing-changed")
}

/// The window for a remote (`who`: "The remote “origin”") that rejects the credentials or
/// would need to ask for them; `detail` is git's text. Used for fetch, pull and push.
pub fn auth_failure(who: &str, detail: Option<String>) -> Failure {
    Failure {
        title: tr!("fetch-auth-title"),
        explanation: format!(
            "{} {}",
            tr!("fetch-auth-explanation", who = who),
            nothing_changed()
        ),
        git: detail,
    }
}

/// The window for a remote that cannot be reached (see [`auth_failure`]).
pub fn unreachable_failure(who: &str, detail: Option<String>) -> Failure {
    Failure {
        title: tr!("fetch-unreachable-title"),
        explanation: format!(
            "{} {}",
            tr!("fetch-unreachable-explanation", who = who),
            nothing_changed()
        ),
        git: detail,
    }
}

/// How many ref names to list in the notice before "and N more".
const NAMES_SHOWN: usize = 3;

/// The name of a ref for the notice: `origin/main`, `tag v1`.
fn ref_text(name: &str) -> String {
    if let Some(remote) = name.strip_prefix(REMOTES_PREFIX) {
        remote.to_owned()
    } else if let Some(tag) = name.strip_prefix(TAGS_PREFIX) {
        tr!("fetch-ref-tag", tag = tag)
    } else {
        name.to_owned()
    }
}

/// The notice for the status bar (also for the fetch of `remote.add`).
pub(crate) fn outcome(r: RemoteFetchResult) -> Outcome {
    // git's lines (`From ...`, `* [new branch] ...`) go only in the log.
    for line in &r.messages {
        log::info!("{COMMAND}: {line}");
    }
    let what = match &r.branch {
        Some(branch) => tr!("fetch-what-branch", branch = branch, remote = &r.remote),
        None => tr!("fetch-label-remote", remote = &r.remote),
    };
    let summary = match &r.updated {
        None => tr!("fetch-done", what = what),
        Some(updated) if updated.is_empty() => tr!("fetch-nothing-new", what = what),
        Some(updated) => {
            let mut names: Vec<String> = updated
                .iter()
                .take(NAMES_SHOWN)
                .map(|u| {
                    let name = ref_text(&u.name);
                    match (u.before, u.after) {
                        (None, Some(_)) => tr!("fetch-ref-new", name = name),
                        (Some(_), None) => tr!("fetch-ref-removed", name = name),
                        _ => name,
                    }
                })
                .collect();
            if updated.len() > NAMES_SHOWN {
                names.push(tr!("fetch-and-more", n = updated.len() - NAMES_SHOWN));
            }
            tr!(
                "fetch-updated",
                what = what,
                n = updated.len(),
                names = names.join(", ")
            )
        }
    };
    let warnings = crate::messages_text(&r.warnings);
    let backups = if r.backups.is_empty() {
        String::new()
    } else {
        tr!("fetch-pruned-backups", n = r.backups.len())
    };
    Outcome {
        notice: summary + &backups + &warnings,
        changed: r.updated.as_ref().is_none_or(|u| !u.is_empty()),
        moved: r
            .updated
            .iter()
            .flatten()
            .filter(|u| u.name.starts_with(REMOTES_PREFIX))
            .cloned()
            .collect(),
        refs: r.refs,
        warning: r
            .refs_error
            .map(|e| tr!("fetch-refs-not-reread", error = e)),
    }
}

/// The translated name of a git phase, if it is one of the known ones.
fn phase_name(phase: &str) -> String {
    match phase {
        "Enumerating objects" => tr!("fetch-phase-enumerating"),
        "Counting objects" => tr!("fetch-phase-counting"),
        "Compressing objects" => tr!("fetch-phase-compressing"),
        "Receiving objects" => tr!("fetch-phase-receiving"),
        "Unpacking objects" => tr!("fetch-phase-unpacking"),
        "Resolving deltas" => tr!("fetch-phase-resolving"),
        "Writing objects" => tr!("fetch-phase-writing"),
        "Total" => tr!("fetch-phase-total"),
        // `git commit-graph write` (repo.optimize): "... in <k> passes".
        "Expanding reachable commits in commit graph" => tr!("fetch-phase-expanding-graph"),
        other if other.starts_with("Writing out commit graph") => {
            tr!("fetch-phase-writing-graph")
        }
        other => other.to_owned(),
    }
}

/// How to show a progress step: the text (phase and count) and the fraction for the
/// bar, if git gives the percentage.
pub fn describe(progress: &Progress) -> (String, Option<f32>) {
    let phase = phase_name(&progress.phase);
    let mut text = if progress.remote {
        tr!("fetch-progress-remote", phase = phase)
    } else {
        phase
    };
    match (progress.current, progress.total) {
        (Some(current), Some(total)) => text.push_str(&format!(" {current}/{total}")),
        (Some(current), None) => text.push_str(&format!(" {current}")),
        _ => {}
    }
    let fraction = progress.percent.map(|p| f32::from(p.min(100)) / 100.0);
    (text, fraction)
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_fetch(&mut self, action: Action) {
        match action {
            Action::Fetch(remote) => {
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                repo.write_error = None;
                self.start_fetch(remote, false);
            }
            Action::CancelFetch => {
                if let Some(ticket) = self.repo.as_mut().and_then(|r| r.fetch.cancel()) {
                    self.executor.cancel(ticket);
                }
            }
            Action::DismissFetchError => {
                if let Some(repo) = &mut self.repo {
                    repo.fetch.dismiss();
                }
            }
            _ => unreachable!("action dispatched to the wrong function"),
        }
    }

    /// Sends `remote.fetch` (the parameters of [`FetchBox::request`]) unless one is already in
    /// progress; `auto`: from the timer of the automatic fetch (S50).
    pub(crate) fn start_fetch(&mut self, remote: Option<String>, auto: bool) {
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        let Some(params) = repo.fetch.request(remote.as_deref()) else {
            return;
        };
        repo.write_label = Some(if auto {
            tr!("fetch-auto-label")
        } else {
            label(remote.as_deref())
        });
        let ticket = self.executor.submit(COMMAND, params);
        self.pending.insert(ticket, Request::Fetch(generation));
        if let Some(repo) = &mut self.repo {
            repo.auto_fetch.started();
            if auto {
                repo.fetch.started_auto(ticket);
            } else {
                repo.fetch.started(ticket, remote);
            }
        }
    }

    /// Response of `remote.fetch`: the refs from the response (the column shows the new remote
    /// branches and how far behind the local branches are), then log and status reread if something
    /// changed; the notice with the updated refs, or the error.
    pub(crate) fn receive_fetch(&mut self, generation: u64, done: Done) {
        let auto = self
            .current(generation)
            .and_then(|r| r.fetch.running())
            .is_some_and(|r| r.auto);
        if auto {
            self.receive_auto_fetch(generation, done);
            return;
        }
        // How many remote branches each remote has: a fetch brings new ones or removes some.
        if self.current(generation).is_some() {
            self.request_remotes(generation);
        }
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        let received =
            repo.fetch
                .receive(done.ticket, done.result, done.error_kind, done.error_detail);
        match received {
            Received::Done(outcome) => {
                log::info!("{}: {}", done.command, outcome.notice);
                let refs_read = outcome.refs.is_some();
                // The remote answers: the automatic fetch starts again if it had stopped.
                repo.auto_fetch.succeeded();
                if let Some(refs) = outcome.refs {
                    repo.refs = refs;
                    repo.update_labels();
                }
                self.notice = outcome.notice;
                self.error = outcome.warning.unwrap_or_default();
                if !refs_read {
                    self.submit("repo.refs", json!({}), Request::Refs(generation));
                }
                if outcome.changed {
                    self.reload_after_fetch(generation);
                }
            }
            Received::Cancelled { notice } => {
                self.notice = notice;
                self.error.clear();
                // Some remote branch may already be updated.
                self.submit("repo.refs", json!({}), Request::Refs(generation));
                self.reload_after_fetch(generation);
            }
            Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
                if reload {
                    self.submit("repo.refs", json!({}), Request::Refs(generation));
                    self.reload_after_fetch(generation);
                }
            }
        }
    }

    /// After a fetch: the status (ahead/behind the upstream) and the log (the new
    /// commits of the remote branches).
    pub(crate) fn reload_after_fetch(&mut self, generation: u64) {
        self.submit("repo.status", json!({}), Request::Status(generation));
        if let Some(repo) = self.current(generation) {
            let first = repo.list.reset();
            self.request_page(first);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Executor, Wake};
    use project_engine::{
        Capabilities, FetchFailure, MemoryBackend, MemoryOpener, Platform, Registry, Session,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The job runs inside `spawn`, on the same thread.
    struct Inline;

    impl Platform for Inline {
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
            on_done(None);
        }
        fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
            job();
        }
    }

    #[test]
    fn progress_is_described_for_the_bar() {
        let p = Progress {
            phase: "Receiving objects".into(),
            remote: false,
            percent: Some(45),
            current: Some(9),
            total: Some(20),
            done: false,
            text: "Receiving objects:  45% (9/20)".into(),
        };
        assert_eq!(
            describe(&p),
            ("Receiving objects 9/20".to_owned(), Some(0.45))
        );
        let remote = Progress {
            phase: "Enumerating objects".into(),
            remote: true,
            percent: None,
            current: Some(5),
            total: None,
            done: true,
            text: "remote: Enumerating objects: 5, done.".into(),
        };
        assert_eq!(
            describe(&remote),
            ("remote: Enumerating objects 5".to_owned(), None)
        );
    }

    #[test]
    fn one_fetch_at_a_time_and_cancel_once() {
        let mut f = FetchBox::default();
        assert_eq!(f.request(None), Some(json!({ "prune": true })));
        assert_eq!(
            f.request(Some("origin")),
            Some(json!({ "remote": "origin", "prune": true }))
        );
        assert_eq!(f.cancel(), None, "nothing to stop");
        let ticket = Ticket::for_test(4);
        f.started(ticket, Some("origin".to_owned()));
        assert_eq!(f.request(None), None);
        assert_eq!(f.cancel(), Some(ticket));
        assert!(f.running().unwrap().cancelling);
        assert_eq!(f.cancel(), None);
        // A response for another ticket does not close the one in progress.
        let other = f.receive(Ticket::for_test(9), Ok(json!({})), None, None);
        assert!(matches!(other, Received::Failed { reload: true, .. }));
        assert!(f.running().is_some());
        let r = f.receive(
            ticket,
            Err("cancelled".into()),
            Some(ErrorKind::Cancelled),
            None,
        );
        let Received::Cancelled { notice } = r else {
            panic!("{r:?}")
        };
        assert!(
            notice.starts_with("Fetch from “origin” cancelled"),
            "{notice}"
        );
        assert!(f.running().is_none());
        assert!(f.failure().is_none());
    }

    #[test]
    fn auth_and_network_errors_are_explained() {
        let mut f = FetchBox::default();
        let ticket = Ticket::for_test(1);
        f.started(ticket, None);
        let r = f.receive(
            ticket,
            Err("authentication...".into()),
            Some(ErrorKind::Auth),
            Some("fatal: Authentication failed for 'https://x'".into()),
        );
        let Received::Failed { message, reload } = r else {
            panic!("{r:?}")
        };
        assert!(!reload);
        assert!(message.contains("authentication"), "{message}");
        let failure = f.failure().unwrap();
        assert_eq!(failure.title, "Authentication failed");
        assert!(failure.explanation.contains("never asks for credentials"));
        assert!(failure.explanation.contains("credential helper"));
        assert!(failure.explanation.contains("ssh-agent"));
        assert_eq!(
            failure.git.as_deref(),
            Some("fatal: Authentication failed for 'https://x'")
        );
        f.dismiss();
        assert!(f.failure().is_none());

        f.started(ticket, Some("origin".to_owned()));
        f.receive(
            ticket,
            Err("x".into()),
            Some(ErrorKind::RemoteUnreachable),
            Some("fatal: Could not resolve host".into()),
        );
        let failure = f.failure().unwrap();
        assert_eq!(failure.title, "Remote unreachable");
        assert!(failure.explanation.starts_with("The remote “origin”"));

        // A new fetch removes the old window; a refusal goes only in the bar.
        f.started(ticket, Some("other".to_owned()));
        assert!(f.failure().is_none());
        let r = f.receive(
            ticket,
            Err("there is no remote `other`".into()),
            Some(ErrorKind::NotAllowed),
            None,
        );
        let Received::Failed { message, reload } = r else {
            panic!("{r:?}")
        };
        assert_eq!(
            message,
            "Fetch from “other” not possible: there is no remote `other`"
        );
        assert!(!reload);
        assert!(f.failure().is_none());
    }

    fn fetch_on(repo: MemoryBackend, ticket: u64) -> (Registry, Session, Received, FetchBox) {
        let mut opener = MemoryOpener::new();
        opener.insert("r", repo);
        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(opener));
        registry
            .run("repo.open", &mut session, json!({ "path": "r" }))
            .unwrap();
        let mut f = FetchBox::default();
        let params = f.request(None).unwrap();
        let ticket = Ticket::for_test(ticket);
        f.started(ticket, None);
        let result = registry.run(COMMAND, &mut session, params);
        let kind = result.as_ref().err().map(ErrorKind::of);
        let detail = result.as_ref().err().and_then(ErrorKind::detail);
        let received = f.receive(ticket, result.map_err(|e| e.text()), kind, detail);
        (registry, session, received, f)
    }

    #[test]
    fn fetch_with_the_sample_repo_updates_the_remote_branch() {
        let (registry, mut session, r, _) = fetch_on(MemoryBackend::sample(), 0);
        let Received::Done(outcome) = r else {
            panic!("{r:?}")
        };
        assert_eq!(
            outcome.notice,
            "Fetch from “origin”: 1 ref updated (origin/feature/graph)"
        );
        assert!(outcome.changed);
        assert_eq!(outcome.warning, None);
        let refs = outcome.refs.unwrap();
        let graph_tip = refs
            .iter()
            .find(|r| r.name == "refs/heads/feature/graph")
            .unwrap();
        // The local branch stays where it was and is one behind the remote.
        assert_eq!(graph_tip.upstream.as_ref().unwrap().behind, Some(1));

        // The second fetch has nothing new.
        let mut f = FetchBox::default();
        let ticket = Ticket::for_test(1);
        let params = f.request(None).unwrap();
        f.started(ticket, None);
        let result = registry.run(COMMAND, &mut session, params);
        let Received::Done(again) = f.receive(ticket, result.map_err(|e| e.text()), None, None)
        else {
            panic!()
        };
        assert_eq!(again.notice, "Fetch from “origin”: nothing new");
        assert!(!again.changed);
    }

    #[test]
    fn auth_failure_from_the_backend() {
        let mut repo = MemoryBackend::sample();
        repo.fail_fetch(
            "origin",
            Some(FetchFailure::Auth(
                "fatal: could not read Username for 'https://example.com'".into(),
            )),
        );
        let (_, _, r, f) = fetch_on(repo, 0);
        assert!(matches!(r, Received::Failed { reload: false, .. }), "{r:?}");
        let failure = f.failure().unwrap();
        assert_eq!(failure.title, "Authentication failed");
        assert_eq!(
            failure.git.as_deref(),
            Some("fatal: could not read Username for 'https://example.com'")
        );
    }

    #[test]
    fn executor_reports_progress_on_the_same_thread() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        let wake: Wake = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let session = Session::new(Arc::new(MemoryOpener::sample()));
        let mut ex = Executor::new(session, Registry::builtin(), Arc::new(Inline), wake);
        ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        assert!(ex.poll()[0].result.is_ok());
        let before = wakes.load(Ordering::SeqCst);
        // With `Inline` the fetch ends inside `submit`: progress arrives before the result,
        // and each one requests a frame.
        let ticket = ex.submit(COMMAND, json!({}));
        assert!(
            wakes.load(Ordering::SeqCst) > before + 1,
            "progress without a frame"
        );
        let last = ex.progress(ticket).expect("last progress");
        assert!(last.done, "{last:?}");
        let done = ex.poll().remove(0);
        assert!(done.result.is_ok(), "{:?}", done.result);
        assert_eq!(ex.progress(ticket), None, "collected: no progress");
        assert!(!ex.cancel(ticket));
    }
}
