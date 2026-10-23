use crate::{EngineError, JobContext, Session};
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
    /// Vero se il comando modifica il repository (indice, worktree, ref). La UI li disabilita
    /// mentre un job è in corso (§6); CLI e MCP lo mostrano nel catalogo.
    pub writes: bool,
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

    /// Esegue un comando. `null` come parametri equivale a `{}`. Il comando riceve un
    /// [`JobContext`] vuoto: nessun avanzamento, mai annullato (vedi [`Registry::run_with`]).
    pub fn run(
        &self,
        id: &str,
        session: &mut Session,
        params: Value,
    ) -> Result<Value, EngineError> {
        self.run_with(id, session, params, JobContext::new())
    }

    /// Come [`Registry::run`], con `job` per l'avanzamento e l'annullamento (§6): il comando
    /// lo trova in [`Session::job`] per tutta la sua durata. I comandi lenti (`remote.fetch`)
    /// mandano l'avanzamento a `job` e si fermano se è annullato; gli altri lo ignorano. Dopo
    /// il comando la sessione torna a un contesto vuoto.
    pub fn run_with(
        &self,
        id: &str,
        session: &mut Session,
        params: Value,
        job: JobContext,
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
        session.set_job(job);
        let result = handler(session, params);
        session.set_job(JobContext::new());
        result
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
