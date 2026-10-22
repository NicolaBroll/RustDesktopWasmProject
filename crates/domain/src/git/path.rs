use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::fmt;

/// Percorso di un file nel repo, relativo alla radice e con `/` come separatore, così come lo
/// scrive git.
///
/// Tiene i byte esatti: su Unix un nome di file può non essere UTF-8 e va ripassato a git
/// identico. Per mostrarlo si usa [`RepoPath::to_string_lossy`] (o `Display`), che sostituisce
/// i byte non validi con `U+FFFD`.
///
/// In JSON è una stringa se i byte sono UTF-8 valido, altrimenti un array di byte
/// (es. `[99, 97, 255]`); in lettura accetta entrambe le forme.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct RepoPath(Vec<u8>);

impl RepoPath {
    /// Costruisce un percorso dai byte grezzi.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    /// I byte esatti del percorso, da ripassare a git.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consuma il percorso e restituisce i byte.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    /// Il percorso come testo, se è UTF-8 valido.
    pub fn to_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// Testo da mostrare: i byte non UTF-8 diventano `U+FFFD`. Non va ripassato a git.
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }

    /// Ultimo componente del percorso (dopo l'ultimo `/`), per la UI.
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
        match self.to_str() {
            Some(s) => serializer.serialize_str(s),
            None => self.0.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for RepoPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PathVisitor;

        impl<'de> Visitor<'de> for PathVisitor {
            type Value = RepoPath;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("un percorso come stringa o come array di byte")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<RepoPath, E> {
                Ok(RepoPath::from(v))
            }

            fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<RepoPath, E> {
                Ok(RepoPath::from(v))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<RepoPath, A::Error> {
                let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(b) = seq.next_element::<u8>()? {
                    bytes.push(b);
                }
                Ok(RepoPath(bytes))
            }
        }

        deserializer.deserialize_any(PathVisitor)
    }
}
