use project_domain::Document;
use std::sync::Arc;

/// Documento corrente più cronologia per undo/redo.
///
/// Gli snapshot sono `Arc<Document>`: salvarne uno costa un incremento del
/// contatore; la copia vera avviene solo alla prima scrittura (`Arc::make_mut`).
#[derive(Debug, Clone, Default)]
pub struct Session {
    current: Arc<Document>,
    undo: Vec<Arc<Document>>,
    redo: Vec<Arc<Document>>,
}

impl Session {
    pub fn new(doc: Document) -> Self {
        Self {
            current: Arc::new(doc),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn document(&self) -> &Document {
        &self.current
    }

    /// Ogni modifica passa da qui. Se `f` fallisce lo stato e la cronologia
    /// restano invariati.
    pub fn edit<R, E>(&mut self, f: impl FnOnce(&mut Document) -> Result<R, E>) -> Result<R, E> {
        self.undo.push(Arc::clone(&self.current));
        match f(Arc::make_mut(&mut self.current)) {
            Ok(r) => {
                self.redo.clear();
                Ok(r)
            }
            Err(e) => {
                self.current = self.undo.pop().expect("snapshot appena salvato");
                Err(e)
            }
        }
    }

    /// Sostituisce l'intero documento (nuovo, apertura); annullabile.
    pub fn replace(&mut self, doc: Document) {
        self.undo
            .push(std::mem::replace(&mut self.current, Arc::new(doc)));
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(&mut self.current, prev));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(&mut self.current, next));
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::DomainError;

    #[test]
    fn failed_edit_leaves_history_untouched() {
        let mut s = Session::default();
        s.edit(|d| d.add_item("a")).unwrap();
        assert!(s.undo());
        assert!(s.can_redo());
        let before = s.document().clone();
        assert_eq!(s.edit(|d| d.add_item("")), Err(DomainError::EmptyText));
        assert_eq!(s.document(), &before);
        assert!(s.can_redo(), "un errore non deve cancellare il redo");
        assert!(!s.can_undo());
    }
}
