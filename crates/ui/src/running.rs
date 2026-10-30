//! The command a dialog or a box is waiting for: its ticket (for progress and "Cancel"), whether
//! "Cancel" was already asked, and what the dialog needs to remember about it (`extra`). Held as
//! `Option<Running<..>>`: `None` = nothing in flight, so "sending without a ticket" cannot happen.

use crate::Ticket;

/// A command in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Running<T = ()> {
    pub ticket: Ticket,
    /// True after "Cancel"/"Stop", until the command answers.
    pub cancelling: bool,
    /// What the owner remembers about the command (e.g. the requested remote).
    pub extra: T,
}

impl Running {
    /// The command started with `ticket`, nothing else to remember.
    pub fn new(ticket: Ticket) -> Self {
        Running::with(ticket, ())
    }
}

impl<T> Running<T> {
    /// The command started with `ticket`, with `extra`.
    pub fn with(ticket: Ticket, extra: T) -> Self {
        Running {
            ticket,
            cancelling: false,
            extra,
        }
    }

    /// "Cancel": the ticket to cancel, only the first time.
    pub fn cancel(&mut self) -> Option<Ticket> {
        if self.cancelling {
            return None;
        }
        self.cancelling = true;
        Some(self.ticket)
    }
}
