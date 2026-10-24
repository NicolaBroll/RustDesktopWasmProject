//! State of the details panel: which commit to ask `commit.show` for and which answer to
//! show. Data only, no egui: tested with unit tests.
//!
//! Rules:
//! - a commit is requested only when the selection changes, not on every frame;
//! - at most one request in flight: if the selection changes while waiting (arrow keys held
//!   down), on the answer only the last selected commit is requested, not all those
//!   traversed;
//! - an answer for a commit that is no longer the selected one is discarded.

use project_domain::{CommitDetails, Oid};

/// What the panel knows about the selected commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown<'a> {
    /// No commit selected.
    Nothing,
    /// Request in flight (or waiting for the one of a previous commit to finish).
    Loading(Oid),
    Loaded(&'a CommitDetails),
    Failed(Oid, &'a str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    Loaded(Box<CommitDetails>),
    Failed(String),
}

#[derive(Debug, Default)]
pub struct DetailsState {
    /// The commit selected in the list.
    target: Option<Oid>,
    /// The commit requested and not yet arrived.
    in_flight: Option<Oid>,
    /// The last answer that arrived for `target`, with its id.
    answer: Option<(Oid, Answer)>,
}

impl DetailsState {
    /// Notes the selected commit and returns the id to ask `commit.show` for, if needed:
    /// the answer is not already there and there is no other request in flight. Called on every frame.
    pub fn want(&mut self, selected: Option<Oid>) -> Option<Oid> {
        self.target = selected;
        let id = selected?;
        let answered = self.answer.as_ref().is_some_and(|(a, _)| *a == id);
        if answered || self.in_flight.is_some() {
            return None;
        }
        self.in_flight = Some(id);
        Some(id)
    }

    /// The answer of `commit.show` for `id`. It is kept only if `id` is still the
    /// selected commit: returns `false` if it was discarded.
    pub fn receive(&mut self, id: Oid, result: Result<CommitDetails, String>) -> bool {
        if self.in_flight == Some(id) {
            self.in_flight = None;
        }
        if self.target != Some(id) {
            return false;
        }
        let answer = match result {
            Ok(details) => Answer::Loaded(Box::new(details)),
            Err(e) => Answer::Failed(e),
        };
        self.answer = Some((id, answer));
        true
    }

    /// Forgets the error of the selected commit: at the next [`Self::want`] it is requested again.
    pub fn retry(&mut self) {
        if matches!(self.answer, Some((_, Answer::Failed(_)))) {
            self.answer = None;
        }
    }

    /// What to show for the selected commit.
    pub fn shown(&self) -> Shown<'_> {
        let Some(id) = self.target else {
            return Shown::Nothing;
        };
        match &self.answer {
            Some((a, Answer::Loaded(details))) if *a == id => Shown::Loaded(details),
            Some((a, Answer::Failed(e))) if *a == id => Shown::Failed(id, e),
            _ => Shown::Loading(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Commit, Signature};

    fn oid(n: u8) -> Oid {
        Oid::from_bytes([n; 20])
    }

    fn details(n: u8) -> CommitDetails {
        let sig = Signature {
            name: "Ada".to_owned(),
            email: "ada@example.com".to_owned(),
            time: 0,
            offset_min: 0,
        };
        CommitDetails {
            commit: Commit {
                id: oid(n),
                parents: Vec::new(),
                author: sig.clone(),
                committer: sig,
                summary: format!("commit {n}"),
                body: String::new(),
            },
            compared_to: None,
            files: Vec::new(),
        }
    }

    #[test]
    fn asks_once_per_selection() {
        let mut s = DetailsState::default();
        assert_eq!(s.want(None), None);
        assert_eq!(s.shown(), Shown::Nothing);
        assert_eq!(s.want(Some(oid(1))), Some(oid(1)));
        // The following frames do not request again.
        assert_eq!(s.want(Some(oid(1))), None);
        assert_eq!(s.shown(), Shown::Loading(oid(1)));
        assert!(s.receive(oid(1), Ok(details(1))));
        assert_eq!(s.want(Some(oid(1))), None);
        assert_eq!(s.shown(), Shown::Loaded(&details(1)));
    }

    #[test]
    fn a_late_answer_for_an_old_selection_is_dropped() {
        let mut s = DetailsState::default();
        assert_eq!(s.want(Some(oid(1))), Some(oid(1)));
        // The selection goes to 2 and then to 3 while 1 is in flight: no new request.
        assert_eq!(s.want(Some(oid(2))), None);
        assert_eq!(s.want(Some(oid(3))), None);
        assert!(!s.receive(oid(1), Ok(details(1))));
        assert_eq!(s.shown(), Shown::Loading(oid(3)));
        // When the request for 1 finishes only the last selected is requested.
        assert_eq!(s.want(Some(oid(3))), Some(oid(3)));
        assert!(s.receive(oid(3), Ok(details(3))));
        assert_eq!(s.shown(), Shown::Loaded(&details(3)));
    }

    #[test]
    fn going_back_to_the_loaded_commit_needs_no_request() {
        let mut s = DetailsState::default();
        s.want(Some(oid(1)));
        s.receive(oid(1), Ok(details(1)));
        assert_eq!(s.want(Some(oid(2))), Some(oid(2)));
        // Goes back to 1 before 2 arrives: 1 is already there.
        assert_eq!(s.want(Some(oid(1))), None);
        assert_eq!(s.shown(), Shown::Loaded(&details(1)));
        assert!(!s.receive(oid(2), Ok(details(2))));
        assert_eq!(s.want(Some(oid(1))), None);
    }

    #[test]
    fn errors_are_shown_and_can_be_retried() {
        let mut s = DetailsState::default();
        s.want(Some(oid(1)));
        assert!(s.receive(oid(1), Err("unknown commit".to_owned())));
        assert_eq!(s.shown(), Shown::Failed(oid(1), "unknown commit"));
        // No repeated requests on every frame after an error.
        assert_eq!(s.want(Some(oid(1))), None);
        s.retry();
        assert_eq!(s.shown(), Shown::Loading(oid(1)));
        assert_eq!(s.want(Some(oid(1))), Some(oid(1)));
    }

    #[test]
    fn deselecting_drops_the_answer_in_flight() {
        let mut s = DetailsState::default();
        s.want(Some(oid(1)));
        s.want(None);
        assert!(!s.receive(oid(1), Ok(details(1))));
        assert_eq!(s.shown(), Shown::Nothing);
    }
}
