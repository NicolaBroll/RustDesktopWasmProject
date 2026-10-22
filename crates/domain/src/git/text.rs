use super::path::{deserialize_bytes, serialize_bytes};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::fmt;

/// Testo letto dall'output di git che può non essere UTF-8, come il contenuto di una riga di un
/// diff: tiene i byte esatti, per poterli riscrivere identici (es. in una patch).
///
/// Per mostrarlo si usa [`ByteText::to_string_lossy`] (o `Display`). In JSON ha la stessa forma
/// di [`super::RepoPath`]: una stringa se i byte sono UTF-8 valido, altrimenti un array di byte.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ByteText(Vec<u8>);

impl ByteText {
    /// Costruisce il testo dai byte grezzi.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    /// I byte esatti.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consuma il testo e restituisce i byte.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    /// Vero se non ci sono byte.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Il testo, se è UTF-8 valido.
    pub fn to_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// Testo da mostrare: i byte non UTF-8 diventano `U+FFFD`.
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
        deserialize_bytes(deserializer, "un testo come stringa o come array di byte").map(Self)
    }
}
