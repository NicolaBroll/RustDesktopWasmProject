//! Several repos open in tabs, the recent repos and their persistence between launches.
//!
//! Each [`Tab`] has its own executor and `Session`: undo/redo journals stay per repo and the
//! results of a tab not shown keep landing in it. What is saved (eframe storage): the paths of the
//! open tabs, the one shown and the recent repos; at startup the tabs reopen.

use crate::clone::{CloneBox, Received};
use crate::executor::{Executor, Wake};
use crate::settings::{self, Settings, SettingsBox};
use crate::{Action, Tab, view};
use project_engine::{Platform, Registry, RepoOpener, Session};
use project_i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Folder chosen in the dialog, and what for: it arrives from a callback, on desktop from
    /// another thread when the dialog closes.
    picked: Arc<Mutex<Option<(Pick, String)>>>,
    /// A folder dialog is open: a second "Open…" does nothing until it closes.
    picking: Arc<AtomicBool>,
    /// The state last written to the storage: written again only when it changes.
    saved: Saved,
    /// Whether the window had focus in the last frame (`None`: not known yet).
    focused: Option<bool>,
    /// Who draws the window frame: with [`view::WindowFrame::Custom`] the menu bar is also the
    /// title bar.
    frame: view::WindowFrame,
    /// The "Clone…"/"New repository…" dialog.
    clone: CloneBox,
    /// The tab, not shown yet, whose session runs `repo.clone`/`repo.init`: it joins the tabs when
    /// the new repo is open.
    cloning: Option<Tab>,
    /// Theme, language, diff layout.
    settings: Settings,
    /// The settings last written to the storage (`None`: never written): written again only when
    /// they change.
    settings_saved: Option<Settings>,
    /// The "Settings…" window.
    settings_box: SettingsBox,
}

