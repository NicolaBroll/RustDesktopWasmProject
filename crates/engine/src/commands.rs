use crate::{EngineError, Session};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct CommandSpec {
    /// Id stabile: non cambiarlo una volta pubblicato.
    pub id: &'static str,
    pub title: &'static str,
    /// Descrizione dei parametri JSON, per CLI, MCP e help.
    pub params_help: &'static str,
}

pub type Handler = fn(&mut Session, Value) -> Result<Value, EngineError>;

#[derive(Default)]
pub struct Registry {
    map: BTreeMap<&'static str, (CommandSpec, Handler)>,
}

impl Registry {
    pub fn register(&mut self, spec: CommandSpec, handler: Handler) {
        let previous = self.map.insert(spec.id, (spec, handler));
        assert!(previous.is_none(), "comando registrato due volte");
    }

    pub fn list(&self) -> impl Iterator<Item = &CommandSpec> {
        self.map.values().map(|(spec, _)| spec)
    }

    /// Comandi il cui id o titolo contiene `text` (senza distinguere maiuscole).
    pub fn search<'a>(&'a self, text: &str) -> impl Iterator<Item = &'a CommandSpec> {
        let needle = text.to_lowercase();
        self.list().filter(move |spec| {
            spec.id.contains(&needle) || spec.title.to_lowercase().contains(&needle)
        })
    }

    pub fn get(&self, id: &str) -> Option<&CommandSpec> {
        self.map.get(id).map(|(spec, _)| spec)
    }

    /// Esegue un comando. `null` come parametri equivale a `{}`.
    pub fn run(
        &self,
        id: &str,
        session: &mut Session,
        params: Value,
    ) -> Result<Value, EngineError> {
        let (_, handler) = self
            .map
            .get(id)
            .ok_or_else(|| EngineError::UnknownCommand(id.into()))?;
        let params = if params.is_null() {
            Value::Object(Default::default())
        } else {
            params
        };
        handler(session, params)
    }
}

/// Deserializza i parametri di un comando nel suo tipo dedicato.
pub(crate) fn parse_params<T: DeserializeOwned>(
    id: &'static str,
    params: Value,
) -> Result<T, EngineError> {
    serde_json::from_value(params).map_err(|e| EngineError::InvalidParams {
        id,
        reason: e.to_string(),
    })
}
