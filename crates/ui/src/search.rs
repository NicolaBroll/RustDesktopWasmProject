//! Search in the history (S48): the field above the commit list (Ctrl+F), the matches of
//! `repo.search` highlighted in the graph (it is not filtered), "next"/"previous" that select the
//! match with [`CommitList::seek`](crate::list::CommitList::seek). Data only, no egui: tested
//! with unit tests. Contract in `impl-docs/git/reading.md`, `repo.search`.
//!
//! - **Typing** is debounced ([`DEBOUNCE`]): the query is sent once the text stays the same for
//!   that long (Enter sends it at once). An empty text or one with a newline/NUL is never sent.
//! - **Requests**: `repo.search` is a slow read (§6) and runs in the background. Every request
//!   has a number; an answer whose number is not the one in flight (the query changed, the search
//!   was closed) is discarded and the replaced request is cancelled
//!   ([`SearchBox::take_cancel`]). A different repo is a different generation of the tab: its
//!   answers never reach this box.
//! - **Pages** of [`PAGE`] ids, in the order of the list's `repo.log` (same `all`/`stashes`):
//!   "next" past the last loaded id asks for the following page with `skip` and moves when it
//!   arrives; scrolling the list past the last loaded match asks for it too, so the matches
//!   below are highlighted.
//! - **Ends**: "next" on the last match and "previous" on the first do nothing (no wrap: going
//!   around would load every page of ids and of commits).
//! - When the log is read again (refs changed) the same query runs again: the highlight stays
//!   until the new answer arrives, the current match is kept by id.

use crate::{Action, Done, RepoView, Request, Tab, Ticket};
use project_domain::Oid;
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;

/// The id of the command.
pub const COMMAND: &str = "repo.search";
/// Ids asked for per page.
pub const PAGE: usize = 200;
/// Seconds the text must stay unchanged before it is searched.
pub const DEBOUNCE: f64 = 0.3;

/// True if `text` can be sent to `repo.search` (otherwise `InvalidParams`).
pub fn valid(text: &str) -> bool {
    !text.is_empty() && !text.contains(['\n', '\r', '\0'])
}

/// A request to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Want {
    pub params: Value,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Job {
    seq: u64,
    ticket: Option<Ticket>,
    query: String,
    skip: usize,
}

/// The matches of a query read so far.
#[derive(Debug, Default)]
struct Found {
    query: String,
    ids: Vec<Oid>,
    set: HashSet<Oid>,
    total: usize,
    has_more: bool,
    /// Index in `ids` of the match selected with next/previous.
    current: Option<usize>,
    error: Option<String>,
}

/// What the bar shows next to the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State<'a> {
    /// Nothing to search (empty field).
    Idle,
    /// The first answer of the query is on its way (or the text is still being typed).
    Searching,
    NoMatches,
    /// `current` is the 0-based index of the selected match, if any.
    Matches {
        current: Option<usize>,
        total: usize,
    },
    Failed(&'a str),
}

#[derive(Deserialize)]
struct Page {
    ids: Vec<Oid>,
    total: usize,
    has_more: bool,
}

#[derive(Debug, Default)]
pub struct SearchBox {
    open: bool,
    text: String,
    /// The field takes the keyboard on the next frame.
    focus: bool,
    /// When the text typed is to be searched.
    due: Option<f64>,
    /// This query runs again (log read again).
    rerun: Option<String>,
    /// The next page of ids is asked for.
    more: bool,
    /// When the asked page arrives, "next" moves onto its first id.
    advance: bool,
    next: u64,
    job: Option<Job>,
    found: Option<Found>,
    cancel: Vec<Ticket>,
}

impl SearchBox {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Ctrl+F: opens the bar and gives the field the keyboard.
    pub fn open(&mut self) {
        self.open = true;
        self.focus = true;
    }

    /// True once after [`SearchBox::open`]: the view focuses the field.
    pub fn take_focus(&mut self) -> bool {
        std::mem::take(&mut self.focus)
    }

    /// Escape: closes the bar, empties the field, removes the highlight and cancels the request
    /// in flight.
    pub fn close(&mut self) {
        self.forget();
        *self = Self {
            next: self.next,
            cancel: std::mem::take(&mut self.cancel),
            ..Self::default()
        };
    }

    /// The text typed at time `now` (seconds): searched after [`DEBOUNCE`]. A text that cannot be
    /// searched removes the previous matches at once.
    pub fn edit(&mut self, text: String, now: f64) {
        if text == self.text {
            return;
        }
        self.text = text;
        if valid(&self.text) {
            self.due = Some(now + DEBOUNCE);
        } else {
            self.due = None;
            self.reset();
        }
    }

