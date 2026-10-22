//! Pull sul [`CliBackend`] (`remote.pull`, S26) e il suo undo/redo, secondo §5 e §7.
//!
//! Il fetch lo fa il comando prima, con [`GitBackend::fetch`](project_engine::GitBackend::fetch);
//! qui solo l'integrazione in locale, che non si annulla a metà:
//!
//! - fast-forward del branch attivo: `git -c core.fsmonitor=false merge --ff-only
//!   --no-autostash --no-edit --end-of-options <oid>`: git rifiuta da sé (prima di scrivere) di
//!   sovrascrivere modifiche locali e file non tracciati; di un altro branch: `update-ref -m ...
//!   refs/heads/<b> <nuovo> <vecchio>` (worktree non toccato), rifiutato se il branch è aperto
//!   in un altro worktree;
//! - merge: `git -c core.fsmonitor=false merge --no-autostash --no-edit --commit --no-squash
//!   [--ff|--no-ff] -F <file> --end-of-options <oid>`: il messaggio da un file temporaneo nella
//!   cartella git (git non legge `-F -` da stdin per `merge`), `--ff`/`--no-ff`, `--commit` e
//!   `--no-squash` espliciti per non dipendere da `merge.ff` e `branch.<nome>.mergeOptions`. Hook (`pre-merge-commit`, `commit-msg`) e firma come dice la configurazione;
//! - rebase: `git -c core.fsmonitor=false -c rebase.updateRefs=false rebase --no-autostash
//!   [--rebase-merges] --end-of-options <oid>` (`rebase.updateRefs` sposterebbe altri branch
//!   fuori dal diario), con `GIT_EDITOR=:` e `GIT_SEQUENCE_EDITOR=:`. Rebase sull'upstream
//!   come `git rebase <upstream>`, senza il fork-point di `git pull --rebase`: se l'upstream è
//!   stato riscritto (force push) si riapplicano anche i suoi commit vecchi, e di solito il
//!   rebase si ferma in conflitto (e si annulla);
//! - conflitto: i file in conflitto dallo stato, poi `git merge --abort` / `git rebase --abort`
//!   (il worktree era pulito: torna esattamente com'era) e [`EngineError::PullConflict`]. Se
//!   l'annullamento non riesce, l'errore lo dice (il repo resta nel merge/rebase in corso).
//!
//! Prima di tutto il backup `refs/project/backup/pull-...` sul branch com'era; se poi niente
//! cambia (errore, conflitto) si toglie. `GIT_REFLOG_ACTION=project: remote.pull` dà il
//! messaggio del reflog.
//!
//! Undo/redo del branch attivo ([`CliBackend::reset_keep`]): gli stessi controlli di
//! `restore_refs` e i backup dei commit abbandonati, poi `git -c core.fsmonitor=false reset
//! --keep <oid>` (aggiorna indice e worktree dei file diversi fra i due commit, rifiuta se uno
//! di loro ha modifiche locali, non sovrascrive i non tracciati); prima si controlla che nessun
//! file da ricreare esista già nel worktree (`reset --keep` sovrascriverebbe un file ignorato).
//! Resta l'istante fra il controllo dei ref e `reset`: `reset` aggiorna il branch con il valore
//! letto da sé, non con quello atteso.

use super::CliBackend;
use crate::command::{failure, os_from_bytes};
use project_domain::{Commit, EntryKind, LOG_ARGS, Oid, Operation, parse_log};
use project_engine::{
    EngineError, HeadState, PullMode, PullOperation, PullOutcome, PullRequest, PullSettings,
    RefRestore, RestoreOutcome, UpstreamConfig, branch_ref, check_pull_state,
};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Un file temporaneo nella cartella git (il messaggio del merge), cancellato all'uscita dallo
/// scope.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Il messaggio del reflog delle scritture di un pull.
const REFLOG_ACTION: &str = "project: remote.pull";

