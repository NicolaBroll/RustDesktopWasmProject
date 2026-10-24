//! State of the commit list: pages of `repo.log` loaded while scrolling, and
//! mouse and keyboard selection, graph lanes. Data only, no egui: tested with unit
//! tests.

use project_domain::{Commit, GraphLayout, Oid, layout_graph};
use serde_json::{Value, json};

/// Commits requested for each page of `repo.log`.
pub const PAGE: usize = 500;

/// How far pages are loaded to find the commit of a clicked ref (see
/// [`CommitList::seek`]): beyond that it says it is not among the loaded ones.
pub const SEEK_LIMIT: usize = 20 * PAGE;

/// How many rows before the end of the loaded commits the next page is requested.
const PREFETCH: usize = PAGE / 2;

/// A page request: `skip` and `limit` of `repo.log`, plus the epoch of the list it
/// belongs to (see [`CommitList::reset`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    pub skip: usize,
    pub limit: usize,
    pub epoch: u64,
}

impl PageRequest {
    /// The parameters of `repo.log`: the graph starts from all refs (local and remote branches, tags,
    /// HEAD), so unmerged and just-fetched branches show up too, and from the bases
    /// of the stashes, so every stash has its row even if no branch contains it anymore.
    pub fn params(&self) -> Value {
        json!({ "skip": self.skip, "limit": self.limit, "all": true, "stashes": true })
    }
}

/// Outcome of [`CommitList::seek`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seek {
    /// The commit was loaded and is selected.
    Selected,
    /// Not loaded yet: it is searched in the following pages. Here is the page to request, or
    /// `None` if a page is already on its way (the search continues when it arrives).
    Loading(Option<PageRequest>),
    /// It is not among the loaded commits and the search goes no further.
    Missing,
}

/// Where the "Uncommitted changes" row attaches: the HEAD commit in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    /// HEAD row among the commits.
    pub index: usize,
    /// HEAD column in the graph.
    pub column: usize,
    /// True if in the rows above HEAD its column is free: a line can be drawn from the
    /// changes row down to HEAD without covering other edges.
    pub free_above: bool,
}

/// The commits loaded so far and the selected commit.
#[derive(Debug, Default)]
pub struct CommitList {
    commits: Vec<Commit>,
    /// Graph lanes of all loaded commits, one row per commit. They are recomputed when
    /// a page arrives, not on every frame: a lane may depend on commits from different
    /// pages.
    graph: GraphLayout,
    has_more: bool,
    /// The requested page that has not arrived yet.
    loading: Option<PageRequest>,
    /// Grows at every [`CommitList::reset`]: pages requested before are discarded.
    epoch: u64,
    selected: Option<usize>,
    /// Row to bring into view on the next frame (after a keyboard move).
    scroll_to: Option<usize>,
    /// The commit being searched in the following pages (see [`CommitList::seek`]).
    seeking: Option<Oid>,
}

impl CommitList {
    pub fn commits(&self) -> &[Commit] {
        &self.commits
    }

    /// The graph lanes: `graph().rows[i]` is the row of `commits()[i]`.
    pub fn graph(&self) -> &GraphLayout {
        &self.graph
    }

    /// True if there are more commits after the last loaded one.
    pub fn has_more(&self) -> bool {
        self.has_more
    }

    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected_commit(&self) -> Option<&Commit> {
        self.commits.get(self.selected?)
    }

    /// Starts again from the first page (repo opened or refreshed) and returns it to be requested.
    /// The commits already loaded stay visible until the new first page arrives.
    pub fn reset(&mut self) -> PageRequest {
        self.epoch += 1;
        self.seeking = None;
        let request = PageRequest {
            skip: 0,
            limit: PAGE,
            epoch: self.epoch,
        };
        self.loading = Some(request);
        request
    }

    /// The next page to request if row `visible_end` (excluded) is near the end of the
    /// loaded commits; `None` if not needed or if a page is already on its way.
    pub fn next_page(&mut self, visible_end: usize) -> Option<PageRequest> {
        if !self.has_more || self.loading.is_some() || visible_end + PREFETCH < self.commits.len() {
            return None;
        }
        let request = PageRequest {
            skip: self.commits.len(),
            limit: PAGE,
            epoch: self.epoch,
        };
        self.loading = Some(request);
        Some(request)
    }

