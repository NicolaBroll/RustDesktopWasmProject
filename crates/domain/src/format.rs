//! Formato di file: JSON con identificativo e versione espliciti.

use crate::{Document, DomainError};
use serde::{Deserialize, Serialize};

pub const FORMAT_ID: &str = "project-doc";
pub const FORMAT_VERSION: u32 = 1;

#[derive(Serialize)]
struct FileOut<'a> {
    format: &'static str,
    version: u32,
    document: &'a Document,
}

#[derive(Deserialize)]
struct Header {
    format: String,
    version: u32,
}

#[derive(Deserialize)]
struct FileIn {
    document: Document,
}

impl Document {
    pub fn to_bytes(&self) -> Vec<u8> {
        let file = FileOut {
            format: FORMAT_ID,
            version: FORMAT_VERSION,
            document: self,
        };
        let mut bytes = serde_json::to_vec_pretty(&file).expect("Document è sempre serializzabile");
        bytes.push(b'\n');
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DomainError> {
        let invalid = |e: serde_json::Error| DomainError::InvalidFile(e.to_string());
        let header: Header = serde_json::from_slice(bytes).map_err(invalid)?;
        if header.format != FORMAT_ID || header.version != FORMAT_VERSION {
            return Err(DomainError::UnsupportedFormat {
                format: header.format,
                version: header.version,
            });
        }
        let file: FileIn = serde_json::from_slice(bytes).map_err(invalid)?;
        Ok(file.document)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut doc = Document::new("Prova");
        doc.add_item("uno").unwrap();
        doc.toggle_item(0).unwrap();
        assert_eq!(Document::from_bytes(&doc.to_bytes()).unwrap(), doc);
    }

    #[test]
    fn rejects_other_versions() {
        let bytes = br#"{"format":"project-doc","version":99,"document":{}}"#;
        assert!(matches!(
            Document::from_bytes(bytes),
            Err(DomainError::UnsupportedFormat { version: 99, .. })
        ));
        assert!(matches!(
            Document::from_bytes(b"not json"),
            Err(DomainError::InvalidFile(_))
        ));
    }
}
