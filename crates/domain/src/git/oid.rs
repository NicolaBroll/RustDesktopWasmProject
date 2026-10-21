use crate::DomainError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Identificatore di un oggetto git (SHA-1, 20 byte).
///
/// Si legge da 40 cifre esadecimali. In ingresso sono accettate sia minuscole sia maiuscole
/// (come fa git stesso); in uscita (`Display`, JSON) è sempre in minuscolo, così due `Oid`
/// uguali hanno sempre la stessa forma testuale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid([u8; 20]);

impl Oid {
    /// Numero di byte di un `Oid` SHA-1.
    pub const LEN: usize = 20;
    /// Numero di cifre esadecimali della forma completa.
    pub const HEX_LEN: usize = 40;
    /// Numero di cifre della forma abbreviata restituita da [`Oid::short`].
    pub const SHORT_LEN: usize = 7;

    /// Costruisce un `Oid` dai 20 byte grezzi.
    pub const fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    /// I 20 byte grezzi.
    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// Legge 40 cifre esadecimali (minuscole o maiuscole) da byte, ad esempio da un campo
    /// dell'output di git. Nessuno spazio o a capo è ammesso: chi chiama deve già aver
    /// separato il campo.
    pub fn from_hex(hex: &[u8]) -> Result<Self, DomainError> {
        let invalid = |reason: String| DomainError::InvalidOid {
            input: String::from_utf8_lossy(hex).into_owned(),
            reason,
        };
        if hex.len() != Self::HEX_LEN {
            return Err(invalid(format!(
                "servono {} cifre esadecimali, trovate {}",
                Self::HEX_LEN,
                hex.len()
            )));
        }
        let mut bytes = [0u8; 20];
        for (i, pair) in hex.as_chunks::<2>().0.iter().enumerate() {
            let digit = |pos: usize| {
                hex_value(pair[pos]).ok_or_else(|| {
                    invalid(format!(
                        "carattere non esadecimale in posizione {}",
                        i * 2 + pos + 1
                    ))
                })
            };
            bytes[i] = (digit(0)? << 4) | digit(1)?;
        }
        Ok(Self(bytes))
    }

    /// Forma abbreviata per la UI: le prime [`Oid::SHORT_LEN`] cifre in minuscolo.
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
                f.write_str("una stringa di 40 cifre esadecimali")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Oid, E> {
                Oid::from_hex(v.as_bytes()).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(OidVisitor)
    }
}
