//! Submodules on the [`MemoryBackend`] (`impl-docs/git/submodules.md`): a minimal model, the
//! list of [`SubmoduleInfo`] as the test sets it ([`MemoryBackend::set_submodules`]). An update
//! applies the same plan and refusals as the `CliBackend` ([`plan_update`]) and moves the HEAD
//! of each submodule run to its recorded commit; nested submodules are not modelled.

use super::MemoryBackend;
use crate::git::{
    SUBMODULE_BACKUP_KIND, SubmoduleInfo, SubmoduleMoved, SubmoduleUpdate, SubmoduleUpdated,
    plan_update, submodule_state,
};
use crate::{BACKUP_PREFIX, EngineError, JobContext};
use project_domain::{Oid, RepoPath};

impl MemoryBackend {
    /// The submodules `submodule.list` reports, in path order.
    pub fn set_submodules(&mut self, mut submodules: Vec<SubmoduleInfo>) {
        submodules.sort_by(|a, b| a.path.cmp(&b.path));
        self.submodules = submodules;
    }

    /// The backups saved by `submodule.update` inside the submodules: ref name, submodule path,
    /// the HEAD saved.
    pub fn submodule_backups(&self) -> &[(String, RepoPath, Oid)] {
        &self.submodule_backups
    }

    /// The clone of a repo with submodules: checked out on the recorded commit (`checkout`,
    /// `--recurse-submodules`) or uninitialized.
    pub(super) fn clone_submodules(&mut self, from: &[SubmoduleInfo], checkout: bool) {
        self.submodules = from
            .iter()
            .map(|s| {
                let head = if checkout { s.recorded } else { None };
                SubmoduleInfo {
                    head,
                    detached: checkout,
                    state: submodule_state(false, checkout, false, head, s.recorded),
                    changes: false,
                    untracked: false,
                    operation: None,
                    unreferenced: false,
                    ..s.clone()
                }
            })
            .collect();
    }

    pub(super) fn memory_submodule_update(
        &mut self,
        request: &SubmoduleUpdate,
        job: &JobContext,
    ) -> Result<SubmoduleUpdated, EngineError> {
        let plan = plan_update(self.submodules.clone(), request)?;
        if job.is_cancelled() {
            return Err(EngineError::Cancelled {
                command: "submodule.update",
            });
        }
        let mut result = SubmoduleUpdated {
            unchanged: plan.unchanged.into_iter().map(|s| s.path).collect(),
            refused: plan.refused,
            ..SubmoduleUpdated::default()
        };
        for info in plan.run {
            let backup = match (info.detached, info.head) {
                (true, Some(head)) => {
                    let name = format!(
                        "{BACKUP_PREFIX}{SUBMODULE_BACKUP_KIND}-{}",
                        self.submodule_backups.len() + 1
                    );
                    self.submodule_backups
                        .push((name.clone(), info.path.clone(), head));
                    Some(name)
                }
                _ => None,
            };
            let sub = self
                .submodules
                .iter_mut()
                .find(|s| s.path == info.path)
                .expect("planned from the list");
            sub.head = sub.recorded;
            sub.detached = true;
            sub.unreferenced = false;
            sub.state = submodule_state(
                false,
                true,
                sub.changes || sub.untracked,
                sub.head,
                sub.recorded,
            );
            result.messages.push(format!(
                "Submodule path '{}': checked out '{}'",
                sub.path,
                sub.head.map(|h| h.to_string()).unwrap_or_default()
            ));
            result.updated.push(SubmoduleMoved {
                path: info.path,
                from: info.head,
                to: sub.head,
                backup,
            });
        }
        Ok(result)
    }
}
