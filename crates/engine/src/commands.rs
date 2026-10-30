use crate::{EngineError, JobContext, Session};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};

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

/// What a command does to the repository ([`CommandSpec::writes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Only reads.
    Read,
    /// Modifies the repository (index, worktree, refs).
    Write,
}

/// A registered command: deserializes the parameters, then runs the typed handler.
type Handler = Box<dyn Fn(&mut Session, Value) -> Result<Value, EngineError> + Send + Sync>;

#[derive(Default)]
pub struct Registry {
    map: BTreeMap<&'static str, (CommandSpec, Handler)>,
}

impl Registry {
    /// Registers command `id`. The registry deserializes the JSON parameters into `P` (an error
    /// is [`EngineError::InvalidParams`] with `id`) and passes them to `handler`; `P` =
    /// [`Value`] takes them as they are.
    pub fn add<P: DeserializeOwned + 'static>(
        &mut self,
        id: &'static str,
        title: &'static str,
        params_help: &'static str,
        access: Access,
        handler: fn(&mut Session, P) -> Result<Value, EngineError>,
    ) {
        let spec = CommandSpec {
            id,
            title,
            params_help,
            writes: access == Access::Write,
        };
        let run: Handler = Box::new(move |session, params| {
            let params =
                serde_json::from_value(params).map_err(|e| EngineError::InvalidParams {
                    id,
                    reason: msg!("engine-params", error = e.to_string()),
                })?;
            handler(session, params)
        });
        let previous = self.map.insert(id, (spec, run));
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
    /// the command the session goes back to an empty context, also if the command panics (the
    /// panic then continues: a session reused afterwards must not see a stale cancellation).
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
        let result = panic::catch_unwind(AssertUnwindSafe(|| handler(&mut *session, params)));
        session.set_job(JobContext::new());
        result.unwrap_or_else(|payload| panic::resume_unwind(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CancelToken, MemoryOpener};
    use std::sync::Arc;

    fn boom(_: &mut Session, _: Value) -> Result<Value, EngineError> {
        panic!("boom");
    }

    #[test]
    fn a_panicking_command_leaves_an_empty_job_context() {
        let mut r = Registry::default();
        r.add("test.boom", "Panics", "{}", Access::Read, boom);
        let mut s = Session::new(Arc::new(MemoryOpener::new()));
        let cancel = CancelToken::new();
        cancel.cancel();
        let job = JobContext::new().with_cancel(cancel);
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            r.run_with("test.boom", &mut s, Value::Null, job)
        }));
        assert!(outcome.is_err());
        assert!(!s.job().is_cancelled());
    }
}
