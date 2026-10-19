//! Catalogo dei comandi predefiniti.

use crate::commands::parse_params;
use crate::{CommandSpec, EngineError, Registry, Session};
use project_domain::Document;
use serde::Deserialize;
use serde_json::{Value, json};

impl Registry {
    /// Registro con tutti i comandi predefiniti.
    pub fn builtin() -> Self {
        let mut r = Self::default();
        let mut add = |id, title, params_help, handler| {
            r.register(
                CommandSpec {
                    id,
                    title,
                    params_help,
                },
                handler,
            )
        };
        add("document.get", "Leggi il documento", "{}", document_get);
        add(
            "document.new",
            "Nuovo documento",
            r#"{"title"?: string}"#,
            document_new,
        );
        add(
            "document.load",
            "Apri documento da testo",
            r#"{"content": string}"#,
            document_load,
        );
        add(
            "document.export",
            "Esporta il documento come testo",
            "{}",
            document_export,
        );
        add(
            "document.set_title",
            "Rinomina il documento",
            r#"{"title": string}"#,
            set_title,
        );
        add("item.add", "Aggiungi voce", r#"{"text": string}"#, item_add);
        add(
            "item.toggle",
            "Segna/rimuovi spunta",
            r#"{"index": number}"#,
            item_toggle,
        );
        add(
            "item.remove",
            "Elimina voce",
            r#"{"index": number}"#,
            item_remove,
        );
        add("edit.undo", "Annulla", "{}", undo);
        add("edit.redo", "Ripeti", "{}", redo);
        r
    }
}

type Out = Result<Value, EngineError>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParams {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Title {
    title: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OptTitle {
    title: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Content {
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    index: usize,
}

fn document_get(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("document.get", p)?;
    Ok(serde_json::to_value(s.document()).expect("Document è serializzabile"))
}

fn document_new(s: &mut Session, p: Value) -> Out {
    let OptTitle { title } = parse_params("document.new", p)?;
    let mut doc = Document::default();
    if let Some(title) = title {
        doc.set_title(&title)?;
    }
    s.replace(doc);
    Ok(json!({}))
}

fn document_load(s: &mut Session, p: Value) -> Out {
    let Content { content } = parse_params("document.load", p)?;
    let doc = Document::from_bytes(content.as_bytes())?;
    s.replace(doc);
    Ok(json!({}))
}

fn document_export(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("document.export", p)?;
    let content = String::from_utf8(s.document().to_bytes()).expect("JSON è UTF-8");
    Ok(json!({ "content": content }))
}

fn set_title(s: &mut Session, p: Value) -> Out {
    let Title { title } = parse_params("document.set_title", p)?;
    s.edit(|d| d.set_title(&title))?;
    Ok(json!({}))
}

fn item_add(s: &mut Session, p: Value) -> Out {
    let Text { text } = parse_params("item.add", p)?;
    let index = s.edit(|d| d.add_item(&text))?;
    Ok(json!({ "index": index }))
}

fn item_toggle(s: &mut Session, p: Value) -> Out {
    let Index { index } = parse_params("item.toggle", p)?;
    let done = s.edit(|d| d.toggle_item(index))?;
    Ok(json!({ "done": done }))
}

fn item_remove(s: &mut Session, p: Value) -> Out {
    let Index { index } = parse_params("item.remove", p)?;
    let removed = s.edit(|d| d.remove_item(index))?;
    Ok(json!({ "removed": removed }))
}

fn undo(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("edit.undo", p)?;
    Ok(json!({ "changed": s.undo() }))
}

fn redo(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("edit.redo", p)?;
    Ok(json!({ "changed": s.redo() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn export_load_round_trip() {
        let r = Registry::builtin();
        let mut s = Session::default();
        r.run("item.add", &mut s, json!({"text": "uno"})).unwrap();
        let exported = r.run("document.export", &mut s, Value::Null).unwrap();

        let mut other = Session::default();
        r.run("document.load", &mut other, exported).unwrap();
        assert_eq!(other.document(), s.document());
    }

    #[test]
    fn errors() {
        let r = Registry::builtin();
        let mut s = Session::default();
        assert!(matches!(
            r.run("nope", &mut s, json!({})),
            Err(EngineError::UnknownCommand(_))
        ));
        assert!(matches!(
            r.run("item.add", &mut s, json!({"txt": "x"})),
            Err(EngineError::InvalidParams { id: "item.add", .. })
        ));
        assert!(matches!(
            r.run("item.remove", &mut s, json!({"index": 0})),
            Err(EngineError::Domain(_))
        ));
    }

    #[test]
    fn search_matches_id_and_title() {
        let r = Registry::builtin();
        let ids: Vec<_> = r.search("ANNULLA").map(|c| c.id).collect();
        assert_eq!(ids, ["edit.undo"]);
        assert!(r.search("item.").count() >= 3);
    }

    #[derive(Debug, Clone)]
    enum Op {
        Add(String),
        Toggle(usize),
        Remove(usize),
        Title(String),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            "[a-z ]{0,6}".prop_map(Op::Add),
            (0usize..8).prop_map(Op::Toggle),
            (0usize..8).prop_map(Op::Remove),
            "[a-z ]{0,6}".prop_map(Op::Title),
        ]
    }

    proptest! {
        /// Annullare tutte le modifiche riuscite riporta allo stato iniziale;
        /// ripeterle tutte riporta allo stato finale.
        #[test]
        fn undo_redo_are_inverse(ops in proptest::collection::vec(op(), 0..40)) {
            let r = Registry::builtin();
            let mut s = Session::default();
            let initial = s.document().clone();
            let mut applied = 0;
            for op in ops {
                let (id, params) = match op {
                    Op::Add(text) => ("item.add", json!({ "text": text })),
                    Op::Toggle(index) => ("item.toggle", json!({ "index": index })),
                    Op::Remove(index) => ("item.remove", json!({ "index": index })),
                    Op::Title(title) => ("document.set_title", json!({ "title": title })),
                };
                if r.run(id, &mut s, params).is_ok() {
                    applied += 1;
                }
            }
            let fin = s.document().clone();
            for _ in 0..applied {
                prop_assert!(s.undo());
            }
            prop_assert!(!s.undo());
            prop_assert_eq!(s.document(), &initial);
            for _ in 0..applied {
                prop_assert!(s.redo());
            }
            prop_assert_eq!(s.document(), &fin);
        }
    }
}
