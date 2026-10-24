//! Several repos open in tabs, the recent repos and their persistence between launches.
//!
//! Each [`Tab`] has its own executor and `Session`: undo/redo journals stay per repo and the
//! results of a tab not shown keep landing in it. What is saved (eframe storage): the paths of the
//! open tabs, the one shown and the recent repos; at startup the tabs reopen.

use crate::executor::{Executor, Wake};
use crate::{Action, Tab, view};
use project_engine::{Platform, Registry, RepoOpener, Session};
use project_i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Key of the state in the eframe storage.
const KEY: &str = "tabs";

/// How many recent repos are remembered.
const MAX_RECENT: usize = 10;

/// Seconds between two checks of which recent folders still exist.
const CHECK_EVERY: f64 = 1.0;

/// What survives between one launch and the next.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    /// Paths of the open tabs, in order.
    tabs: Vec<String>,
    /// Index in `tabs` of the tab shown.
    active: usize,
    /// Most recent first.
    recent: Vec<String>,
}

pub struct App {
    tabs: Vec<Tab>,
    /// Index in `tabs` of the tab shown; there is always at least one tab.
    active: usize,
    /// Repos opened successfully, most recent first.
    recent: Vec<String>,
    /// The recent repos whose folder no longer exists, as of `checked_at`.
    missing: HashSet<String>,
    /// When `missing` was computed (egui time, seconds); `None`: to compute again.
    checked_at: Option<f64>,
    /// The id of the next tab created (see `Tab::id`).
    next_id: u64,
    platform: Arc<dyn Platform>,
    opener: Arc<dyn RepoOpener>,
    wake: Wake,
    /// Folder chosen in the dialog; on desktop it arrives at once, in general from a callback.
    picked: Arc<Mutex<Option<String>>>,
    /// The state last written to the storage: written again only when it changes.
    saved: Saved,
}

