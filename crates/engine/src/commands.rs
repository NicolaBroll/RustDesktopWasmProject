use crate::{EngineError, JobContext, Session};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct CommandSpec {
    /// Stable id: never change it once published.
    pub id: &'static str,
    pub title: &'static str,
    /// Description of the JSON parameters, for CLI, MCP and help.
    pub params_help: &'static str,
    /// True if the command modifies the repository (index, worktree, refs). The UI disables them
    /// while a job is running (§6); CLI and MCP show it in the catalog.
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
        assert!(previous.is_none(), "command registered twice");
    }

    pub fn list(&self) -> impl Iterator<Item = &CommandSpec> {
        self.map.values().map(|(spec, _)| spec)
    }

    /// Commands whose id or title contains `text` (case-insensitive).
    pub fn search<'a>(&'a self, text: &str) -> impl Iterator<Item = &'a CommandSpec> {
        let needle = text.to_lowercase();
        self.list().filter(move |spec| {
            spec.id.contains(&needle) || spec.title.to_lowercase().contains(&needle)
        })
    }

    pub fn get(&self, id: &str) -> Option<&CommandSpec> {
        self.map.get(id).map(|(spec, _)| spec)
    }

    /// Runs a command. `null` as parameters is the same as `{}`. The command receives an empty
    /// [`JobContext`]: no progress, never cancelled (see [`Registry::run_with`]).
    pub fn run(
        &self,
        id: &str,
        session: &mut Session,
        params: Value,
    ) -> Result<Value, EngineError> {
        self.run_with(id, session, params, JobContext::new())
    }

    /// Like [`Registry::run`], with `job` for progress and cancellation (§6): the command
    /// finds it in [`Session::job`] for its whole duration. Slow commands (`remote.fetch`)
    /// send progress to `job` and stop if it is cancelled; the others ignore it. After
    /// the command the session goes back to an empty context.
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

/// Deserializes a command's parameters into its dedicated type.
pub(crate) fn parse_params<T: DeserializeOwned>(
    id: &'static str,
    params: Value,
) -> Result<T, EngineError> {
    serde_json::from_value(params).map_err(|e| EngineError::InvalidParams {
        id,
        reason: msg!("engine-params", error = e.to_string()),
    })
}