    /// A page that arrived. If it is the first it replaces the list and keeps the same commit
    /// selected (or the first); the others are appended at the end. Pages of a past epoch
    /// or that do not attach to the end of the list are discarded: returns `false`.
    pub fn receive(&mut self, request: PageRequest, commits: Vec<Commit>, has_more: bool) -> bool {
        if self.loading != Some(request) {
            return false;
        }
        self.loading = None;
        if request.skip == 0 {
            let selected = self.selected_commit().map(|c| c.id);
            self.commits = commits;
            self.selected = selected
                .and_then(|id| self.position(id))
                .or((!self.commits.is_empty()).then_some(0));
        } else {
            self.commits.extend(commits);
        }
        self.graph = layout_graph(&self.commits);
        self.has_more = has_more;
        true
    }

    /// The requested page did not arrive (error): it can be requested again.
    pub fn failed(&mut self, request: PageRequest) {
        if self.loading == Some(request) {
            self.loading = None;
            self.seeking = None;
        }
    }

    /// The commit being searched in the following pages, if a search is in progress.
    pub fn seeking(&self) -> Option<Oid> {
        self.seeking
    }

    /// Selects commit `id` (click on a ref) and brings it into view. If it is not loaded yet
    /// and there are more pages, it searches by requesting them one after the other, up to
    /// [`SEEK_LIMIT`] commits: whoever receives a page calls `seek` again with [`CommitList::seeking`].
    pub fn seek(&mut self, id: Oid) -> Seek {
        self.seeking = None;
        if let Some(index) = self.position(id) {
            self.selected = Some(index);
            self.scroll_to = Some(index);
            return Seek::Selected;
        }
        if !self.has_more || self.commits.len() >= SEEK_LIMIT {
            return Seek::Missing;
        }
        self.seeking = Some(id);
        if self.loading.is_some() {
            return Seek::Loading(None);
        }
        let request = PageRequest {
            skip: self.commits.len(),
            limit: PAGE,
            epoch: self.epoch,
        };
        self.loading = Some(request);
        Seek::Loading(Some(request))
    }

    /// Where to attach the changes row: row and column of commit `head`, if loaded.
    pub fn anchor(&self, head: Oid) -> Option<Anchor> {
        let index = self.position(head)?;
        let column = self.graph.rows.get(index)?.column;
        let free_above = self.graph.rows[..index].iter().all(|row| {
            row.column != column
                && row
                    .segments
                    .iter()
                    .all(|s| s.from != column && s.to != column)
        });
        Some(Anchor {
            index,
            column,
            free_above,
        })
    }

    fn position(&self, id: Oid) -> Option<usize> {
        self.commits.iter().position(|c| c.id == id)
    }

    /// Selects row `index` (mouse click); outside the list it does nothing.
    pub fn select(&mut self, index: usize) {
        self.seeking = None;
        if index < self.commits.len() {
            self.selected = Some(index);
        }
    }

    /// Selects commit `id` and brings it into view; `false` if it is not among the loaded ones.
    pub fn select_id(&mut self, id: Oid) -> bool {
        self.seeking = None;
        match self.position(id) {
            Some(index) => {
                self.selected = Some(index);
                self.scroll_to = Some(index);
                true
            }
            None => false,
        }
    }

    /// Moves the selection by `delta` rows (negative upwards), staying in the list, and
    /// brings it into view. Without a selection it starts from the first row.
    pub fn move_selection(&mut self, delta: isize) {
        self.seeking = None;
        let Some(last) = self.commits.len().checked_sub(1) else {
            return;
        };
        let index = match self.selected {
            Some(current) => current.saturating_add_signed(delta).min(last),
            None => 0,
        };
        self.selected = Some(index);
        self.scroll_to = Some(index);
    }

    /// Selects the first row (`true`) or the last loaded one (`false`).
    pub fn select_edge(&mut self, first: bool) {
        self.seeking = None;
        if let Some(last) = self.commits.len().checked_sub(1) {
            let index = if first { 0 } else { last };
            self.selected = Some(index);
            self.scroll_to = Some(index);
        }
    }

    /// The row to bring into view, without consuming it.
    pub fn peek_scroll_to(&self) -> Option<usize> {
        self.scroll_to
    }

