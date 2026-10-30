//! Engine texts (errors, warnings, journal entries) as key + parameters, translated with
//! `project-i18n`.
//!
//! A [`Message`] is not an already composed text: the UI shows it in the active language
//! ([`Message::text`]), CLI, MCP, logs and tests in English (`Display`). It is always created with the
//! [`msg!`](crate::msg) macro and the literal key (a test checks that every key used exists
//! in both languages); the keys live in `crates/i18n/locales/{it,en}/engine*.ftl`.
//!
//! In JSON (command results) a `Message` is `{"text": <English>, "key", "args"}`: CLI and
//! MCP read `text`, the UI re-reads key and parameters and shows it in its language. It is also read
//! back from a plain string (an already composed text, which stays as it is).

use project_i18n::{Arg, Lang};
use serde::de::Deserializer;
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

/// An engine text: Fluent key and parameters (sorted by name).
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    key: Cow<'static, str>,
    args: Vec<(Cow<'static, str>, MessageArg)>,
}

impl Eq for Message {}

/// A parameter of a [`Message`]: text (names, paths, git's text), number (chooses the
/// plural) or another message (a reason composed of other texts), translated into the same
/// language.
#[derive(Debug, Clone, PartialEq)]
pub enum MessageArg {
    Text(String),
    Number(f64),
    Message(Box<Message>),
    /// Several messages joined by the separator (e.g. one line per ref, with `"; "`).
    Joined(Vec<Message>, Cow<'static, str>),
}

impl Message {
    /// Use [`msg!`](crate::msg), which keeps the key literal.
    #[doc(hidden)]
    pub fn new(key: &'static str, args: Vec<(&'static str, MessageArg)>) -> Self {
        Self::build(
            Cow::Borrowed(key),
            args.into_iter()
                .map(|(n, v)| (Cow::Borrowed(n), v))
                .collect(),
        )
    }

    fn build(key: Cow<'static, str>, mut args: Vec<(Cow<'static, str>, MessageArg)>) -> Self {
        args.sort_by(|a, b| a.0.cmp(&b.0));
        Message { key, args }
    }

    /// An already composed text, which stays as it is in every language (e.g. read from an old JSON).
    pub fn plain(text: impl Into<String>) -> Self {
        Message::new(
            "engine-other",
            vec![("text", MessageArg::Text(text.into()))],
        )
    }

    /// The Fluent key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The parameters, sorted by name.
    pub fn args(&self) -> &[(Cow<'static, str>, MessageArg)] {
        &self.args
    }

    /// The text in `lang`.
    pub fn text_in(&self, lang: Lang) -> String {
        let i18n = project_i18n::get(lang);
        if self.args.is_empty() {
            return i18n.tr(&self.key);
        }
        let args = self.args.iter().map(|(name, value)| {
            let value = match value {
                MessageArg::Text(text) => Arg::from(text.as_str()),
                MessageArg::Number(n) => Arg::from(*n),
                MessageArg::Message(m) => Arg::from(m.text_in(lang)),
                MessageArg::Joined(list, sep) => Arg::from(
                    list.iter()
                        .map(|m| m.text_in(lang))
                        .collect::<Vec<_>>()
                        .join(sep),
                ),
            };
            (name.as_ref(), value)
        });
        i18n.tr_args(&self.key, args)
    }

    /// The text in the UI's active language.
    pub fn text(&self) -> String {
        self.text_in(project_i18n::lang())
    }

    /// True if the English text contains `pattern` (for tests).
    pub fn contains(&self, pattern: &str) -> bool {
        self.to_string().contains(pattern)
    }

    /// True if the English text starts with `pattern` (for tests).
    pub fn starts_with(&self, pattern: &str) -> bool {
        self.to_string().starts_with(pattern)
    }
}

/// The English text, the one used by CLI, MCP, logs and tests.
impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text_in(Lang::En))
    }
}

impl Serialize for Message {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("text", &self.to_string())?;
        map.serialize_entry("key", self.key())?;
        if !self.args.is_empty() {
            let args: BTreeMap<&str, &MessageArg> =
                self.args.iter().map(|(n, v)| (n.as_ref(), v)).collect();
            map.serialize_entry("args", &args)?;
        }
        map.end()
    }
}