impl CliBackend {
    /// [`GitBackend::upstream`](project_engine::GitBackend::upstream) con `for-each-ref`:
    /// `%(upstream:remotename)` e `%(upstream:remoteref)` sono `branch.<nome>.remote` e
    /// `.merge`, `%(upstream)` il branch remoto locale secondo il refspec del remoto.
    pub(super) fn upstream_of(&self, branch: &str) -> Result<Option<UpstreamConfig>, EngineError> {
        let full = branch_ref(branch);
        let args = [
            "for-each-ref",
            "--format=%(refname)%00%(upstream)%00%(upstream:remotename)%00%(upstream:remoteref)",
            &full,
        ];
        let out = self.run(&args)?;
        for line in out.split(|&b| b == b'\n') {
            let fields: Vec<&[u8]> = line.split(|&b| b == 0).collect();
            let [name, tracking, remote, merge] = fields[..] else {
                continue;
            };
            if name != full.as_bytes() {
                continue;
            }
            if remote.is_empty() || merge.is_empty() {
                return Ok(None);
            }
            let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
            let tracking = text(tracking);
            let remote = text(remote);
            return Ok(Some(UpstreamConfig {
                tracking: (!tracking.is_empty() && remote != ".").then_some(tracking),
                remote,
                merge: text(merge),
            }));
        }
        Ok(None)
    }

    /// Il valore di `key` nella configurazione (tutte le fonti, come la legge git), `None` se
    /// non c'è.
    fn config_value(&self, key: &str) -> Result<Option<String>, EngineError> {
        let args = ["config", "--get", "--end-of-options", key];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.code() == Some(1) && out.stderr.is_empty() {
            return Ok(None);
        }
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        let value = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
        Ok(Some(String::from_utf8_lossy(value).into_owned()))
    }

    /// [`GitBackend::config_values`](project_engine::GitBackend::config_values): `git config
    /// -z --get-all`, valori non UTF-8 convertiti con `from_utf8_lossy`.
    pub(super) fn config_all(&self, key: &str) -> Result<Vec<String>, EngineError> {
        let args = ["config", "-z", "--get-all", "--end-of-options", key];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.code() == Some(1) && out.stderr.is_empty() {
            return Ok(Vec::new());
        }
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        Ok(out
            .stdout
            .split(|&b| b == 0)
            .filter(|v| !v.is_empty())
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .collect())
    }