impl App {
    /// `opener` opens repositories: `CliOpener` in the app, `MemoryOpener` in tests. The tabs of
    /// the previous launch reopen in the background; `repo`, if present, opens in one more tab (or
    /// shows the one where it is already open).
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        platform: Arc<dyn Platform>,
        opener: Arc<dyn RepoOpener>,
        repo: Option<String>,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        let wake: Wake = Arc::new(move || ctx.request_repaint());
        let saved = cc
            .storage
            .and_then(|s| eframe::get_value::<Saved>(s, KEY))
            .unwrap_or_default();
        Self::restore(platform, opener, wake, saved, repo)
    }

    fn restore(
        platform: Arc<dyn Platform>,
        opener: Arc<dyn RepoOpener>,
        wake: Wake,
        saved: Saved,
        repo: Option<String>,
    ) -> Self {
        let mut app = Self {
            tabs: Vec::new(),
            active: 0,
            recent: saved.recent.clone(),
            missing: HashSet::new(),
            checked_at: None,
            next_id: 0,
            platform,
            opener,
            wake,
            picked: Arc::default(),
            saved: saved.clone(),
        };
        for path in &saved.tabs {
            let mut tab = app.new_tab();
            tab.open(path.clone());
            app.tabs.push(tab);
        }
        if app.tabs.is_empty() {
            let tab = app.new_tab();
            app.tabs.push(tab);
        }
        app.active = saved.active.min(app.tabs.len() - 1);
        if let Some(path) = repo {
            app.open(path);
        }
        app
    }

    fn new_tab(&mut self) -> Tab {
        let id = self.next_id;
        self.next_id += 1;
        Tab::new(
            id,
            Executor::new(
                Session::new(Arc::clone(&self.opener)),
                Registry::builtin(),
                Arc::clone(&self.platform),
                Arc::clone(&self.wake),
            ),
        )
    }

    /// Shows the tab where `path` is open (or opening); otherwise opens it in the empty tab shown
    /// or in a new tab.
    fn open(&mut self, path: String) {
        if let Some(index) = self.tabs.iter().position(|t| t.path() == Some(&path)) {
            self.active = index;
            return;
        }
        if !self.tabs[self.active].is_empty() {
            let tab = self.new_tab();
            self.tabs.push(tab);
            self.active = self.tabs.len() - 1;
        }
        self.tabs[self.active].open(path);
    }

    /// Closes tab `index`, not while it is writing: the write would go on without anyone
    /// showing its outcome.
    fn close(&mut self, index: usize) {
        if self
            .tabs
            .get(index)
            .is_none_or(|t| t.executor.writing().is_some())
        {
            return;
        }
        self.remove(index);
    }

    fn remove(&mut self, index: usize) {
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            let tab = self.new_tab();
            self.tabs.push(tab);
        }
        self.active = after_close(self.active, index, self.tabs.len());
    }

    /// Collects the results of every tab, shown or not, and the repos just opened: they enter the
    /// recent list; the same repo already open in another tab (reached by another path) closes the
    /// new tab and shows the other one.
    fn receive(&mut self) {
        let folder = self.picked.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(folder) = folder {
            self.open(folder);
        }
        for tab in &mut self.tabs {
            tab.receive();
            tab.request_operation();
            // No repository there any more: it leaves the recent list.
            if let Some(path) = tab.not_found.take() {
                self.recent.retain(|p| *p != path);
            }
        }
        let mut index = 0;
        while index < self.tabs.len() {
            let Some(path) = self.tabs[index].opened.take() else {
                index += 1;
                continue;
            };
            remember(&mut self.recent, &path);
            let paths: Vec<_> = self.tabs.iter().map(|t| t.repo_path()).collect();
            match twin(&paths, index) {
                Some(other) => {
                    let shown = self.active == index;
                    self.remove(index);
                    if shown {
                        self.active = if other > index { other - 1 } else { other };
                    }
                }
                None => index += 1,
            }
        }
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::PickFolder => {
                let picked = Arc::clone(&self.picked);
                let ctx = ctx.clone();
                self.platform.pick_folder(Box::new(move |folder| {
                    if let Some(folder) = folder {
                        *picked.lock().unwrap_or_else(|e| e.into_inner()) = Some(folder);
                        ctx.request_repaint();
                    }
                }));
            }
            Action::Open(path) => self.open(path),
            Action::SwitchTab(index) => {
                if index < self.tabs.len() {
                    self.active = index;
                }
            }
            Action::CloseTab(index) => self.close(index),
            Action::ForgetMissing => {
                prune(&mut self.recent, &self.missing);
                self.missing.clear();
            }
            action => self.tabs[self.active].apply(action),
        }
    }

    /// The tabs with a repo open or opening (not those whose opening failed).
    fn state(&self) -> Saved {
        let mut tabs = Vec::new();
        let mut active = 0;
        for (index, tab) in self.tabs.iter().enumerate() {
            if let Some(path) = tab.path() {
                if index == self.active {
                    active = tabs.len();
                }
                tabs.push(path.to_owned());
            }
        }
        Saved {
            tabs,
            active,
            recent: self.recent.clone(),
        }
    }

    /// Writes the state as soon as it changes: the app can be closed without `save` being called
    /// (a killed process, `make desktop-stop`).
    fn persist(&mut self, storage: Option<&mut (dyn eframe::Storage + '_)>) {
        let state = self.state();
        if state == self.saved {
            return;
        }
        if let Some(storage) = storage {
            eframe::set_value(storage, KEY, &state);
            storage.flush();
        }
        self.saved = state;
    }

    /// Recomputes which recent folders are missing, at most every [`CHECK_EVERY`] seconds: not
    /// on every frame, each check touches the disk.
    fn check_missing(&mut self, now: f64) {
        if self
            .checked_at
            .is_some_and(|t| now - t < CHECK_EVERY && now >= t)
        {
            return;
        }
        self.checked_at = Some(now);
        self.missing = missing(&self.recent, |p| Path::new(p).is_dir());
    }

    /// The recent list for the menus, with the missing folders marked.
    fn recent_view(&self) -> Vec<view::RecentRepo> {
        self.recent
            .iter()
            .map(|path| view::RecentRepo {
                path: path.clone(),
                missing: self.missing.contains(path),
            })
            .collect()
    }

    fn labels(&self) -> Vec<view::TabLabel> {
        self.tabs
            .iter()
            .map(|tab| view::TabLabel {
                title: tab
                    .path()
                    .map_or_else(|| tr!("tabs-empty"), |p| view::repo_name(p).to_owned()),
                path: tab.path().map(str::to_owned),
                opening: tab.repo.is_none() && tab.requested.is_some(),
                writing: tab.executor.writing().is_some(),
            })
            .collect()
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.receive();
        let can_pick = self.platform.capabilities().open_folder;
        // The strip's actions after the tab's: they can change the tab shown.
        let mut strip = Vec::new();
        if self.tabs.len() > 1 || self.tabs[0].path().is_some() {
            let labels = self.labels();
            egui::Panel::top("tabs").show(ui, |ui| {
                view::tab_strip(ui, &labels, self.active, can_pick, &mut strip);
            });
        }
        self.check_missing(ctx.input(|i| i.time));
        let recent = self.recent_view();
        // Under the tab's own id: panel widths and scroll positions do not pass between tabs.
        let tab = &mut self.tabs[self.active];
        let actions = ui
            .push_id(("tab", tab.id), |ui| tab.ui(ui, can_pick, &recent))
            .inner;
        for action in actions.into_iter().chain(strip) {
            self.apply(&ctx, action);
        }
        self.tabs[self.active].after_actions();
        self.persist(frame.storage_mut());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.saved = self.state();
        eframe::set_value(storage, KEY, &self.saved);
    }
}

