//! Engine texts in command results (`warnings`, journal descriptions): in JSON they are
//! [`Message`] (`{"text", "key", "args"}`, or a string); here they become text in the UI
//! language, as soon as the result is read.

use project_engine::Message;
use serde::{Deserialize, Deserializer};

/// `#[serde(deserialize_with = "crate::engine_text::texts")]` for a `Vec<String>`.
pub(crate) fn texts<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(Vec::<Message>::deserialize(d)?
        .iter()
        .map(Message::text)
        .collect())
}

/// Like [`texts`], for a `String`.
pub(crate) fn text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Message::deserialize(d)?.text())
}

/// Like [`texts`], for an `Option<String>`.
pub(crate) fn opt_text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<Message>::deserialize(d)?
        .as_ref()
        .map(Message::text))
}
