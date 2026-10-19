use crate::DomainError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    title: String,
    items: Vec<Item>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new("Senza titolo")
    }
}

impl Document {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn set_title(&mut self, title: &str) -> Result<(), DomainError> {
        self.title = non_empty(title)?;
        Ok(())
    }

    /// Aggiunge una voce in fondo e ne restituisce l'indice.
    pub fn add_item(&mut self, text: &str) -> Result<usize, DomainError> {
        self.items.push(Item {
            text: non_empty(text)?,
            done: false,
        });
        Ok(self.items.len() - 1)
    }

    /// Inverte lo stato della voce e restituisce il nuovo valore.
    pub fn toggle_item(&mut self, index: usize) -> Result<bool, DomainError> {
        let item = self.item_mut(index)?;
        item.done = !item.done;
        Ok(item.done)
    }

    pub fn remove_item(&mut self, index: usize) -> Result<Item, DomainError> {
        self.check_index(index)?;
        Ok(self.items.remove(index))
    }

    fn item_mut(&mut self, index: usize) -> Result<&mut Item, DomainError> {
        self.check_index(index)?;
        Ok(&mut self.items[index])
    }

    fn check_index(&self, index: usize) -> Result<(), DomainError> {
        let len = self.items.len();
        if index < len {
            Ok(())
        } else {
            Err(DomainError::IndexOutOfRange { index, len })
        }
    }
}

fn non_empty(text: &str) -> Result<String, DomainError> {
    let text = text.trim();
    if text.is_empty() {
        Err(DomainError::EmptyText)
    } else {
        Ok(text.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_toggle_remove() {
        let mut doc = Document::new("Spesa");
        assert_eq!(doc.add_item("  pane ").unwrap(), 0);
        assert_eq!(doc.items()[0].text, "pane");
        assert!(doc.toggle_item(0).unwrap());
        assert_eq!(doc.remove_item(0).unwrap().text, "pane");
        assert!(doc.items().is_empty());
    }

    #[test]
    fn rejects_empty_and_out_of_range() {
        let mut doc = Document::default();
        assert_eq!(doc.add_item("   "), Err(DomainError::EmptyText));
        assert_eq!(doc.set_title(""), Err(DomainError::EmptyText));
        assert_eq!(
            doc.toggle_item(3),
            Err(DomainError::IndexOutOfRange { index: 3, len: 0 })
        );
    }
}
