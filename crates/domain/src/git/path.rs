use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::fmt;

/// Path of a file in the repo, relative to the root and with `/` as separator, exactly as
/// git writes it.
///
/// It keeps the exact bytes: on Unix a file name may not be UTF-8 and must be passed back to git
/// identical. To display it use [`RepoPath::to_string_lossy`] (or `Display`), which replaces
/// invalid bytes with `U+FFFD`.
///
/// In JSON it is a string if the bytes are valid UTF-8, otherwise a byte array
/// (e.g. `[99, 97, 255]`); when reading it accepts both forms.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct RepoPath(Vec<u8>);

impl RepoPath {
    /// Builds a path from raw bytes.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    /// The exact bytes of the path, to pass back to git.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the path and returns the bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    /// The path as text, if it is valid UTF-8.
    pub fn to_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// Text to display: non-UTF-8 bytes become `U+FFFD`. Not to be passed back to git.
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }

    /// Last component of the path (after the last `/`), for the UI.
    pub fn file_name(&self) -> &[u8] {
        match self.0.iter().rposition(|&b| b == b'/') {
            Some(i) => &self.0[i + 1..],
            None => &self.0,
        }
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

impl From<&str> for RepoPath {
    fn from(s: &str) -> Self {
        Self(s.as_bytes().to_vec())
    }
}

impl From<String> for RepoPath {
    fn from(s: String) -> Self {
        Self(s.into_bytes())
    }
}

impl From<&[u8]> for RepoPath {
    fn from(b: &[u8]) -> Self {
        Self(b.to_vec())
    }
}

impl From<Vec<u8>> for RepoPath {
    fn from(b: Vec<u8>) -> Self {
        Self(b)
    }
}

impl Serialize for RepoPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_bytes(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for RepoPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_bytes(deserializer, "a path as a string or as a byte array").map(Self)
    }
}

impl JsonSchema for RepoPath {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "RepoPath".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "Path relative to the root of the worktree, with `/`; a byte array if it is not UTF-8",
            "anyOf": [
                { "type": "string" },
                { "type": "array", "items": { "type": "integer", "minimum": 0, "maximum": 255 } },
            ],
        })
    }
}

/// Serializes bytes as a string if they are valid UTF-8, otherwise as a byte array.
pub(super) fn serialize_bytes<S: Serializer>(
    bytes: &[u8],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match std::str::from_utf8(bytes) {
        Ok(s) => serializer.serialize_str(s),
        Err(_) => bytes.serialize(serializer),
    }
}

/// Reads bytes written by [`serialize_bytes`]: a string or a byte array.
pub(super) fn deserialize_bytes<'de, D: Deserializer<'de>>(
    deserializer: D,
    expecting: &'static str,
) -> Result<Vec<u8>, D::Error> {
    struct BytesVisitor(&'static str);

    impl<'de> Visitor<'de> for BytesVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Vec<u8>, E> {
            Ok(v.as_bytes().to_vec())
        }

        fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Vec<u8>, E> {
            Ok(v.to_vec())
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
            let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(b) = seq.next_element::<u8>()? {
                bytes.push(b);
            }
            Ok(bytes)
        }
    }

    deserializer.deserialize_any(BytesVisitor(expecting))
}