    /// The log was read again: the query searched so far runs again.
    pub fn refresh(&mut self) {
        let query = self.found.as_ref().map(|f| &f.query);
        self.rerun = query.or(self.job.as_ref().map(|j| &j.query)).cloned();
    }

    /// Seconds to wait before the typed text is due, to repaint then.
    pub fn wait(&self, now: f64) -> Option<f64> {
        self.due.map(|due| (due - now).max(0.0))
    }

    /// The request to send at time `now`, if any. Called on every frame.
    pub fn want(&mut self, now: f64) -> Option<Want> {
        let (query, skip) = if self.due.is_some_and(|due| due <= now) {
            self.due = None;
            self.reset();
            (self.text.clone(), 0)
        } else if self.due.is_none()
            && let Some(query) = self.rerun.take()
        {
            self.more = false;
            self.advance = false;
            self.forget();
            (query, 0)
        } else if self.more && self.job.is_none() {
            self.more = false;
            let found = self.found.as_ref()?;
            (found.query.clone(), found.ids.len())
        } else {
            return None;
        };
        let seq = self.next;
        self.next += 1;
        let params = json!({
            "query": query,
            "all": true,
            "stashes": true,
            "skip": skip,
            "limit": PAGE,
        });
        self.job = Some(Job {
            seq,
            ticket: None,
            query,
            skip,
        });
        Some(Want { params, seq })
    }

    /// Notes the ticket of request `seq`, to cancel it if it is replaced.
    pub fn sent(&mut self, seq: u64, ticket: Ticket) {
        if let Some(job) = &mut self.job
            && job.seq == seq
        {
            job.ticket = Some(ticket);
        }
    }

    /// The tickets of the requests replaced or closed since the last call.
    pub fn take_cancel(&mut self) -> Vec<Ticket> {
        std::mem::take(&mut self.cancel)
    }

    /// The answer to request `seq`. Returns the commit to select: the first match of a new
    /// query, or the match "next" was waiting for. A stale answer is discarded.
    pub fn receive(&mut self, seq: u64, result: Result<Value, String>) -> Option<Oid> {
        let job = self.job.take_if(|j| j.seq == seq)?;
        let page = result.and_then(|value| {
            serde_json::from_value::<Page>(value).map_err(|e| {
                tr!(
                    "common-unexpected-result",
                    command = COMMAND,
                    error = e.to_string()
                )
            })
        });
        if job.skip == 0 {
            let previous = self.found.take();
            let mut found = Found {
                query: job.query,
                ..Found::default()
            };
            let mut select = None;
            match page {
                Ok(page) => {
                    found.set = page.ids.iter().copied().collect();
                    found.ids = page.ids;
                    found.total = page.total;
                    found.has_more = page.has_more;
                    match previous.filter(|p| p.query == found.query) {
                        // The same query again (log read again): the selection does not move.
                        Some(previous) => {
                            let id = previous.current.and_then(|i| previous.ids.get(i));
                            found.current =
                                id.and_then(|id| found.ids.iter().position(|x| x == id));
                        }
                        None => {
                            select = found.ids.first().copied();
                            found.current = select.map(|_| 0);
                        }
                    }
                }
                Err(e) => found.error = Some(e),
            }
            self.found = Some(found);
            return select;
        }
        let found = self.found.as_mut().filter(|f| f.query == job.query)?;
        let advance = std::mem::take(&mut self.advance);
        match page {
            Ok(page) => {
                found.ids.truncate(job.skip);
                found.set.extend(page.ids.iter().copied());
                found.ids.extend(page.ids);
                found.total = page.total;
                found.has_more = page.has_more;
                found.error = None;
                if advance {
                    let index = found.current.map_or(0, |c| c + 1);
                    if let Some(&id) = found.ids.get(index) {
                        found.current = Some(index);
                        return Some(id);
                    }
                }
            }
            Err(e) => found.error = Some(e),
        }
        None
    }

    /// "Next" (Enter): the match after the selected one. Past the loaded ids it asks for the next
    /// page and moves when it arrives (`None` now). With the text still being typed it searches
    /// at once.
    pub fn next(&mut self) -> Option<Oid> {
        if self.due.is_some() {
            self.due = Some(f64::NEG_INFINITY);
            return None;
        }
        let found = self.found.as_mut()?;
        let index = found.current.map_or(0, |c| c + 1);
        if let Some(&id) = found.ids.get(index) {
            found.current = Some(index);
            return Some(id);
        }
        if found.has_more && self.job.is_none() {
            self.more = true;
            self.advance = true;
        }
        None
    }