    /// The row to bring into view, only once: after it has been drawn.
    pub fn take_scroll_to(&mut self) -> Option<usize> {
        self.scroll_to.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::Signature;

    fn commit(n: usize) -> Commit {
        let mut bytes = [0u8; 20];
        bytes[..8].copy_from_slice(&(n as u64).to_be_bytes());
        let sig = Signature {
            name: "Ada".to_owned(),
            email: "ada@example.com".to_owned(),
            time: 0,
            offset_min: 0,
        };
        Commit {
            id: Oid::from_bytes(bytes),
            parents: Vec::new(),
            author: sig.clone(),
            committer: sig,
            summary: format!("commit {n}"),
            body: String::new(),
        }
    }

    /// A page of a history of `total` commits, as `repo.log` would give it.
    fn page(request: PageRequest, total: usize) -> (Vec<Commit>, bool) {
        let end = (request.skip + request.limit).min(total);
        let commits = (request.skip.min(end)..end).map(commit).collect();
        (commits, end < total)
    }

    fn load(list: &mut CommitList, request: PageRequest, total: usize) {
        let (commits, more) = page(request, total);
        assert!(list.receive(request, commits, more));
    }

    #[test]
    fn scrolling_loads_every_page_once() {
        let total = 50_000;
        let mut list = CommitList::default();
        let first = list.reset();
        assert_eq!(
            list.next_page(0),
            None,
            "the first page is already on its way"
        );
        load(&mut list, first, total);
        assert_eq!(list.selected(), Some(0));

        // Visible rows far from the end: no pages.
        assert_eq!(list.next_page(10), None);

        let mut requests = 1;
        let mut visible_end = 0;
        while list.has_more() {
            visible_end += 40;
            if let Some(request) = list.next_page(visible_end) {
                assert_eq!(request.skip, list.commits().len());
                assert_eq!(list.next_page(visible_end), None, "one page at a time");
                load(&mut list, request, total);
                requests += 1;
            }
        }
        assert_eq!(list.commits().len(), total);
        assert_eq!(requests, total.div_ceil(PAGE));
        let ids: Vec<Oid> = list.commits().iter().map(|c| c.id).collect();
        assert_eq!(ids, (0..total).map(|n| commit(n).id).collect::<Vec<_>>());
        assert_eq!(list.next_page(total), None);
    }

    #[test]
    fn graph_spans_every_loaded_page() {
        // A linear history with a branch that opens in the first page and closes with a
        // merge in the second: the branch lane must cross the boundary between the pages.
        let total = PAGE + 100;
        let merge = PAGE + 10;
        let mut history: Vec<Commit> = (0..total)
            .map(|n| {
                let mut c = commit(n);
                // Commit n has n + 1 as parent (the list goes from the most recent).
                if n + 1 < total {
                    c.parents.push(commit(n + 1).id);
                }
                if n == 5 {
                    // The merge on top brings in the branch that starts from `merge`.
                    c.parents.push(commit(total + 1).id);
                }
                c
            })
            .collect();
        let mut side = commit(total + 1);
        side.parents.push(commit(merge).id);
        history.insert(6, side);

        let mut list = CommitList::default();
        let first = list.reset();
        assert!(list.receive(first, history[..PAGE].to_vec(), true));
        assert_eq!(list.graph().rows.len(), PAGE);
        let second = list.next_page(PAGE).unwrap();
        assert!(list.receive(second, history[PAGE..].to_vec(), false));

        assert_eq!(list.graph(), &layout_graph(&history));
        assert_eq!(list.graph().rows.len(), list.commits().len());
        assert_eq!(list.graph().width, 2);
        // The branch converges into lane 0 on commit `merge` (row `merge + 1`, shifted by the
        // branch commit), in the second page: the line is in the previous row.
        let before = &list.graph().rows[merge];
        assert!(before.segments.iter().any(|s| s.from == 1 && s.to == 0));
    }

    #[test]
    fn pages_of_an_old_epoch_are_dropped() {
        let mut list = CommitList::default();
        let first = list.reset();
        load(&mut list, first, 2_000);
        let second = list.next_page(PAGE).unwrap();
        // Refresh while the second page is on its way.
        let fresh = list.reset();
        let (commits, more) = page(second, 2_000);
        assert!(!list.receive(second, commits, more));
        load(&mut list, fresh, 2_000);
        assert_eq!(list.commits().len(), PAGE);
    }

    #[test]
    fn refresh_keeps_the_selected_commit() {
        let mut list = CommitList::default();
        let first = list.reset();
        load(&mut list, first, 100);
        list.select(5);
        let id = list.selected_commit().unwrap().id;

        // The new log has two more commits on top: the selection follows the commit.
        let request = list.reset();
        let mut commits: Vec<Commit> = (0..100).map(commit).collect();
        commits.insert(0, commit(1000));
        commits.insert(0, commit(1001));
        assert!(list.receive(request, commits, false));
        assert_eq!(list.selected(), Some(7));
        assert_eq!(list.selected_commit().unwrap().id, id);
    }

    #[test]
    fn a_failed_page_can_be_asked_again() {
        let mut list = CommitList::default();
        let first = list.reset();
        load(&mut list, first, 1_000);
        let request = list.next_page(PAGE).unwrap();
        list.failed(request);
        assert_eq!(list.next_page(PAGE), Some(request));
    }

    #[test]
    fn keyboard_moves_stay_inside_the_list() {
        let mut list = CommitList::default();
        list.move_selection(1);
        assert_eq!(list.selected(), None, "empty list");

        let first = list.reset();
        load(&mut list, first, 30);
        list.move_selection(3);
        assert_eq!(list.selected(), Some(3));
        assert_eq!(list.take_scroll_to(), Some(3));
        assert_eq!(list.take_scroll_to(), None);
        list.move_selection(-10);
        assert_eq!(list.selected(), Some(0));
        list.move_selection(100);
        assert_eq!(list.selected(), Some(29));
        list.select_edge(true);
        assert_eq!(list.selected(), Some(0));
        list.select_edge(false);
        assert_eq!(list.selected(), Some(29));

        list.select(500);
        assert_eq!(list.selected(), Some(29), "click outside the list");
        assert!(list.select_id(commit(4).id));
        assert_eq!((list.selected(), list.take_scroll_to()), (Some(4), Some(4)));
        assert!(!list.select_id(commit(999).id));
    }

    #[test]
    fn seek_loads_pages_until_the_commit_arrives() {
        let total = 3_000;
        let mut list = CommitList::default();
        let first = list.reset();
        load(&mut list, first, total);
        assert_eq!(list.seek(commit(3).id), Seek::Selected);
        assert_eq!((list.selected(), list.take_scroll_to()), (Some(3), Some(3)));

        // Commit 1700 is in the fourth page: the pages are requested one after the other, as
        // the UI does when each one arrives.
        let target = commit(1_700).id;
        let Seek::Loading(Some(mut request)) = list.seek(target) else {
            panic!("must ask for the next page")
        };
        let mut pages = 0;
        loop {
            assert_eq!(request.skip, list.commits().len());
            assert_eq!(
                list.next_page(list.commits().len()),
                None,
                "already on its way"
            );
            load(&mut list, request, total);
            pages += 1;
            let id = list.seeking().expect("ricerca in corso");
            match list.seek(id) {
                Seek::Selected => break,
                Seek::Loading(Some(next)) => request = next,
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(pages, 3);
        assert_eq!(list.selected_commit().unwrap().id, target);
        assert_eq!(list.take_scroll_to(), Some(1_700));
        assert_eq!(list.seeking(), None);

        // With a page already on its way that one is awaited; a click on a row stops the search.
        let pending = list.next_page(list.commits().len()).unwrap();
        assert_eq!(list.seek(commit(2_900).id), Seek::Loading(None));
        list.select(0);
        assert_eq!(list.seeking(), None);
        load(&mut list, pending, total);
        // A commit that does not exist: it loads up to the end and then says it is missing.
        let missing = commit(99_999).id;
        while let Seek::Loading(Some(request)) = list.seek(missing) {
            load(&mut list, request, total);
        }
        assert_eq!(list.seek(missing), Seek::Missing);
        assert_eq!(list.commits().len(), total);
    }

    #[test]
    fn seek_stops_at_the_limit() {
        let total = SEEK_LIMIT * 2;
        let mut list = CommitList::default();
        let first = list.reset();
        load(&mut list, first, total);
        let target = commit(SEEK_LIMIT + 10).id;
        while let Seek::Loading(Some(request)) = list.seek(target) {
            load(&mut list, request, total);
        }
        assert_eq!(list.commits().len(), SEEK_LIMIT);
        assert_eq!(list.seek(target), Seek::Missing);
        assert_eq!(list.seeking(), None);
        // A refresh stops a search in progress.
        let mut list = CommitList::default();
        let first = list.reset();
        load(&mut list, first, total);
        assert!(matches!(list.seek(target), Seek::Loading(Some(_))));
        list.reset();
        assert_eq!(list.seeking(), None);
    }

    mod sample {
        use super::super::*;
        use project_domain::RefInfo;
        use project_engine::{
            GitBackend, LogQuery, MemoryBackend, MemoryOpener, Registry, Session,
        };
        use std::sync::Arc;

        fn open(repo: MemoryBackend) -> (Registry, Session) {
            let mut opener = MemoryOpener::new();
            opener.insert("r", repo);
            let registry = Registry::builtin();
            let mut session = Session::new(Arc::new(opener));
            registry
                .run("repo.open", &mut session, json!({ "path": "r" }))
                .unwrap();
            (registry, session)
        }

        /// The first page of the log as the UI requests it.
        fn load(registry: &Registry, session: &mut Session) -> CommitList {
            let mut list = CommitList::default();
            let request = list.reset();
            let page = registry.run("repo.log", session, request.params()).unwrap();
            let commits: Vec<Commit> = serde_json::from_value(page["commits"].clone()).unwrap();
            let more = page["has_more"].as_bool().unwrap();
            assert!(list.receive(request, commits, more));
            list
        }

        fn target(registry: &Registry, session: &mut Session, name: &str) -> Oid {
            let refs: Vec<RefInfo> =
                serde_json::from_value(registry.run("repo.refs", session, json!({})).unwrap())
                    .unwrap();
            refs.iter().find(|r| r.name == name).unwrap().target
        }

        fn oid(n: u64) -> Oid {
            MemoryBackend::fake_oid(n)
        }

        #[test]
        fn after_a_fetch_the_remote_branch_is_in_the_graph() {
            let (registry, mut session) = open(MemoryBackend::sample());
            let list = load(&registry, &mut session);
            assert_eq!(list.commits().len(), 12);
            assert_eq!(list.commits()[0].id, oid(12), "HEAD on top");
            let head = list.anchor(oid(12)).unwrap();
            assert_eq!((head.index, head.column), (0, 0));

            registry
                .run("remote.fetch", &mut session, json!({}))
                .unwrap();
            let mut list = load(&registry, &mut session);
            assert_eq!(list.commits().len(), 13);
            // Commit 13 of `origin/feature/graph` is the most recent: it comes before HEAD.
            assert_eq!(list.commits()[0].id, oid(13));
            let graph_tip = target(&registry, &mut session, "refs/remotes/origin/feature/graph");
            assert_eq!(graph_tip, oid(13));
            assert_eq!(list.seek(graph_tip), Seek::Selected);
            assert_eq!(list.selected_commit().unwrap().id, oid(13));

            // HEAD is in row 1, in a free column of its own above it: the changes row
            // attaches there, not to commit 13.
            let head = list.anchor(oid(12)).unwrap();
            assert_eq!(head.index, 1);
            assert_ne!(head.column, list.graph().rows[0].column);
            assert!(head.free_above);
            assert_eq!(list.graph().width, 2);
        }

        #[test]
        fn an_unmerged_local_branch_is_in_the_graph() {
            let mut repo = MemoryBackend::sample();
            let base = repo.log(&LogQuery::page(4, 1)).unwrap().remove(0);
            assert_eq!(base.id, oid(8));
            let extra = Commit {
                id: Oid::from_bytes([0xee; 20]),
                parents: vec![base.id],
                summary: "Unmerged test".to_owned(),
                ..base
            };
            repo.set_changes(extra.id, Vec::new());
            repo.set_branch("experiment", extra.id);
            repo.insert_commit(extra.clone());
            let (registry, mut session) = open(repo);
            let mut list = load(&registry, &mut session);
            assert_eq!(list.commits().len(), 13);
            assert!(list.commits().iter().any(|c| c.id == extra.id));
            let branch = target(&registry, &mut session, "refs/heads/experiment");
            assert_eq!(list.seek(branch), Seek::Selected);
            assert_eq!(list.selected_commit().unwrap().summary, "Unmerged test");
            // The branch commit is older than HEAD (same date as the parent): HEAD stays
            // on top, with the changes row attached.
            let head = list.anchor(oid(12)).unwrap();
            assert_eq!(head.index, 0);
            assert!(head.free_above);
        }
    }
}
