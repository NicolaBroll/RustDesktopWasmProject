use super::path::{deserialize_bytes, serialize_bytes};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::fmt;

/// Text read from git's output that may not be UTF-8, such as the content of a diff line:
/// it keeps the exact bytes, so they can be written back identical (e.g. in a patch).
///
/// To display it use [`ByteText::to_string_lossy`] (or `Display`). In JSON it has the same form
/// as [`super::RepoPath`]: a string if the bytes are valid UTF-8, otherwise a byte array.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ByteText(Vec<u8>);

impl ByteText {
    /// Builds the text from raw bytes.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    /// The exact bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the text and returns the bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    /// True if there are no bytes.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The text, if it is valid UTF-8.
    pub fn to_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// Text to display: non-UTF-8 bytes become `U+FFFD`.
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }
}

impl fmt::Display for ByteText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

impl From<&str> for ByteText {
    fn from(s: &str) -> Self {
        Self(s.as_bytes().to_vec())
    }
}

impl From<String> for ByteText {
    fn from(s: String) -> Self {
        Self(s.into_bytes())
    }
}

impl From<&[u8]> for ByteText {
    fn from(b: &[u8]) -> Self {
        Self(b.to_vec())
    }
}

impl From<Vec<u8>> for ByteText {
    fn from(b: Vec<u8>) -> Self {
        Self(b)
    }
}

impl Serialize for ByteText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_bytes(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for ByteText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_bytes(deserializer, "a text as a string or as a byte array").map(Self)
    }
}
