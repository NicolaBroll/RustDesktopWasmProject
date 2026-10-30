use crate::DomainError;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

/// Identifier of a git object (SHA-1, 20 bytes).
///
/// Read from 40 hexadecimal digits. On input both lowercase and uppercase are accepted
/// (as git itself does); on output (`Display`, JSON) it is always lowercase, so two equal `Oid`s
/// always have the same textual form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid([u8; 20]);

impl Oid {
    /// Number of bytes of a SHA-1 `Oid`.
    pub const LEN: usize = 20;
    /// Number of hexadecimal digits of the full form.
    pub const HEX_LEN: usize = 40;
    /// Number of digits of the short form returned by [`Oid::short`].
    pub const SHORT_LEN: usize = 7;

    /// Builds an `Oid` from the 20 raw bytes.
    pub const fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    /// The 20 raw bytes.
    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// Reads 40 hexadecimal digits (lowercase or uppercase) from bytes, for example from a field
    /// of git's output. No whitespace or newline is allowed: the caller must already have
    /// split the field.
    pub fn from_hex(hex: &[u8]) -> Result<Self, DomainError> {
        let invalid = |reason: String| DomainError::InvalidOid {
            input: String::from_utf8_lossy(hex).into_owned(),
            reason,
        };
        if hex.len() != Self::HEX_LEN {
            return Err(invalid(format!(
                "{} hex digits are needed, found {}",
                Self::HEX_LEN,
                hex.len()
            )));
        }
        let mut bytes = [0u8; 20];
        for (i, pair) in hex.as_chunks::<2>().0.iter().enumerate() {
            let digit = |pos: usize| {
                hex_value(pair[pos]).ok_or_else(|| {
                    invalid(format!("non-hex character at position {}", i * 2 + pos + 1))
                })
            };
            bytes[i] = (digit(0)? << 4) | digit(1)?;
        }
        Ok(Self(bytes))
    }

    /// Short form for the UI: the first [`Oid::SHORT_LEN`] digits in lowercase.
    pub fn short(&self) -> String {
        let mut s = self.to_string();
        s.truncate(Self::SHORT_LEN);
        s
    }
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for Oid {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hex(s.as_bytes())
    }
}

impl JsonSchema for Oid {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Oid".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "Full commit id (40 hex digits)",
            "type": "string",
            "pattern": "^[0-9a-fA-F]{40}$",
        })
    }
}

impl Serialize for Oid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Oid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OidVisitor;

        impl serde::de::Visitor<'_> for OidVisitor {
            type Value = Oid;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string of 40 hex digits")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Oid, E> {
                Oid::from_hex(v.as_bytes()).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(OidVisitor)
    }
}