/// What the folder chosen in the dialog is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    /// "Open…": the repo opens in a tab.
    Open,
    /// "Browse…" of the clone dialog: the destination.
    Destination,
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
        crate::icons::install(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let wake: Wake = Arc::new(move || ctx.request_repaint());
        let saved = cc
            .storage
            .and_then(|s| eframe::get_value::<Saved>(s, KEY))
            .unwrap_or_default();
        let settings = Settings::load(cc.storage, &cc.egui_ctx);
        let stored = cc
            .storage
            .and_then(|s| eframe::get_value::<Settings>(s, settings::KEY));
        let mut app = Self::restore(platform, opener, wake, saved, repo);
        app.set_settings(&cc.egui_ctx, settings, stored);
        app
    }

    /// Applies `settings` (theme and language at once); `stored`: what the storage holds.
    fn set_settings(&mut self, ctx: &egui::Context, settings: Settings, stored: Option<Settings>) {
        settings.apply(ctx, self.platform.as_ref());
        self.settings = settings;
        self.settings_saved = stored;
    }

    /// Who draws the window frame (default: the platform).
    #[must_use]
    pub fn with_window_frame(mut self, frame: view::WindowFrame) -> Self {
        self.frame = frame;
        self
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
            picking: Arc::default(),
            saved: saved.clone(),
            focused: None,
            frame: view::WindowFrame::Native,
            clone: CloneBox::default(),
            cloning: None,
            settings: Settings::default(),
            settings_saved: Some(Settings::default()),
            settings_box: SettingsBox::default(),
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
        match folder {
            Some((Pick::Open, folder)) => self.open(folder),
            Some((Pick::Destination, folder)) => self.clone.set_folder(&folder),
            None => {}
        }
        self.receive_clone();
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
            let wake = Arc::clone(&self.wake);
            self.tabs[index].auto.start(&*self.platform, &path, wake);
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

    /// The answer of `repo.clone`/`repo.init`: the new repo, open in the session of the tab that
    /// ran it, is shown in that tab (in place of the empty tab shown, or as a new one); the loop of
    /// [`App::receive`] then puts it in the recent list. Failed or cancelled: the tab goes away,
    /// the dialog shows why.
    fn receive_clone(&mut self) {
        let Some(tab) = &mut self.cloning else {
            return;
        };
        tab.receive();
        let Some(done) = tab.clone_done.take() else {
            return;
        };
        let received =
            self.clone
                .receive(done.ticket, done.result, done.error_kind, done.error_detail);
        let Some(mut tab) = self.cloning.take() else {
            return;
        };
        if let Received::Opened { value, notice } = received {
            tab.created(value, notice);
            if self.tabs[self.active].is_empty() {
                self.tabs[self.active] = tab;
            } else {
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
            }
        }
    }

    /// The folder dialog, one at a time: the choice arrives in `picked` with `what`.
    fn pick_folder(&mut self, ctx: &egui::Context, what: Pick) {
        if self.picking.swap(true, Ordering::AcqRel) {
            return;
        }
        let picked = Arc::clone(&self.picked);
        // Cleared when the callback is called or dropped (a platform that cannot show
        // the dialog): "Open…" never stays dead.
        let picking = Picking(Arc::clone(&self.picking));
        let ctx = ctx.clone();
        self.platform.pick_folder(Box::new(move |folder| {
            if let Some(folder) = folder {
                *picked.lock().unwrap_or_else(|e| e.into_inner()) = Some((what, folder));
            }
            drop(picking);
            ctx.request_repaint();
        }));
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::PickFolder => self.pick_folder(ctx, Pick::Open),
            Action::OpenClone(kind) => self.clone.open(kind),
            Action::CloneFields(fields) => self.clone.set_fields(fields),
            Action::BrowseCloneFolder => self.pick_folder(ctx, Pick::Destination),
            Action::SubmitClone => {
                if self.cloning.is_some() {
                    return;
                }
                let Some((command, params)) = self.clone.submit() else {
                    return;
                };
                let mut tab = self.new_tab();
                let ticket = tab.create_repo(command, params);
                self.clone.started(ticket);
                self.cloning = Some(tab);
            }
            Action::CancelClone => {
                if let (Some(ticket), Some(tab)) = (self.clone.cancel(), &self.cloning) {
                    tab.executor.cancel(ticket);
                }
            }
            Action::CloseClone => self.clone.close(),
            Action::OpenSettings => self.settings_box.open = true,
            Action::CloseSettings => self.settings_box.open = false,
            Action::SettingsSection(section) => self.settings_box.section = section,
            Action::Setting(change) => {
                let mut settings = self.settings.clone();
                settings.change(change);
                if settings != self.settings {
                    settings.apply(ctx, self.platform.as_ref());
                    self.settings = settings;
                }
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
    fn persist(&mut self, mut storage: Option<&mut (dyn eframe::Storage + '_)>) {
        let mut changed = false;
        let state = self.state();
        if state != self.saved {
            if let Some(storage) = storage.as_deref_mut() {
                eframe::set_value(storage, KEY, &state);
                changed = true;
            }
            self.saved = state;
        }
        if self.settings_saved.as_ref() != Some(&self.settings) {
            if let Some(storage) = storage.as_deref_mut() {
                eframe::set_value(storage, settings::KEY, &self.settings);
                changed = true;
            }
            self.settings_saved = Some(self.settings.clone());
        }
        if let Some(storage) = storage
            && changed
        {
            storage.flush();
        }
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

    /// The window gained focus: the tab shown re-reads (the worktree is not watched, an editor may
    /// have changed it). Then every tab gets its tick: only the one shown re-reads now.
    fn auto_refresh(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.viewport().focused);
        if crate::refresh::focus_gained(&mut self.focused, focused) {
            self.tabs[self.active].auto.mark();
        }
        let now = std::time::Instant::now();
        let interval = self.settings.fetch_minutes.duration();
        let app_blocked = self.clone.dialog().is_some() || self.settings_box.open;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let shown = index == self.active;
            tab.tick(now, shown);
            tab.tick_fetch(now, shown, interval, app_blocked);
        }
        // The timer of the tab shown ticks even when nothing else repaints the window.
        if let Some(wait) = self.tabs[self.active].fetch_wake(now) {
            ctx.request_repaint_after(wait);
        }
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
        self.auto_refresh(&ctx);
        let can_pick = self.platform.capabilities().open_folder;
        self.check_missing(ctx.input(|i| i.time));
        let recent = self.recent_view();
        // The actions of the menus and of the strip after the tab's: they can change the tab shown.
        let mut strip = Vec::new();
        let bar = view::TitleBar {
            frame: self.frame,
            repo: self.tabs[self.active].repo.as_ref(),
            can_pick,
            recent: &recent,
            theme: self.settings.theme,
        };
        view::title_bar_panel(ui, &bar, &mut strip);
        if self.tabs.len() > 1 || self.tabs[0].path().is_some() {
            let labels = self.labels();
            view::tab_strip_panel(ui, &labels, self.active, can_pick, &mut strip);
        }
        let modal = self.clone.dialog().is_some() || self.settings_box.open;
        if !modal && ctx.input_mut(|i| i.consume_shortcut(&settings::SHORTCUT)) {
            strip.push(Action::OpenSettings);
        }
        // Under the tab's own id: panel widths and scroll positions do not pass between tabs.
        let tab = &mut self.tabs[self.active];
        let settings = &self.settings;
        let mut actions = ui
            .push_id(("tab", tab.id), |ui| {
                tab.ui(ui, can_pick, &recent, settings)
            })
            .inner;
        if self.settings_box.open {
            // Modal as well: what happened under it is dropped.
            actions.clear();
            strip.clear();
            view::settings_window(&ctx, &self.settings_box, &self.settings, &mut actions);
        } else if let Some(dialog) = self.clone.dialog() {
            // The dialog is modal: the clicks under it are blocked, the tab's keys are dropped.
            actions.clear();
            strip.clear();
            let progress = self
                .clone
                .running()
                .zip(self.cloning.as_ref())
                .and_then(|(running, tab)| tab.executor.progress(running.ticket));
            view::clone_dialog(&ctx, dialog, progress.as_ref(), can_pick, &mut actions);
        }
        for action in actions.into_iter().chain(strip) {
            self.apply(&ctx, action);
        }
        self.tabs[self.active].after_actions(&ctx);
        view::window_edges(&ctx, self.frame);
        self.persist(frame.storage_mut());
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        view::end_window_move(ctx, raw_input);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.saved = self.state();
        eframe::set_value(storage, KEY, &self.saved);
        eframe::set_value(storage, settings::KEY, &self.settings);
        self.settings_saved = Some(self.settings.clone());
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

/// Marks the folder dialog closed when dropped.
struct Picking(Arc<AtomicBool>);

impl Drop for Picking {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
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
    use crate::clone::Kind;
    use project_engine::{Capabilities, MemoryBackend, MemoryOpener};

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

    type OnPicked = Box<dyn FnOnce(Option<String>) + Send>;

    /// Keeps the dialog "open": the callbacks wait in `pending` until the test calls them.
    #[derive(Default)]
    struct Deferred {
        pending: Mutex<Vec<OnPicked>>,
    }

    impl Platform for Deferred {
        fn capabilities(&self) -> Capabilities {
            Capabilities { open_folder: true }
        }
        fn pick_folder(&self, on_done: OnPicked) {
            self.pending.lock().unwrap().push(on_done);
        }
        fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
            job();
        }
    }

    #[test]
    fn only_one_folder_dialog_at_a_time() {
        let platform = Arc::new(Deferred::default());
        let mut opener = MemoryOpener::new();
        opener.insert("a", MemoryBackend::sample());
        let mut app = App::restore(
            Arc::clone(&platform) as Arc<dyn Platform>,
            Arc::new(opener),
            Arc::new(|| {}),
            Saved::default(),
            None,
        );
        let ctx = egui::Context::default();
        app.apply(&ctx, Action::PickFolder);
        app.apply(&ctx, Action::PickFolder);
        let first = platform.pending.lock().unwrap().pop().unwrap();
        assert!(platform.pending.lock().unwrap().is_empty());
        // Cancelled: the next click opens a dialog again.
        first(None);
        app.apply(&ctx, Action::PickFolder);
        let second = platform.pending.lock().unwrap().pop().unwrap();
        second(Some("a".to_owned()));
        app.receive();
        assert_eq!(paths(&app), [Some("a")]);
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

    #[test]
    fn settings_are_written_under_their_key_only_when_they_change() {
        use crate::settings::{Change, Theme};
        let mut app = app(Saved::default(), None);
        let ctx = egui::Context::default();
        let mut storage = Memory::default();
        app.persist(Some(&mut storage));
        assert_eq!(storage.flushes, 0, "nothing changed");
        app.apply(&ctx, Action::Setting(Change::Theme(Theme::Dark)));
        app.apply(&ctx, Action::Setting(Change::DiffSplit(true)));
        assert_eq!(
            ctx.options(|o| o.theme_preference),
            egui::ThemePreference::Dark
        );
        app.persist(Some(&mut storage));
        assert_eq!(storage.flushes, 1);
        app.persist(Some(&mut storage));
        assert_eq!(storage.flushes, 1, "unchanged settings: not written again");
        assert!(storage.values.contains_key(settings::KEY));
        // Round trip through the storage.
        let loaded = Settings::load(Some(&storage), &egui::Context::default());
        assert_eq!(loaded, app.settings);
        assert_eq!((loaded.theme, loaded.diff_split), (Theme::Dark, true));
    }

    #[test]
    fn an_old_settings_save_without_the_new_fields_loads() {
        use crate::settings::{Language, Theme};
        let mut storage = Memory::default();
        // RON, as eframe writes it, from before the language and the diff layout.
        eframe::Storage::set_string(
            &mut storage,
            settings::KEY,
            r#"(theme:"light",unknown:3)"#.to_owned(),
        );
        let loaded = Settings::load(Some(&storage), &egui::Context::default());
        assert_eq!(
            (loaded.theme, loaded.language, loaded.diff_split),
            (Theme::Light, Language::System, false)
        );
    }

    /// Keeps the spawned jobs until the test runs them (a clone still in progress).
    #[derive(Default)]
    struct Queued {
        jobs: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    }

    impl Platform for Queued {
        fn capabilities(&self) -> Capabilities {
            Capabilities { open_folder: true }
        }
        fn pick_folder(&self, on_done: OnPicked) {
            on_done(Some("/work".to_owned()));
        }
        fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
            self.jobs.lock().unwrap().push(job);
        }
    }

    impl Queued {
        fn run_all(&self) {
            loop {
                let jobs = std::mem::take(&mut *self.jobs.lock().unwrap());
                if jobs.is_empty() {
                    break;
                }
                jobs.into_iter().for_each(|job| job());
            }
        }
    }

    fn clone_fields(url: &str, path: &str) -> Action {
        Action::CloneFields(crate::clone::Fields {
            url: url.into(),
            path: path.into(),
            ..Default::default()
        })
    }

    #[test]
    fn a_clone_opens_in_the_empty_tab_then_in_new_tabs_and_enters_recent() {
        let ctx = egui::Context::default();
        let mut app = app(Saved::default(), None);
        app.apply(&ctx, Action::OpenClone(Kind::Clone));
        app.apply(&ctx, clone_fields("a", "copy"));
        app.apply(&ctx, Action::SubmitClone);
        app.receive();
        assert!(app.clone.dialog().is_none());
        assert_eq!(paths(&app), [Some("copy")]);
        assert_eq!(app.recent, ["copy"]);
        assert!(
            !app.tabs[0].repo.as_ref().unwrap().list.commits().is_empty(),
            "log read"
        );
        // With a repo shown: a new tab, shown.
        app.apply(&ctx, Action::OpenClone(Kind::Init));
        app.apply(&ctx, clone_fields("", "fresh"));
        app.apply(&ctx, Action::SubmitClone);
        app.receive();
        assert_eq!(
            (paths(&app), app.active),
            (vec![Some("copy"), Some("fresh")], 1)
        );
        assert_eq!(app.recent, ["fresh", "copy"]);
        assert_eq!(app.state().tabs, ["copy", "fresh"]);
    }

    #[test]
    fn a_failed_clone_leaves_the_tabs_alone_and_the_dialog_open() {
        let ctx = egui::Context::default();
        let mut app = app(Saved::default(), Some("a"));
        app.apply(&ctx, Action::OpenClone(Kind::Clone));
        app.apply(&ctx, clone_fields("nowhere", "copy"));
        app.apply(&ctx, Action::SubmitClone);
        app.receive();
        assert_eq!(paths(&app), [Some("a")]);
        assert!(app.cloning.is_none());
        let dialog = app.clone.dialog().unwrap();
        assert!(dialog.failure.is_some() && dialog.can_submit());
        assert_eq!(app.recent, ["a"]);
    }

    #[test]
    fn a_clone_in_progress_shows_no_tab_and_can_be_cancelled() {
        let platform = Arc::new(Queued::default());
        let mut opener = MemoryOpener::new();
        opener.insert("a", MemoryBackend::sample());
        let mut app = App::restore(
            Arc::clone(&platform) as Arc<dyn Platform>,
            Arc::new(opener),
            Arc::new(|| {}),
            Saved::default(),
            None,
        );
        let ctx = egui::Context::default();
        app.apply(&ctx, Action::OpenClone(Kind::Clone));
        app.apply(&ctx, clone_fields("https://host/a.git", ""));
        // "Browse…": the destination is the chosen folder plus the name.
        app.apply(&ctx, Action::BrowseCloneFolder);
        app.receive();
        let path = app.clone.dialog().unwrap().fields.path.clone();
        assert_eq!(path, Path::new("/work").join("a").to_string_lossy());
        app.apply(&ctx, clone_fields("a", "copy"));
        app.apply(&ctx, Action::SubmitClone);
        app.receive();
        assert!(app.clone.running().is_some());
        assert_eq!(paths(&app), [None], "not among the tabs yet");
        // Not closed while running; a second submit does nothing.
        app.apply(&ctx, Action::CloseClone);
        app.apply(&ctx, Action::SubmitClone);
        assert!(app.clone.dialog().is_some());
        app.apply(&ctx, Action::CancelClone);
        platform.run_all();
        app.receive();
        let dialog = app.clone.dialog().unwrap();
        assert!(dialog.cancelled && dialog.running.is_none());
        assert!(app.cloning.is_none());
        assert_eq!(paths(&app), [None]);
        assert!(app.recent.is_empty());
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
