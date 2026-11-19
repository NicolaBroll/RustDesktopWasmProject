//! The message of the status bar: one line on the bar, the whole text on hover (with the names
//! of the backup refs, which the line leaves out), and drawn weak once it is old.

use project_i18n::tr;
use std::borrow::Cow;
use std::time::{Duration, Instant};

/// How long a message that is not an error is drawn in full colour.
pub(crate) const FRESH: Duration = Duration::from_secs(10);

/// The informational message of a tab and when it was set. It derefs to its text.
#[derive(Debug, Default)]
pub(crate) struct Notice {
    text: String,
    since: Option<Instant>,
}

impl Notice {
    /// The new message, fresh from now. An empty `text` is no message.
    pub fn set(&mut self, text: String) {
        self.set_at(text, Instant::now());
    }

    /// [`Notice::set`] at the time `now`.
    pub fn set_at(&mut self, text: String, now: Instant) {
        self.since = (!text.is_empty()).then_some(now);
        self.text = text;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.since = None;
    }

    /// How long the message is still to be drawn in full colour at `now`: `None` once it is old
    /// (drawn weak) or when there is none.
    pub fn fresh_for(&self, now: Instant) -> Option<Duration> {
        let age = now.saturating_duration_since(self.since?);
        FRESH.checked_sub(age).filter(|left| !left.is_zero())
    }
}

impl std::ops::Deref for Notice {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Display for Notice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl PartialEq<&str> for Notice {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

impl PartialEq<String> for Notice {
    fn eq(&self, other: &String) -> bool {
        self.text == *other
    }
}

/// What the bar draws of `text`: its first line. The lines after it (the backup refs added by
/// [`with_backups`]) are only in the hover text, which is the whole `text`.
pub(crate) fn line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

/// An error on one line: git may answer with several, and all of them are the message.
pub(crate) fn one_line(text: &str) -> Cow<'_, str> {
    if !text.contains(['\n', '\r']) {
        return Cow::Borrowed(text);
    }
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    Cow::Owned(lines.join(" "))
}

/// `text` followed, on a line of its own, by the names of the backup `refs`: the message says
/// only that a backup was saved, the names are read on hover. Called last, after everything else
/// was appended to `text`.
pub(crate) fn with_backups<S: AsRef<str>>(mut text: String, refs: &[S]) -> String {
    let names: Vec<&str> = refs
        .iter()
        .map(AsRef::as_ref)
        .filter(|name| !name.is_empty())
        .collect();
    if !names.is_empty() {
        text.push('\n');
        text.push_str(&tr!(
            "status-backup",
            n = names.len(),
            refs = names.join(", ")
        ));
    }
    text
}

/// [`with_backups`] for the backup a command may have saved.
pub(crate) fn with_backup(text: String, backup: Option<&str>) -> String {
    with_backups(text, backup.as_slice())
}

/// For the tests of the messages: `name` (a backup ref, or a part of one) is in the lines of
/// `text` for the hover and not in the line the bar draws.
#[cfg(test)]
pub(crate) fn hides(text: &str, name: &str) -> bool {
    let line = line(text);
    !line.contains(name) && text[line.len()..].contains(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_is_fresh_for_ten_seconds() {
        let start = Instant::now();
        let mut notice = Notice::default();
        assert_eq!(notice.fresh_for(start), None, "no message");
        notice.set_at("Fetch done".to_owned(), start);
        assert_eq!(&*notice, "Fetch done");
        assert_eq!(notice.fresh_for(start), Some(FRESH));
        let later = start + Duration::from_secs(4);
        assert_eq!(notice.fresh_for(later), Some(Duration::from_secs(6)));
        assert_eq!(notice.fresh_for(start + FRESH), None, "old: drawn weak");
        assert_eq!(notice.fresh_for(start + Duration::from_secs(3600)), None);
        // The text stays.
        assert_eq!(&*notice, "Fetch done");
    }

    #[test]
    fn the_same_message_again_is_fresh_again() {
        let start = Instant::now();
        let mut notice = Notice::default();
        notice.set_at("A backup of the index was saved".to_owned(), start);
        let later = start + Duration::from_secs(60);
        assert_eq!(notice.fresh_for(later), None);
        notice.set_at("A backup of the index was saved".to_owned(), later);
        assert_eq!(notice.fresh_for(later), Some(FRESH));
    }

    #[test]
    fn a_cleared_or_empty_notice_has_no_age() {
        let start = Instant::now();
        let mut notice = Notice::default();
        notice.set_at("Done".to_owned(), start);
        notice.clear();
        assert!(notice.is_empty());
        assert_eq!(notice.fresh_for(start), None);
        notice.set_at(String::new(), start);
        assert_eq!(notice.fresh_for(start), None);
        // A clock that went back does not make it old.
        notice.set_at("Done".to_owned(), start + Duration::from_secs(5));
        assert_eq!(notice.fresh_for(start), Some(FRESH));
    }

    #[test]
    fn the_backup_refs_are_only_in_the_hover_text() {
        let text = with_backup(
            "Undone: Amend 83c3266; a backup was saved".to_owned(),
            Some("refs/project/backup/undo-1"),
        );
        assert_eq!(line(&text), "Undone: Amend 83c3266; a backup was saved");
        assert_eq!(
            text,
            "Undone: Amend 83c3266; a backup was saved\nBackup: refs/project/backup/undo-1"
        );
        assert!(hides(&text, "refs/project/backup/undo-1"));
        assert!(!hides("backup in refs/project/backup/undo-1", "undo-1"));
        assert!(!hides("Done", "undo-1"));
        let two = with_backups("Reset done".to_owned(), &["refs/a", "refs/b"]);
        assert_eq!(two, "Reset done\nBackups: refs/a, refs/b");
    }

    #[test]
    fn a_message_without_backups_is_its_own_line() {
        let none: [&str; 0] = [];
        assert_eq!(with_backups("Done".to_owned(), &none), "Done");
        assert_eq!(with_backup("Done".to_owned(), None), "Done");
        assert_eq!(with_backups("Done".to_owned(), &[""]), "Done");
        assert_eq!(line("Done"), "Done");
        assert_eq!(line(""), "");
    }

    #[test]
    fn an_error_in_several_lines_is_one_line() {
        assert_eq!(one_line("Fetch failed"), "Fetch failed");
        assert_eq!(
            one_line("Push failed: remote: denied\r\n\n  hint: ask the owner\n"),
            "Push failed: remote: denied hint: ask the owner"
        );
    }
}