impl Serialize for MessageArg {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            MessageArg::Text(text) => serializer.serialize_str(text),
            MessageArg::Number(n) => serializer.serialize_f64(*n),
            MessageArg::Message(m) => m.serialize(serializer),
            MessageArg::Joined(list, separator) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("messages", list)?;
                map.serialize_entry("separator", separator)?;
                map.end()
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MessageRepr {
    Plain(String),
    Full {
        key: String,
        #[serde(default)]
        args: BTreeMap<String, MessageArg>,
    },
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match MessageRepr::deserialize(deserializer)? {
            MessageRepr::Plain(text) => Message::plain(text),
            MessageRepr::Full { key, args } => Message::build(
                Cow::Owned(key),
                args.into_iter().map(|(n, v)| (Cow::Owned(n), v)).collect(),
            ),
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ArgRepr {
    Text(String),
    Number(f64),
    Joined {
        messages: Vec<Message>,
        separator: String,
    },
    Message(Box<Message>),
}

impl<'de> Deserialize<'de> for MessageArg {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match ArgRepr::deserialize(deserializer)? {
            ArgRepr::Text(text) => MessageArg::Text(text),
            ArgRepr::Number(n) => MessageArg::Number(n),
            ArgRepr::Joined {
                messages,
                separator,
            } => MessageArg::Joined(messages, Cow::Owned(separator)),
            ArgRepr::Message(m) => MessageArg::Message(m),
        })
    }
}

/// A value that becomes a [`MessageArg`]: the [`msg!`](crate::msg) macro takes the parameters
/// by reference, so it does not move them.
pub trait ToArg {
    fn to_arg(&self) -> MessageArg;
}

impl<T: ToArg + ?Sized> ToArg for &T {
    fn to_arg(&self) -> MessageArg {
        (**self).to_arg()
    }
}

impl ToArg for MessageArg {
    fn to_arg(&self) -> MessageArg {
        self.clone()
    }
}

impl ToArg for Message {
    fn to_arg(&self) -> MessageArg {
        MessageArg::Message(Box::new(self.clone()))
    }
}

/// Types that go into the text as `Display` writes them (names, paths, ids, git's text).
macro_rules! display_arg {
    ($($t:ty),*) => {$(
        impl ToArg for $t {
            fn to_arg(&self) -> MessageArg {
                MessageArg::Text(self.to_string())
            }
        }
    )*};
}
display_arg!(
    str,
    String,
    char,
    std::io::Error,
    crate::PullOperation,
    project_domain::Oid,
    project_domain::RepoPath
);

/// Like `display_arg!`, for types with a lifetime: a name written with `escape_debug`
/// (visible control characters), a `from_utf8_lossy` text, a disk path.
macro_rules! display_arg_borrowed {
    ($($t:ty),*) => {$(
        impl ToArg for $t {
            fn to_arg(&self) -> MessageArg {
                MessageArg::Text(self.to_string())
            }
        }
    )*};
}
display_arg_borrowed!(
    std::str::EscapeDebug<'_>,
    Cow<'_, str>,
    std::path::Display<'_>
);

macro_rules! number_arg {
    ($($t:ty),*) => {$(
        impl ToArg for $t {
            #[allow(clippy::cast_precision_loss, clippy::cast_lossless)]
            fn to_arg(&self) -> MessageArg {
                MessageArg::Number(*self as f64)
            }
        }
    )*};
}
number_arg!(u8, u16, u32, u64, usize, i32, i64);

/// A [`Message`] with the literal key: `msg!("engine-no-repo")`,
/// `msg!("engine-stash-missing", index = 3, name = name)`. A parameter (taken by
/// reference) is a text, a number, an `Oid`, a `RepoPath` or another `Message`.
#[macro_export]
macro_rules! msg {
    ($key:literal) => {
        $crate::Message::new($key, ::std::vec::Vec::new())
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::Message::new(
            $key,
            ::std::vec![$((stringify!($name), $crate::ToArg::to_arg(&$value))),+],
        )
    };
}

#[cfg(test)]
mod tests {
    use crate::{EngineError, Lang, Message, MessageArg};
    use project_domain::Oid;

    fn oid(c: char) -> Oid {
        c.to_string().repeat(40).parse().unwrap()
    }

