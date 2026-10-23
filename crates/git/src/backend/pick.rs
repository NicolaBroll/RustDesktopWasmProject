//! Cherry-pick e revert sul [`CliBackend`] (`commit.cherry_pick`, `commit.revert`, S31),
//! secondo §5 e `impl-docs/git/cherry-pick.md`.
//!
//! `git -c core.fsmonitor=false -c revert.reference=false <cherry-pick|revert> --no-edit
//! --no-rerere-autoupdate [-m <n>] --end-of-options <oid>`, con `GIT_REFLOG_ACTION=project:
//! <id>` e `GIT_EDITOR=:`. Hook e firma secondo la configurazione (mai `--no-verify`).
//!
//! - riuscito: il branch su un commit nuovo;
//! - **fermo** (conflitti, un hook che rifiuta il commit): l'operazione **resta in corso**
//!   (`CHERRY_PICK_HEAD`/`REVERT_HEAD`), i file in conflitto dallo stato e dall'output;
//! - vuoto (commit già applicato): `git <cmd> --skip` se git l'ha lasciato in corso, poi
//!   [`EngineError::NotAllowed`] e il backup tolto;
//! - non partito (un file che verrebbe sovrascritto): niente scritto, il backup si toglie.
//!
//! Prima di tutto il backup `refs/project/backup/<cherry-pick|revert>-...` sul branch com'era;
//! si toglie solo se niente è cambiato.

use super::CliBackend;
use super::sync::overwrite_error;
use project_engine::{
    EngineError, MergeOutcome, PickKind, PickRequest, branch_ref, check_pick_state,
    pick_empty_error,
};
use std::ffi::OsStr;

/// Vero se il testo di git dice che il risultato è vuoto (niente da committare).
fn says_empty(text: &str) -> bool {
    text.contains("is now empty")
        || text.contains("nothing to commit")
        || text.contains("--allow-empty")
}

impl CliBackend {
    /// [`GitBackend::pick`](project_engine::GitBackend::pick).
    pub(super) fn pick_onto_head(
        &mut self,
        request: &PickRequest,
    ) -> Result<MergeOutcome, EngineError> {
        let kind = request.kind;
        let id = kind.command();
        let status = self.status_for_write()?;
        check_pick_state(&status, kind)?;
        let full = branch_ref(&request.branch);
        if self.head_ref_name()?.as_deref() != Some(full.as_str()) {
            return Err(EngineError::NotAllowed(format!(
                "HEAD è cambiata mentre si preparava `{id}`: niente modificato, riprovare"
            )));
        }
        self.check_branch_value(&full, &request.expected)?;
        // La patch che si applica: dal genitore al commit (cherry-pick) o al contrario.
        let parent = request
            .parent
            .map_or_else(|| self.empty_tree.to_owned(), |p| p.to_string());
        let commit = request.commit.to_string();
        let (from, to) = match kind {
            PickKind::CherryPick => (parent, commit.clone()),
            PickKind::Revert => (commit.clone(), parent),
        };
        self.check_assume_unchanged_between(&from, &to)?;
        self.check_pick_paths(&request.expected.to_string(), &from, &to, id)?;
        let expected = request.expected.to_string();
        let had_msg = self.git_dir.join("MERGE_MSG").exists();
        let backup = self.create_backup_ref(
            kind.git(),
            &expected,
            &format!("project: backup di {full} prima di {id}"),
        )?;
        let reflog = format!("project: {id}");
        let env: [(&str, &OsStr); 2] = [
            ("GIT_REFLOG_ACTION", OsStr::new(&reflog)),
            ("GIT_EDITOR", OsStr::new(":")),
        ];
        let mainline = request.mainline.map(|m| m.to_string());
        let mut args: Vec<&str> = vec![
            "-c",
            "core.fsmonitor=false",
            // `revert.reference=true` cambierebbe il messaggio del revert.
            "-c",
            "revert.reference=false",
            kind.git(),
            "--no-edit",
            "--no-rerere-autoupdate",
        ];
        if let Some(m) = &mainline {
            args.extend(["-m", m.as_str()]);
        }
        args.extend(["--end-of-options", commit.as_str()]);
        let args: Vec<&OsStr> = args.into_iter().map(OsStr::new).collect();
        let out = self.git.output_os_env(Some(&self.workdir), &args, &env)?;
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .trim()
        .to_owned();
        let operation = self.operation();
        if out.status.success() && operation.is_none() {
            let after = self.ref_value(&full)?.ok_or_else(|| EngineError::Git {
                command: kind.git().to_owned(),
                stderr: format!("dopo `{id}` `{full}` non esiste più"),
            })?;
            if after == request.expected {
                return Err(EngineError::Git {
                    command: kind.git().to_owned(),
                    stderr: format!(
                        "`git {}` è terminato senza errori ma `{full}` non si è spostato \
                         ({text}): controllare indice e worktree; il branch com'era è in \
                         `{backup}`",
                        kind.git()
                    ),
                });
            }
            let mut warnings = Vec::new();
            if self.head_ref_name()?.as_deref() != Some(full.as_str()) {
                warnings.push(format!(
                    "dopo `{id}` HEAD non è più su `{full}`: un hook o un altro processo l'ha \
                     cambiata; controllare HEAD prima di continuare"
                ));
            }
            return Ok(MergeOutcome {
                after,
                backup,
                in_progress: false,
                conflicts: Vec::new(),
                message: String::new(),
                warnings,
            });
        }
        if operation == Some(kind.operation()) {
            let conflicts = self.stopped_conflicts(&text);
            if conflicts.is_empty() && says_empty(&text) && self.index_matches_head()? {
                // Vuoto: git lascia l'operazione in corso senza niente da committare. Si salta
                // il commit (`--skip`: con un commit solo riporta a HEAD indice e worktree, che
                // erano puliti) e niente resta in corso.
                return self.skip_empty(request, &full, &backup, had_msg);
            }
            return Ok(MergeOutcome {
                after: request.expected,
                backup,
                in_progress: true,
                conflicts,
                message: text,
                warnings: Vec::new(),
            });
        }
        // Niente in corso: niente è partito, o il risultato era vuoto (il revert di un commit
        // già annullato esce così).
        let unchanged = operation.is_none();
        if unchanged {
            self.drop_merge_backup(&backup, &full, &request.expected);
            if !had_msg {
                self.remove_stale_merge_msg();
            }
        }
        if unchanged && says_empty(&text) {
            return Err(pick_empty_error(kind, &request.commit));
        }
        if let Some(error) = overwrite_error(&text, &format!("`{id}`")) {
            return Err(error);
        }
        Err(EngineError::Git {
            command: kind.git().to_owned(),
            stderr: if unchanged {
                text
            } else {
                format!("{text}\n(il branch com'era è in `{backup}`)")
            },
        })
    }

