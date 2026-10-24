//! [`Platform::watch`](project_engine::Platform::watch) on desktop: the `notify` crate on the git
//! directory of the repo (never the worktree: on a huge repo a recursive watch would exhaust the
//! inotify watches), with a debounce.

use notify::event::{AccessKind, AccessMode};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use project_engine::{OnChange, Watch};
use project_git::CliOpener;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Quiet time after the last event before `on_change` is called: a commit touches the index, the
/// ref and the reflog in a few milliseconds.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Longest wait while the events keep arriving (a long rebase): the view updates at least this often.
const MAX_WAIT: Duration = Duration::from_secs(2);

/// Holds the watcher once it is ready; dropped, the watcher stops and its thread ends.
type Slot = Arc<Mutex<Option<notify::RecommendedWatcher>>>;

/// Starts the watch of `repo` on a thread of its own: finding the git directories runs git.
pub fn start(repo: &str, on_change: OnChange) -> Option<Watch> {
    let slot: Slot = Arc::default();
    let weak = Arc::downgrade(&slot);
    let repo = repo.to_owned();
    let spawned = std::thread::Builder::new()
        .name("project-watch".to_owned())
        .spawn(move || {
            let dirs = match git_dirs(&repo) {
                Ok(dirs) => dirs,
                Err(e) => {
                    log::warn!("{repo}: not watched: {e}");
                    return;
                }
            };
            let (sender, events) = channel();
            let mut watcher = match notify::recommended_watcher(sender) {
                Ok(w) => w,
                Err(e) => {
                    log::warn!("{repo}: not watched: {e}");
                    return;
                }
            };
            for (dir, mode) in &dirs {
                if let Err(e) = watcher.watch(dir, *mode) {
                    log::warn!("{}: not watched: {e}", dir.display());
                }
            }
            // The tab may have closed in the meantime.
            match weak.upgrade() {
                Some(slot) => *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(watcher),
                None => return,
            }
            log::debug!("{repo}: watching {dirs:?}");
            // Ends when the watcher (and with it the sender) is dropped.
            while let Ok(event) = events.recv() {
                if !relevant(&event) {
                    continue;
                }
                let start = Instant::now();
                let mut deadline = start + DEBOUNCE;
                loop {
                    let wait = deadline.saturating_duration_since(Instant::now());
                    match events.recv_timeout(wait) {
                        Ok(event) if relevant(&event) => {
                            deadline = (Instant::now() + DEBOUNCE).min(start + MAX_WAIT);
                        }
                        Ok(_) => {}
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                on_change();
            }
        });
    if let Err(e) = spawned {
        log::error!("cannot start a thread: {e}");
        return None;
    }
    Some(Box::new(slot))
}

/// What to watch: the git directory of the worktree (HEAD, index, `MERGE_HEAD`, `rebase-merge/`…
/// appear as entries in it) and its `refs/`; for a linked worktree also the common directory
/// (`packed-refs`, the branches).
fn git_dirs(repo: &str) -> Result<Vec<(PathBuf, RecursiveMode)>, project_engine::EngineError> {
    let git_dir = CliOpener::new().open_cli(repo)?.git_dir().to_path_buf();
    let mut dirs = vec![(git_dir.clone(), RecursiveMode::NonRecursive)];
    // `commondir` is git's own pointer from `.git/worktrees/<name>` to the main git directory.
    if let Ok(common) = std::fs::read_to_string(git_dir.join("commondir")) {
        let common = git_dir.join(common.trim_end_matches(['\r', '\n']));
        if let Ok(common) = common.canonicalize()
            && common != git_dir
        {
            dirs.push((common, RecursiveMode::NonRecursive));
        }
    }
    let refs: Vec<_> = dirs
        .iter()
        .map(|(dir, _)| dir.join("refs"))
        .filter(|refs| refs.is_dir())
        .collect();
    dirs.extend(refs.into_iter().map(|r| (r, RecursiveMode::Recursive)));
    Ok(dirs)
}

/// True if the event can change what the app shows. Not: opening or reading a file (the app's
/// own reads would refresh it in a loop), lock files (git writes `index.lock`, then renames it to
/// `index`: the rename counts), objects and reflogs (a ref or HEAD always changes with them).
fn relevant(event: &notify::Result<Event>) -> bool {
    let event = match event {
        Ok(event) => event,
        // Lost events (queue overflow): better one refresh too many.
        Err(e) => {
            log::warn!("watch: {e}");
            return true;
        }
    };
    match event.kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => {}
        EventKind::Access(_) => return false,
        _ => {}
    }
    event.paths.iter().any(|path| relevant_path(path))
}

fn relevant_path(path: &Path) -> bool {
    if path.extension().is_some_and(|e| e == "lock") {
        return false;
    }
    !path
        .components()
        .any(|c| c.as_os_str() == "objects" || c.as_os_str() == "logs")
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind};

    fn event(kind: EventKind, path: &str) -> notify::Result<Event> {
        Ok(Event::new(kind).add_path(PathBuf::from(path)))
    }

    #[test]
    fn reads_locks_objects_and_reflogs_are_not_changes() {
        let open = EventKind::Access(AccessKind::Open(AccessMode::Any));
        let closed = EventKind::Access(AccessKind::Close(AccessMode::Write));
        let modify = EventKind::Modify(ModifyKind::Any);
        let create = EventKind::Create(CreateKind::File);
        assert!(!relevant(&event(open, "/r/.git/HEAD")));
        assert!(relevant(&event(closed, "/r/.git/HEAD")));
        assert!(!relevant(&event(create, "/r/.git/index.lock")));
        assert!(relevant(&event(create, "/r/.git/index")));
        assert!(relevant(&event(modify, "/r/.git/refs/heads/main")));
        assert!(!relevant(&event(modify, "/r/.git/objects/ab/cd")));
        assert!(!relevant(&event(modify, "/r/.git/logs/HEAD")));
        assert!(relevant(&event(create, "/r/.git/MERGE_HEAD")));
    }

    #[test]
    fn a_commit_from_outside_calls_back_once() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(ok, "{args:?}");
        };
        git(&["init", "-q"]);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let watch = start(
            dir.path().to_str().unwrap(),
            Arc::new(move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
        );
        assert!(watch.is_some());
        let count = || calls.load(std::sync::atomic::Ordering::SeqCst);
        // The watcher starts on its own thread: commit again until one is seen.
        for _ in 0..20 {
            git(&["commit", "-q", "--allow-empty", "-m", "outside"]);
            std::thread::sleep(DEBOUNCE * 2);
            if count() > 0 {
                break;
            }
        }
        assert!(count() >= 1);
        // Reads do not count.
        std::thread::sleep(DEBOUNCE * 2);
        let before = count();
        git(&["--no-optional-locks", "status", "--porcelain=v2"]);
        git(&["log", "--oneline", "-n1"]);
        std::thread::sleep(DEBOUNCE * 3);
        assert_eq!(count(), before);
        drop(watch);
    }
}