    /// "Previous" (Shift+Enter): the match before the selected one; on the first it stays.
    pub fn previous(&mut self) -> Option<Oid> {
        if self.due.is_some() {
            self.due = Some(f64::NEG_INFINITY);
            return None;
        }
        let found = self.found.as_mut()?;
        let index = found.current?.checked_sub(1)?;
        found.current = Some(index);
        found.ids.get(index).copied()
    }

    pub fn can_next(&self) -> bool {
        self.found.as_ref().is_some_and(|f| {
            let next = f.current.map_or(0, |c| c + 1);
            next < f.ids.len() || f.has_more
        })
    }

    pub fn can_previous(&self) -> bool {
        self.found
            .as_ref()
            .is_some_and(|f| f.current.is_some_and(|c| c > 0))
    }

    /// True if commit `id` matches the query: its row is highlighted.
    pub fn is_match(&self, id: Oid) -> bool {
        self.found.as_ref().is_some_and(|f| f.set.contains(&id))
    }

    /// The last match loaded, if more follow and none is being asked for: when the list shows
    /// its row the next page is asked for ([`SearchBox::load_more`]).
    pub fn last_if_more(&self) -> Option<Oid> {
        let found = self.found.as_ref()?;
        if !found.has_more || self.job.is_some() || self.more || found.error.is_some() {
            return None;
        }
        found.ids.last().copied()
    }

    /// Asks for the next page of ids (the list scrolled past the last loaded match).
    pub fn load_more(&mut self) {
        if self.found.as_ref().is_some_and(|f| f.has_more) {
            self.more = true;
        }
    }

    pub fn state(&self) -> State<'_> {
        if self.due.is_some() {
            return State::Searching;
        }
        match &self.found {
            None if valid(&self.text) => State::Searching,
            None => State::Idle,
            Some(Found {
                error: Some(e),
                ids,
                ..
            }) if ids.is_empty() => State::Failed(e),
            Some(f) if f.total == 0 => State::NoMatches,
            Some(f) => State::Matches {
                current: f.current,
                total: f.total,
            },
        }
    }

    /// Drops the matches and the request in flight.
    fn reset(&mut self) {
        self.forget();
        self.found = None;
        self.rerun = None;
        self.more = false;
        self.advance = false;
    }

    /// Cancels the request in flight: its answer will be discarded.
    fn forget(&mut self) {
        if let Some(ticket) = self.job.take().and_then(|j| j.ticket) {
            self.cancel.push(ticket);
        }
    }
}

impl RepoView {
    /// The list shows rows up to `end` (excluded): if the last loaded match is among them, the
    /// next page of ids is asked for, so the matches below are highlighted too.
    pub(crate) fn search_visible(&mut self, end: usize) {
        if let Some(last) = self.search.last_if_more()
            && self.list.index_of(last).is_some_and(|index| index < end)
        {
            self.search.load_more();
        }
    }
}

impl Tab {
    pub(crate) fn apply_search(&mut self, action: Action) {
        let Some(repo) = &mut self.repo else {
            return;
        };
        let select = match action {
            Action::SearchOpen => {
                repo.search.open();
                None
            }
            Action::SearchText(text, now) => {
                repo.search.edit(text, now);
                None
            }
            Action::SearchNext => repo.search.next(),
            Action::SearchPrevious => repo.search.previous(),
            Action::SearchClose => {
                repo.search.close();
                None
            }
            _ => unreachable!("action dispatched to the wrong function"),
        };
        if let Some(id) = select {
            self.select_match(id);
        }
    }

    /// Selects a match and brings it into view, loading the pages of the log up to it.
    fn select_match(&mut self, id: Oid) {
        if let Some(repo) = &mut self.repo {
            repo.stash.select(None);
        }
        self.seek(id);
    }

    /// Cancels the replaced request and sends the one due, if any; repaints when the typed text
    /// is due.
    pub(crate) fn request_search(&mut self, ctx: &egui::Context) {
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        let now = ctx.input(|i| i.time);
        let cancel = repo.search.take_cancel();
        let want = repo.search.want(now);
        if let Some(wait) = repo.search.wait(now) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
        for ticket in cancel {
            self.executor.cancel(ticket);
        }
        if let Some(want) = want {
            let ticket = self.executor.submit(COMMAND, want.params);
            self.pending
                .insert(ticket, Request::Search(generation, want.seq));
            if let Some(repo) = &mut self.repo {
                repo.search.sent(want.seq, ticket);
            }
        }
    }

    pub(crate) fn receive_search(&mut self, generation: u64, seq: u64, done: Done) {
        if let Err(e) = &done.result {
            log::warn!("{}: {e}", done.command);
        }
        let select = self
            .current(generation)
            .and_then(|repo| repo.search.receive(seq, done.result));
        if let Some(id) = select {
            self.select_match(id);
        }
    }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