    /// Vero se l'indice è uguale a HEAD: il testo `says_empty` può venire anche da un hook, e
    /// `--skip` butterebbe via un risultato in stage.
    fn index_matches_head(&self) -> Result<bool, EngineError> {
        let out = self.git.output(
            Some(&self.workdir),
            &["diff-index", "--cached", "--quiet", "HEAD", "--"],
        )?;
        Ok(out.status.success())
    }

    /// Il risultato vuoto rimasto in corso: `git <cmd> --skip`, poi
    /// [`pick_empty_error`] con il backup tolto; se git non lo chiude, errore che nomina il
    /// backup (che resta).
    fn skip_empty(
        &mut self,
        request: &PickRequest,
        full: &str,
        backup: &str,
        had_msg: bool,
    ) -> Result<MergeOutcome, EngineError> {
        let kind = request.kind;
        let reflog = format!("project: {}", kind.command());
        let env = [("GIT_REFLOG_ACTION", OsStr::new(&reflog))];
        let args = ["-c", "core.fsmonitor=false", kind.git(), "--skip"];
        let out = self
            .git
            .output_with_input_env(Some(&self.workdir), &args, b"", &env)?;
        if !out.status.success() || self.operation().is_some() {
            return Err(EngineError::Git {
                command: kind.git().to_owned(),
                stderr: format!(
                    "`{}` di {} non cambierebbe niente, ma `git {} --skip` non ha chiuso \
                     l'operazione ({}): rileggere lo stato; il branch com'era è in `{backup}`",
                    kind.command(),
                    request.commit,
                    kind.git(),
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            });
        }
        self.drop_merge_backup(backup, full, &request.expected);
        if !had_msg {
            self.remove_stale_merge_msg();
        }
        Err(pick_empty_error(kind, &request.commit))
    }

    /// Toglie `MERGE_MSG` lasciato da un cherry-pick o revert che non è partito o era vuoto
    /// (git lo lascia anche senza operazione in corso, e un `git commit` successivo lo
    /// proporrebbe come messaggio). Solo senza operazione in corso.
    fn remove_stale_merge_msg(&self) {
        if self.operation().is_none() {
            let _ = std::fs::remove_file(self.git_dir.join("MERGE_MSG"));
        }
    }

    /// I file che la patch `from` → `to` scrive e che HEAD (`head`) non ha non devono esistere
    /// nel worktree (non tracciati o ignorati): git li sovrascriverebbe.
    fn check_pick_paths(
        &self,
        head: &str,
        from: &str,
        to: &str,
        what: &str,
    ) -> Result<(), EngineError> {
        let diff = |a: &str, b: &str, filter: &str| -> Result<Vec<Vec<u8>>, EngineError> {
            let args = [
                "diff-tree",
                "-r",
                "-z",
                "--name-only",
                "--no-renames",
                filter,
                "--end-of-options",
                a,
                b,
            ];
            Ok(self
                .run(&args)?
                .split(|&b| b == 0)
                .filter(|p| !p.is_empty())
                .map(<[u8]>::to_vec)
                .collect())
        };
        let written = diff(from, to, "--diff-filter=AMT")?;
        if written.is_empty() {
            return Ok(());
        }
        let not_in_head = diff(head, to, "--diff-filter=A")?;
        let paths: Vec<Vec<u8>> = written
            .into_iter()
            .filter(|p| not_in_head.contains(p))
            .collect();
        self.check_paths_free(&paths, &format!("`{what}`"))
    }
}