    /// Prima di integrare l'upstream nel branch attivo: i file che il pull porterebbe nel
    /// worktree e che HEAD non ha (`diff-tree --diff-filter=A HEAD <upstream>`) non devono
    /// esistere nel worktree, né come file né al posto di una loro cartella: sarebbero non
    /// tracciati o ignorati, e git sovrascrive (o, con un merge annullato, cancella) gli
    /// ignorati senza dirlo. Il rebase non ha `--no-overwrite-ignore`; per fast-forward e merge
    /// è una seconda difesa. Altrimenti [`EngineError::NotAllowed`] con i file.
    ///
    /// Con `replay` (rebase) contano anche i file toccati da ognuno dei commit da riapplicare
    /// (`git log -m --no-renames --name-only <upstream>..HEAD`, merge compresi per
    /// `--rebase-merges`) che HEAD non ha: il rebase passa per gli alberi intermedi, e un file
    /// aggiunto da un commit e tolto da uno successivo (o rinominato, o tolto e ricreato), o
    /// modificato da un commit locale ma tolto dall'upstream (conflitto modify/delete, poi
    /// `rebase --abort`), verrebbe scritto e poi cancellato, portandosi via quello ignorato al
    /// suo posto.
    fn check_incoming_paths(
        &self,
        head: &Oid,
        upstream: &Oid,
        replay: bool,
    ) -> Result<(), EngineError> {
        let (from, to) = (head.to_string(), upstream.to_string());
        let args = [
            "diff-tree",
            "-r",
            "-z",
            "--name-only",
            "--no-renames",
            "--diff-filter=A",
            "--end-of-options",
            &from,
            &to,
        ];
        let mut paths: Vec<Vec<u8>> = self
            .run(&args)?
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        if replay {
            let range = format!("{to}..{from}");
            let log = [
                "--no-replace-objects",
                "log",
                "-m",
                "--no-renames",
                "--format=",
                "-z",
                "--name-only",
                "--end-of-options",
                &range,
                "--",
            ];
            let tracked = self.run(&[
                "ls-tree",
                "-r",
                "-z",
                "--name-only",
                "--end-of-options",
                &from,
            ])?;
            let tracked: std::collections::BTreeSet<&[u8]> = tracked
                .split(|&b| b == 0)
                .filter(|p| !p.is_empty())
                .collect();
            for path in self.run(&log)?.split(|&b| b == 0) {
                if !path.is_empty() && !tracked.contains(path) && !paths.iter().any(|p| p == path) {
                    paths.push(path.to_vec());
                }
            }
        }
        let mut hit = Vec::new();
        for path in &paths {
            let path = path.as_slice();
            let blocked = std::fs::symlink_metadata(self.workdir.join(os_from_bytes(path))).is_ok()
                || path
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| **b == b'/')
                    .any(|(i, _)| {
                        std::fs::symlink_metadata(self.workdir.join(os_from_bytes(&path[..i])))
                            .is_ok_and(|m| !m.is_dir())
                    });
            if blocked {
                hit.push(format!("`{}`", String::from_utf8_lossy(path)));
            }
        }
        if hit.is_empty() {
            return Ok(());
        }
        Err(EngineError::NotAllowed(format!(
            "il pull porterebbe file che ora esistono nel worktree come non tracciati o ignorati \
             ({}): git li sovrascriverebbe. Il branch non è stato toccato; spostarli o \
             rinominarli e riprovare",
            hit.join(", ")
        )))
    }

    /// [`GitBackend::pull_settings`](project_engine::GitBackend::pull_settings).
    pub(super) fn read_pull_settings(&self, branch: &str) -> Result<PullSettings, EngineError> {
        let rebase = match self.config_value(&format!("branch.{branch}.rebase"))? {
            Some(value) => Some(value),
            None => self.config_value("pull.rebase")?,
        };
        Ok(PullSettings {
            rebase,
            ff: self.config_value("pull.ff")?,
        })
    }

    /// [`GitBackend::missing_commits`](project_engine::GitBackend::missing_commits).
    pub(super) fn commits_missing(
        &self,
        tip: &Oid,
        exclude: &[Oid],
        limit: usize,
    ) -> Result<Vec<Commit>, EngineError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let max = format!("--max-count={limit}");
        let revs: Vec<String> = std::iter::once(tip.to_string())
            .chain(exclude.iter().map(|o| format!("^{o}")))
            .collect();
        let mut args = vec!["--no-replace-objects", "log"];
        args.extend_from_slice(LOG_ARGS);
        args.extend(["--date-order", &max, "--end-of-options"]);
        args.extend(revs.iter().map(String::as_str));
        args.push("--");
        Ok(parse_log(&self.run(&args)?)?)
    }

    /// Toglie il backup `name` (solo se punta ancora a `oid`): il pull non ha cambiato niente.
    fn drop_backup(&self, name: &str, oid: &Oid) {
        let oid = oid.to_string();
        let _ = self
            .git
            .output(Some(&self.workdir), &["update-ref", "-d", name, &oid]);
    }

    /// [`GitBackend::pull`](project_engine::GitBackend::pull).
    pub(super) fn pull_upstream(
        &mut self,
        request: &PullRequest,
    ) -> Result<PullOutcome, EngineError> {
        let status = self.status_for_write()?;
        check_pull_state(&status, &request.mode)?;
        let full = branch_ref(&request.branch);
        let on_head = self.head_ref_name()?.as_deref() == Some(full.as_str());
        if on_head != request.current {
            return Err(EngineError::NotAllowed(
                "HEAD è cambiata mentre si preparava il pull: niente modificato, riprovare"
                    .to_owned(),
            ));
        }
        if !request.current {
            if request.mode != PullMode::FastForward {
                return Err(EngineError::NotAllowed(format!(
                    "`{}` non è il branch attivo: si può solo fare fast-forward, niente \
                     modificato",
                    request.branch.escape_debug()
                )));
            }
            if let Some(path) = self.checked_out_elsewhere(&full)? {
                return Err(EngineError::NotAllowed(format!(
                    "`{}` è il branch attivo del worktree `{}`: il pull va fatto lì, niente \
                     modificato",
                    request.branch.escape_debug(),
                    path.display()
                )));
            }
        }
        self.check_branch_value(&full, &request.expected)?;
        if request.current {
            self.check_assume_unchanged(&Some(request.expected), &request.upstream)?;
            let replay = matches!(request.mode, PullMode::Rebase { .. });
            self.check_incoming_paths(&request.expected, &request.upstream, replay)?;
        }
        let expected = request.expected.to_string();
        let upstream = request.upstream.to_string();
        let backup = self.create_backup_ref(
            "pull",
            &expected,
            &format!("project: backup di {full} prima di remote.pull"),
        )?;
        let result = self.integrate(request, &full, &expected, &upstream);
        match result {
            Ok(()) => {}
            Err(e) => {
                // Niente cambiato: il backup non serve (si toglie solo se il branch è com'era).
                if self.ref_value(&full)? == Some(request.expected) {
                    self.drop_backup(&backup, &request.expected);
                }
                return Err(e);
            }
        }
        let after = self.ref_value(&full)?.ok_or_else(|| EngineError::Git {
            command: "merge".to_owned(),
            stderr: format!("dopo il pull `{full}` non esiste più"),
        })?;
        let mut warnings = Vec::new();
        if request.current && self.head_ref_name()?.as_deref() != Some(full.as_str()) {
            warnings.push(format!(
                "dopo il pull HEAD non è più su `{full}`: un hook o un altro processo l'ha \
                 cambiata; controllare HEAD prima di continuare"
            ));
        }
        if !self.is_ancestor(&request.upstream, &after)? {
            warnings.push(format!(
                "dopo il pull `{full}` ({after}) non contiene `{}`: un hook o un altro processo \
                 ha spostato il branch durante l'operazione",
                request.upstream_name
            ));
        }
        Ok(PullOutcome {
            after,
            backup,
            warnings,
        })
    }

    /// L'integrazione vera e propria (vedi il modulo); `Ok` se il branch è stato spostato.
    fn integrate(
        &self,
        request: &PullRequest,
        full: &str,
        expected: &str,
        upstream: &str,
    ) -> Result<(), EngineError> {
        let env: [(&str, &OsStr); 3] = [
            ("GIT_REFLOG_ACTION", OsStr::new(REFLOG_ACTION)),
            ("GIT_EDITOR", OsStr::new(":")),
            ("GIT_SEQUENCE_EDITOR", OsStr::new(":")),
        ];
        let base = ["-c", "core.fsmonitor=false"];
        match (request.mode, request.current) {
            (PullMode::FastForward, false) => {
                let reflog = format!("{REFLOG_ACTION}: fast-forward su {upstream}");
                let args = ["update-ref", "-m", &reflog, full, upstream, expected];
                let out = self.git.output(Some(&self.workdir), &args)?;
                if !out.status.success() {
                    self.check_branch_value(full, &request.expected)?;
                    return Err(failure(&args, &out));
                }
                Ok(())
            }
            (PullMode::FastForward, true) => {
                let args = [
                    &base[..],
                    &[
                        "merge",
                        "--ff-only",
                        "--no-autostash",
                        "--no-overwrite-ignore",
                        "--no-edit",
                        "--end-of-options",
                        upstream,
                    ],
                ]
                .concat();
                let out = self.run_input(&args, b"", &env).err();
                match out {
                    None => Ok(()),
                    Some(EngineError::Git { stderr, .. }) => Err(overwrite_error(&stderr)
                        .unwrap_or(EngineError::Git {
                            command: "merge".to_owned(),
                            stderr,
                        })),
                    Some(other) => Err(other),
                }
            }
            (PullMode::Merge { no_ff }, _) => {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos());
                let file = TempFile(
                    self.git_dir
                        .join(format!("project-merge-msg-{}-{nanos}", std::process::id())),
                );
                std::fs::write(&file.0, request.message.as_bytes()).map_err(|e| {
                    EngineError::Git {
                        command: "merge".to_owned(),
                        stderr: format!("messaggio del merge non scritto: {e}"),
                    }
                })?;
                let mut args: Vec<&OsStr> = [
                    "-c",
                    "core.fsmonitor=false",
                    "merge",
                    "--no-autostash",
                    "--no-overwrite-ignore",
                    "--no-edit",
                    "--commit",
                    "--no-squash",
                    if no_ff { "--no-ff" } else { "--ff" },
                    "-F",
                ]
                .into_iter()
                .map(OsStr::new)
                .collect();
                // Il percorso come `OsStr`: la cartella git può non essere UTF-8.
                args.push(file.0.as_os_str());
                args.extend(["--end-of-options", upstream].map(OsStr::new));
                self.run_stopping(request, &args, &env, PullOperation::Merge)
            }
            (PullMode::Rebase { merges }, _) => {
                let mut args = vec![
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "rebase.updateRefs=false",
                    "rebase",
                    "--no-autostash",
                    "--no-autosquash",
                ];
                if merges {
                    args.push("--rebase-merges");
                }
                args.extend(["--end-of-options", upstream]);
                let args: Vec<&OsStr> = args.into_iter().map(OsStr::new).collect();
                self.run_stopping(request, &args, &env, PullOperation::Rebase)
            }
        }
    }

    /// Esegue un merge o un rebase; se si ferma (conflitto o altro) lo annulla e restituisce
    /// l'errore: [`EngineError::PullConflict`] con i file in conflitto, altrimenti quello di git.
    fn run_stopping(
        &self,
        request: &PullRequest,
        args: &[&OsStr],
        env: &[(&str, &OsStr)],
        operation: PullOperation,
    ) -> Result<(), EngineError> {
        let out = self.git.output_os_env(Some(&self.workdir), args, env)?;
        if out.status.success() && self.operation().is_none() {
            return Ok(());
        }
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .trim()
        .to_owned();
        let in_progress = matches!(
            (operation, self.operation()),
            (PullOperation::Merge, Some(Operation::Merge))
                | (PullOperation::Rebase, Some(Operation::Rebase))
        );
        if !in_progress {
            // Niente è partito (es. un file non tracciato che verrebbe sovrascritto): git non ha
            // scritto niente.
            if let Some(error) = overwrite_error(&text) {
                return Err(error);
            }
            return Err(EngineError::Git {
                command: operation.to_string(),
                stderr: text,
            });
        }
        // I file in conflitto: quelli ancora non uniti e quelli che git ha detto in conflitto
        // (anche se `rerere.autoUpdate` li ha già risolti e messi in stage con una risoluzione
        // registrata: il merge/rebase è comunque fermo e va concluso a mano).
        let mut files: Vec<String> = self
            .status_for_write()
            .map(|s| {
                s.entries
                    .iter()
                    .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
                    .map(|e| e.path.to_string())
                    .collect()
            })
            .unwrap_or_default();
        for file in conflicted_in_output(&text) {
            if !files.contains(&file) {
                files.push(file);
            }
        }
        let abort = [operation.to_string(), "--abort".to_owned()];
        let abort: Vec<&str> = abort.iter().map(String::as_str).collect();
        let aborted = self
            .git
            .output_with_input_env(Some(&self.workdir), &abort, b"", env)?;
        if !aborted.status.success() || self.operation().is_some() {
            return Err(EngineError::Git {
                command: operation.to_string(),
                stderr: format!(
                    "il {operation} dell'upstream si è fermato ({text}) e `git {operation} \
                     --abort` non è riuscito ({}): il repository è ancora nel {operation} in \
                     corso. Il branch com'era è in `refs/project/backup/pull-...`",
                    String::from_utf8_lossy(&aborted.stderr).trim()
                ),
            });
        }
        if files.is_empty() {
            // Fermo per un altro motivo (hook, firma, commit vuoto): annullato.
            return Err(EngineError::Git {
                command: operation.to_string(),
                stderr: format!("{text}\n(il {operation} è stato annullato, niente modificato)"),
            });
        }
        Err(EngineError::PullConflict {
            branch: request.branch.clone(),
            upstream: request
                .upstream_name
                .strip_prefix("refs/remotes/")
                .unwrap_or(&request.upstream_name)
                .to_owned(),
            operation,
            files,
            message: text,
        })
    }

    /// [`GitBackend::reset_branch`](project_engine::GitBackend::reset_branch).
    pub(super) fn reset_keep(
        &mut self,
        request: &RefRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        request.check(self.operation())?;
        let (name, expected, target) = match (request.refs.as_slice(), &request.head_expected) {
            ([r], HeadState::Branch(b)) if r.name == *b => match (r.expected, r.target) {
                (Some(e), Some(t)) => (r.name.clone(), e, t),
                _ => return Err(invalid(request)),
            },
            _ => return Err(invalid(request)),
        };
        self.check_expected(request)?;
        // File che `reset --keep` ricreerebbe (in `target`, non in `expected`): niente deve
        // esserci al loro posto nel worktree (un file ignorato sarebbe sovrascritto).
        let (from, to) = (expected.to_string(), target.to_string());
        let args = [
            "diff-tree",
            "-r",
            "-z",
            "--name-only",
            "--no-renames",
            "--diff-filter=D",
            "--end-of-options",
            &to,
            &from,
        ];
        let out = self.run(&args)?;
        let present: Vec<String> = out
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .filter(|p| std::fs::symlink_metadata(self.workdir.join(os_from_bytes(p))).is_ok())
            .map(|p| format!("`{}`", String::from_utf8_lossy(p)))
            .collect();
        if !present.is_empty() {
            return Err(EngineError::NotAllowed(format!(
                "`{}` ricreerebbe file che ora esistono nel worktree come non tracciati o \
                 ignorati ({}): niente modificato; spostarli e riprovare",
                request.command,
                present.join(", ")
            )));
        }
        self.check_assume_unchanged(&Some(expected), &target)?;
        let backups = self.protect(request)?;
        let env = [("GIT_REFLOG_ACTION", OsStr::new(&request.reflog))];
        let args = [
            "-c",
            "core.fsmonitor=false",
            "reset",
            "--keep",
            "--quiet",
            "--end-of-options",
            &to,
        ];
        let out = self
            .git
            .output_with_input_env(Some(&self.workdir), &args, b"", &env)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            if stderr.contains("would be overwritten") || stderr.contains("not uptodate") {
                return Err(EngineError::NotAllowed(format!(
                    "`{}` riporterebbe `{name}` a {to} ma sovrascriverebbe modifiche locali: \
                     niente modificato; committarle o metterle da parte e riprovare ({})",
                    request.command,
                    stderr.trim()
                )));
            }
            return Err(failure(&args, &out));
        }
        let mut warnings = Vec::new();
        match self.ref_value(&name)? {
            Some(now) if now == target => {}
            now => warnings.push(format!(
                "dopo `{}` `{name}` vale {} invece di {target}: un altro processo l'ha spostato \
                 durante l'operazione",
                request.command,
                now.map_or_else(|| "niente".to_owned(), |o| o.to_string())
            )),
        }
        Ok(RestoreOutcome { backups, warnings })
    }
}

