//! Cherry-pick e revert sul [`MemoryBackend`] (`commit.cherry_pick`, `commit.revert`, S31), con
//! gli stessi controlli del `CliBackend`.
//!
//! Come il merge in memoria, file per file: la patch del commit (dal genitore scelto al commit;
//! per il revert all'inverso) si applica a HEAD, e un file che HEAD ha cambiato in modo diverso
//! è un conflitto. Senza conflitti un commit nuovo sopra HEAD (per il cherry-pick con l'autore
//! originale); con conflitti l'operazione **resta in corso** come il merge (voci `u`, i file
//! senza conflitti in stage, `CHERRY_PICK_HEAD`/`REVERT_HEAD` e `MERGE_MSG` in
//! [`MemoryMerge`]). Un risultato uguale a HEAD (commit già applicato) è
//! [`EngineError::NotAllowed`] e non cambia niente.

use super::merge::{MemoryMerge, merge_partial};
use super::{HEADS, HeadRef, MemoryBackend};
use crate::EngineError;
use crate::git::GitBackend;
use crate::git::{
    BACKUP_PREFIX, MergeOutcome, PickKind, PickRequest, check_pick_state, pick_empty_error, short,
    split_message,
};

impl MemoryBackend {
    /// [`GitBackend::pick`](crate::GitBackend::pick).
    pub(super) fn pick_commit(
        &mut self,
        request: &PickRequest,
    ) -> Result<MergeOutcome, EngineError> {
        let kind = request.kind;
        check_pick_state(&self.status()?, kind)?;
        if self.head != HeadRef::Branch(request.branch.clone()) {
            return Err(EngineError::NotAllowed(format!(
                "HEAD è cambiata mentre si preparava `{}`: niente modificato, riprovare",
                kind.command()
            )));
        }
        let full = format!("{HEADS}{}", request.branch);
        self.check_value(&full, &request.expected)?;
        let Some(picked) = self.commits.get(&request.commit).cloned() else {
            return Err(EngineError::UnknownRev(request.commit.to_string()));
        };
        let commit_tree = self.tree_at(request.commit);
        let parent_tree = request.parent.map(|p| self.tree_at(p)).unwrap_or_default();
        let (base, theirs) = match kind {
            PickKind::CherryPick => (parent_tree, commit_tree),
            PickKind::Revert => (commit_tree, parent_tree),
        };
        let ours = self.tree_at(request.expected);
        let (merged, conflicts) = merge_partial(&base, &ours, &theirs);
        if conflicts.is_empty() && merged == ours {
            return Err(pick_empty_error(kind, &request.commit));
        }
        // Nel worktree vanno anche i file in conflitto presenti solo da loro.
        let mut written = merged.clone();
        for c in &conflicts {
            if let Some(entry) = c.ours.or(c.theirs) {
                written.insert(c.path.clone(), entry);
            }
        }
        let (_, files) = self.switch_plan(&written);
        if !files.is_empty() {
            return Err(EngineError::NotAllowed(format!(
                "`{}` sovrascriverebbe modifiche locali o file non tracciati ({}): committarli, \
                 metterli da parte o spostarli e riprovare; niente modificato",
                kind.command(),
                files
                    .iter()
                    .map(|f| format!("`{f}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let mut work = self.clone();
        let (differing, _) = work.switch_plan(&merged);
        let index = work.switch_index(&differing, &merged);
        let mut outcome = MergeOutcome {
            after: request.expected,
            backup: String::new(),
            in_progress: false,
            conflicts: Vec::new(),
            message: String::new(),
            warnings: Vec::new(),
        };
        if conflicts.is_empty() {
            let author = match kind {
                PickKind::CherryPick => Some(picked.author.clone()),
                PickKind::Revert => None,
            };
            let (summary, body) = split_message(&request.message);
            let id = work.make_commit(vec![request.expected], &merged, author, summary, body)?;
            work.set_ref(full, id, None);
            work.rebase_entries(&index);
            outcome.after = id;
        } else {
            // Le etichette dei marcatori come git: `<id> (<oggetto>)`, `parent of ...` per il
            // revert.
            let label = format!("{} ({})", short(&request.commit), picked.summary);
            let label = match kind {
                PickKind::CherryPick => label,
                PickKind::Revert => format!("parent of {label}"),
            };
            let mut text = Vec::new();
            let mut merge_msg = format!("{}\n\n# Conflicts:\n", request.message);
            for c in &conflicts {
                work.add_conflict(c, &label);
                text.push(format!("CONFLICT (content): Merge conflict in {}", c.path));
                merge_msg.push_str(&format!("#\t{}\n", c.path));
                outcome.conflicts.push(c.path.clone());
            }
            work.rebase_entries(&index);
            work.operation = Some(kind.operation());
            work.merge_state = Some(MemoryMerge {
                heads: vec![request.commit],
                message: merge_msg,
            });
            text.push(format!(
                "error: could not {} {}... {}",
                match kind {
                    PickKind::CherryPick => "apply",
                    PickKind::Revert => "revert",
                },
                short(&request.commit),
                picked.summary
            ));
            outcome.in_progress = true;
            outcome.message = text.join("\n");
        }
        let prefix = format!("{BACKUP_PREFIX}{}-", kind.git());
        let n = work
            .commit_backups
            .iter()
            .filter(|(b, _)| b.starts_with(&prefix) && !b.starts_with(&format!("{prefix}abort-")))
            .count()
            + 1;
        outcome.backup = format!("{prefix}{n}");
        work.commit_backups
            .push((outcome.backup.clone(), request.expected));
        *self = work;
        Ok(outcome)
    }
}