    /// `Display` is the English text, also for messages made of several keys, with selectors,
    /// newlines and braces.
    #[test]
    fn display_is_the_english_text() {
        let post = EngineError::PostCommit {
            commit: Some(oid('a')),
            backup: Some("refs/project/backup/amend-1".to_owned()),
            reason: msg!("engine-commit-head-no-commit"),
        };
        assert_eq!(
            post.to_string(),
            format!(
                "commit {} was created, but HEAD does not point to a commit; the commit rewritten \
                 by `amend` is saved in `refs/project/backup/amend-1`: do not repeat the commit, \
                 reload status and log",
                oid('a')
            )
        );
        let drop = msg!(
            "engine-stash-drop-confirm",
            name = "stash@{0}",
            id = oid('b'),
            message = "On main: x",
            files = 2,
            untracked = "yes"
        );
        assert_eq!(
            drop.to_string(),
            format!(
                "stash `stash@{{0}}` ({} “On main: x”, 2 tracked files, with untracked files) \
                 would be dropped. Its commit would first be saved in \
                 `refs/project/backup/stash-...`, and `edit.undo` would put it back at the top \
                 of the list. To drop it, repeat with `\"expected\": \"{}\"`",
                oid('b'),
                oid('b')
            )
        );
        let unknown = msg!(
            "engine-stash-drop-confirm",
            name = "s",
            id = "i",
            message = "m",
            files = -1,
            untracked = "no"
        );
        assert!(
            unknown
                .to_string()
                .starts_with("stash `s` (i “m”) would be"),
            "{unknown}"
        );
        let none = msg!("engine-stash-no-index", index = 5, n = 2, last = 1);
        assert_eq!(
            none.to_string(),
            "there is no `stash@{5}`: there are 2 stashes (from `stash@{0}` to `stash@{1}`), \
             nothing changed"
        );
        let busy = EngineError::NotAllowed(msg!(
            "engine-operation-in-progress",
            operation = "am",
            command = "branch.checkout",
            what = msg!("engine-busy-checkout")
        ));
        assert_eq!(
            busy.to_string(),
            "operation refused: a `git am` is in progress: `branch.checkout` would switch branch \
             in the middle of the operation, nothing changed. Finish or abort the operation and \
             try again"
        );
        let aborted = msg!(
            "engine-pull-aborted",
            text = "stopped",
            operation = "rebase"
        );
        assert_eq!(
            aborted.to_string(),
            "stopped\n(the rebase was aborted, nothing changed)"
        );
        let note = msg!(
            "engine-commit-backup-note",
            text = "hook",
            note = msg!("engine-other", text = "note")
        );
        assert_eq!(note.to_string(), "hook\n\nnote");
        let abort = msg!(
            "engine-merge-abort-failed",
            git = "merge",
            stderr = "error",
            hint = "mixed",
            state = "still",
            backup = "b"
        );
        assert_eq!(
            abort.to_string(),
            "`git merge --abort` failed (error). This happens when a staged file also has \
             unstaged changes: stage them (`index.stage`) or discard them (`worktree.discard`) \
             and try again. The operation is still in progress and some files may already have \
             been rewritten: reload the status. The index and worktree from before are saved in \
             `b` (`git stash apply b`)"
        );
        let plain = msg!(
            "engine-merge-abort-failed",
            git = "revert",
            stderr = "e",
            hint = "none",
            state = "maybe",
            backup = "b"
        );
        assert!(
            plain
                .to_string()
                .starts_with("`git revert --abort` failed (e). The operation may"),
            "{plain}"
        );
        let undo = EngineError::UndoConflict {
            command: "edit.undo",
            refs: vec![
                msg!("engine-head-changed", expected = "a", now = "b"),
                msg!("engine-ref-missing"),
            ],
        };
        assert!(
            undo.to_string()
                .contains("(HEAD: should have been a, now it is b; missing)"),
            "{undo}"
        );
    }

    /// In JSON a message carries the English text, the key and the arguments, and reads back
    /// the same (also from a plain string).
    #[test]
    fn json_round_trip() {
        let m = msg!(
            "engine-remote-changed",
            name = "origin",
            command = "remote.remove",
            refs = MessageArg::Joined(
                vec![msg!("engine-head-changed", expected = "a", now = "b")],
                "; ".into()
            )
        );
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["key"], "engine-remote-changed");
        assert_eq!(json["text"], m.to_string());
        assert_eq!(json["args"]["name"], "origin");
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.text_in(Lang::It), m.text_in(Lang::It));
        let plain: Message = serde_json::from_value(serde_json::json!("text")).unwrap();
        assert_eq!(plain.to_string(), "text");
        let n = msg!("engine-journal-remote-add-fetched", name = "o", n = 1);
        let back: Message = serde_json::from_value(serde_json::to_value(&n).unwrap()).unwrap();
        assert_eq!(
            back.to_string(),
            "Add remote `o` (with fetch: 1 remote branch)"
        );
    }

    #[test]
    fn the_ui_language_translates_the_whole_message() {
        let err = EngineError::NotAllowed(msg!(
            "engine-operation-in-progress",
            operation = "merge",
            command = "stash.apply",
            what = msg!("engine-busy-stash-apply")
        ));
        assert_eq!(
            err.text_in(Lang::En),
            "operation refused: a merge is in progress: `stash.apply` would apply the stash in \
             the middle of the operation, nothing changed. Finish or abort the operation and \
             try again"
        );
        let joined = msg!(
            "engine-remote-changed",
            name = "origin",
            command = "remote.remove",
            refs = MessageArg::Joined(
                vec![
                    msg!("engine-remote-push-default-changed"),
                    msg!("engine-ref-missing")
                ],
                "; ".into()
            )
        );
        assert!(
            joined
                .text_in(Lang::En)
                .contains("(`remote.pushDefault` changed; missing)"),
            "{}",
            joined.text_in(Lang::En)
        );
        // git's text stays as it is.
        let git = EngineError::Git {
            command: "commit".to_owned(),
            stderr: "fatal: nothing".to_owned(),
        };
        assert_eq!(git.text_in(Lang::En), "`git commit` failed: fatal: nothing");
        assert_eq!(git.message().key(), "engine-git");
    }
}