/// I file che l'output di merge/rebase dice in conflitto: `CONFLICT (...): Merge conflict in
/// <file>` e, con `rerere`, `Resolved '<file>' using previous resolution.` / `Staged '<file>'
/// using previous resolution.`.
fn conflicted_in_output(text: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in text.lines() {
        let file = if let Some((_, f)) = line.split_once("Merge conflict in ") {
            Some(f.trim())
        } else {
            ["Resolved '", "Staged '"].iter().find_map(|p| {
                line.strip_prefix(p)?
                    .strip_suffix("' using previous resolution.")
            })
        };
        if let Some(f) = file
            && !files.iter().any(|x| x == f)
        {
            files.push(f.to_owned());
        }
    }
    files
}

fn invalid(request: &RefRestore) -> EngineError {
    EngineError::NotAllowed(format!(
        "`{}`: voce del diario non valida per il branch attivo, niente modificato",
        request.command
    ))
}

/// [`EngineError::NotAllowed`] se git ha rifiutato perché sovrascriverebbe modifiche locali o
/// file non tracciati (prima di scrivere: niente è cambiato), con i file (righe con il tab).
fn overwrite_error(stderr: &str) -> Option<EngineError> {
    if !(stderr.contains("would be overwritten by")
        || stderr.contains("would be removed by")
        || stderr.contains("would lose untracked files"))
    {
        return None;
    }
    let files: Vec<String> = stderr
        .lines()
        .filter_map(|l| l.strip_prefix('\t'))
        .map(|f| format!("`{f}`"))
        .collect();
    Some(EngineError::NotAllowed(format!(
        "il pull sovrascriverebbe modifiche locali o file non tracciati ({}): committarli, \
         metterli da parte o spostarli e riprovare; il branch non è stato toccato",
        files.join(", ")
    )))
}