/// `path` at the top of the recent list, without duplicates and at most [`MAX_RECENT`].
fn remember(recent: &mut Vec<String>, path: &str) {
    recent.retain(|p| p != path);
    recent.insert(0, path.to_owned());
    recent.truncate(MAX_RECENT);
}

/// The recent repos for which `is_dir` says the folder no longer exists.
fn missing(recent: &[String], is_dir: impl Fn(&str) -> bool) -> HashSet<String> {
    recent.iter().filter(|p| !is_dir(p)).cloned().collect()
}

/// Drops the `missing` repos from `recent`, keeping the order of the others.
fn prune(recent: &mut Vec<String>, missing: &HashSet<String>) {
    recent.retain(|p| !missing.contains(p));
}

/// The tab shown after closing `closed`, with `len` tabs left (at least one): the same one if it
/// was another, otherwise the one that took its place or the last.
fn after_close(active: usize, closed: usize, len: usize) -> usize {
    if active > closed {
        active - 1
    } else {
        active.min(len - 1)
    }
}

/// Another tab with the same repo as tab `index`.
fn twin(paths: &[Option<&str>], index: usize) -> Option<usize> {
    let path = paths.get(index).copied().flatten()?;
    (0..paths.len()).find(|&other| other != index && paths[other] == Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_engine::{Capabilities, MemoryBackend, MemoryOpener, Picked};

    /// The job runs inside `spawn`, on the same thread.
    struct Inline;

    impl Platform for Inline {
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn pick_file(&self, on_done: Box<dyn FnOnce(Option<Picked>) + Send>) {
            on_done(None);
        }
        fn save_file(&self, _: &str, _: Vec<u8>) {}
        fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
            on_done(None);
        }
        fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
            job();
        }
    }

    /// An app with repos `a` and `b`, restored from `saved`, after the answers arrived.
    fn app(saved: Saved, repo: Option<&str>) -> App {
        let mut opener = MemoryOpener::new();
        opener.insert("a", MemoryBackend::sample());
        opener.insert("b", MemoryBackend::sample());
        let mut app = App::restore(
            Arc::new(Inline),
            Arc::new(opener),
            Arc::new(|| {}),
            saved,
            repo.map(str::to_owned),
        );
        app.receive();
        app
    }

    fn paths(app: &App) -> Vec<Option<&str>> {
        app.tabs.iter().map(|t| t.path()).collect()
    }

    #[test]
    fn recent_is_deduplicated_and_capped() {
        let mut recent = Vec::new();
        for n in 0..12 {
            remember(&mut recent, &format!("r{n}"));
        }
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0], "r11");
        remember(&mut recent, "r5");
        assert_eq!(recent[0], "r5");
        assert_eq!(recent.iter().filter(|p| *p == "r5").count(), 1);
        assert_eq!(recent.len(), MAX_RECENT);
    }

    #[test]
    fn missing_folders_are_found_and_pruned_in_order() {
        let mut recent: Vec<String> = ["/a", "/gone", "/b", "/gone2"].map(String::from).into();
        let gone = missing(&recent, |p| !p.starts_with("/gone"));
        assert_eq!(gone, HashSet::from(["/gone".into(), "/gone2".into()]));
        prune(&mut recent, &gone);
        assert_eq!(recent, ["/a", "/b"]);
        prune(&mut recent, &HashSet::new());
        assert_eq!(recent, ["/a", "/b"]);
    }

    #[test]
    fn forget_missing_drops_them_and_checks_at_most_every_second() {
        let saved = Saved {
            recent: vec!["/surely/not/a/folder".into(), ".".into()],
            ..Saved::default()
        };
        let mut app = app(saved, None);
        app.check_missing(10.0);
        assert!(app.recent_view()[0].missing);
        assert!(!app.recent_view()[1].missing);
        // Within the second: not checked again.
        app.recent.push("/another/missing".into());
        app.check_missing(10.5);
        assert!(!app.missing.contains("/another/missing"));
        app.check_missing(11.0);
        assert!(app.missing.contains("/another/missing"));
        app.apply(&egui::Context::default(), Action::ForgetMissing);
        assert_eq!(app.recent, ["."]);
        assert_eq!(app.state().recent, ["."]);
    }

    #[test]
    fn opening_a_vanished_recent_repo_drops_it() {
        let saved = Saved {
            recent: vec!["a".into(), "gone".into()],
            ..Saved::default()
        };
        let mut app = app(saved, None);
        app.open("gone".into());
        app.receive();
        assert_eq!(app.recent, ["a"]);
    }

    #[test]
    fn tab_ids_are_stable_and_never_reused() {
        let mut app = app(Saved::default(), Some("a"));
        app.open("b".into());
        app.receive();
        let ids: Vec<_> = app.tabs.iter().map(|t| t.id).collect();
        assert_eq!(ids, [0, 1]);
        // Closing the first: the second keeps its id (not its index).
        app.close(0);
        assert_eq!(app.tabs[0].id, 1);
        app.open("a".into());
        app.receive();
        assert_eq!(app.tabs.iter().map(|t| t.id).collect::<Vec<_>>(), [1, 2]);
        // Closing the last one: the new empty tab gets a new id too.
        app.close(1);
        app.close(0);
        assert_eq!(app.tabs[0].id, 3);
    }

    #[test]
    fn active_tab_after_close() {
        // Closing a tab before the one shown: the same tab, one place to the left.
        assert_eq!(after_close(2, 0, 2), 1);
        // Closing one after: unchanged.
        assert_eq!(after_close(0, 1, 2), 0);
        // Closing the one shown: the one that took its place, or the last.
        assert_eq!(after_close(1, 1, 2), 1);
        assert_eq!(after_close(2, 2, 2), 1);
        assert_eq!(after_close(0, 0, 1), 0);
    }

    #[test]
    fn twin_finds_the_same_repo_in_another_tab() {
        let paths = [Some("a"), None, Some("b"), Some("a")];
        assert_eq!(twin(&paths, 3), Some(0));
        assert_eq!(twin(&paths, 0), Some(3));
        assert_eq!(twin(&paths, 2), None);
        assert_eq!(twin(&paths, 1), None);
    }

    #[test]
    fn saved_state_round_trips_and_tolerates_missing_fields() {
        let saved = Saved {
            tabs: vec!["/a".into(), "/b".into()],
            active: 1,
            recent: vec!["/b".into(), "/a".into(), "/c".into()],
        };
        let text = serde_json::to_string(&saved).unwrap();
        assert_eq!(serde_json::from_str::<Saved>(&text).unwrap(), saved);
        let old: Saved = serde_json::from_str(r#"{"tabs":["/a"]}"#).unwrap();
        assert_eq!(old.tabs, ["/a"]);
        assert_eq!((old.active, old.recent.len()), (0, 0));
    }

    #[test]
    fn opening_fills_the_empty_tab_then_adds_tabs_and_reuses_open_ones() {
        let mut app = app(Saved::default(), None);
        assert_eq!(paths(&app), [None]);
        app.open("a".into());
        app.receive();
        assert_eq!(paths(&app), [Some("a")]);
        app.open("b".into());
        app.receive();
        assert_eq!((paths(&app), app.active), (vec![Some("a"), Some("b")], 1));
        // Already open: only shown.
        app.open("a".into());
        app.receive();
        assert_eq!((app.tabs.len(), app.active), (2, 0));
        assert_eq!(app.recent, ["b", "a"]);
    }

    #[test]
    fn failed_open_stays_out_of_recent_and_saved_tabs() {
        let mut app = app(Saved::default(), Some("a"));
        app.open("missing".into());
        app.receive();
        assert_eq!(app.tabs.len(), 2);
        assert!(app.tabs[1].repo.is_none());
        assert!(!app.tabs[1].error.is_empty());
        assert_eq!(app.recent, ["a"]);
        assert_eq!(app.state().tabs, ["a"]);
        // The failed tab is empty: the next repo opens there.
        app.open("b".into());
        app.receive();
        assert_eq!(paths(&app), [Some("a"), Some("b")]);
    }

    #[test]
    fn saved_tabs_reopen_and_the_argument_shows_its_tab() {
        let saved = Saved {
            tabs: vec!["a".into(), "b".into()],
            active: 1,
            recent: vec!["b".into(), "a".into()],
        };
        let app_ = app(saved.clone(), None);
        assert_eq!(paths(&app_), [Some("a"), Some("b")]);
        assert_eq!(app_.state(), saved);
        // The argument already among the tabs: shown, not duplicated.
        let app_ = app(saved, Some("a"));
        assert_eq!((app_.tabs.len(), app_.active), (2, 0));
    }

    #[test]
    fn closing_keeps_one_tab_and_persists_only_on_change() {
        let mut app = app(Saved::default(), Some("a"));
        app.open("b".into());
        app.receive();
        app.close(1);
        assert_eq!((paths(&app), app.active), (vec![Some("a")], 0));
        app.close(0);
        assert_eq!(paths(&app), [None]);
        let mut storage = Memory::default();
        app.persist(Some(&mut storage));
        assert_eq!(storage.flushes, 1);
        app.persist(Some(&mut storage));
        assert_eq!(storage.flushes, 1, "unchanged state: not written again");
        let saved: Saved = eframe::get_value(&storage, KEY).unwrap();
        assert_eq!(
            (saved.tabs.len(), saved.recent),
            (0, vec!["b".into(), "a".into()])
        );
    }

    #[derive(Default)]
    struct Memory {
        values: std::collections::HashMap<String, String>,
        flushes: usize,
    }

    impl eframe::Storage for Memory {
        fn get_string(&self, key: &str) -> Option<String> {
            self.values.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.values.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.values.remove(key);
        }
        fn flush(&mut self) {
            self.flushes += 1;
        }
    }
}
