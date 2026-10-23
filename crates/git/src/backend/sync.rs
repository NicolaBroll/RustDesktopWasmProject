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
//! - merge fermo (conflitti, hook `pre-merge-commit`/`commit-msg`, firma): **resta in corso**
//!   come con `merge.start` ([`PullOutcome::in_progress`] con i file in conflitto dallo stato e
//!   dall'output di git), da concludere con `merge.continue` o annullare con `merge.abort`;
//! - rebase fermo: i file in conflitto, poi `git rebase --abort` (il worktree era pulito: torna
//!   esattamente com'era) e [`EngineError::PullConflict`]. Se l'annullamento non riesce,
//!   l'errore lo dice (il repo resta nel rebase in corso, il backup resta).
//!
//! Prima di tutto il backup `refs/project/backup/pull-...` sul branch com'era; se poi niente
//! cambia (errore, rebase annullato) si toglie; con il merge fermo resta. `GIT_REFLOG_ACTION=project: remote.pull` dà il
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
use project_domain::{Commit, EntryKind, LOG_ARGS, Oid, Operation, RepoPath, parse_log};
use project_engine::{
    EngineError, HeadState, PullMode, PullOperation, PullOutcome, PullRequest, PullSettings,
    RefRestore, RestoreOutcome, UpstreamConfig, branch_ref, check_pull_state,
};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Un file temporaneo nella cartella git (il messaggio del merge), cancellato all'uscita dallo
/// scope.
pub(super) struct TempFile(pub(super) PathBuf);

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
    pub(super) fn check_incoming_paths(
        &self,
        head: &Oid,
        upstream: &Oid,
        replay: bool,
        what: &str,
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
            "{what} porterebbe file che ora esistono nel worktree come non tracciati o ignorati \
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
            self.check_incoming_paths(&request.expected, &request.upstream, replay, "il pull")?;
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
            Ok(None) => {}
            Ok(Some(Stopped { conflicts, message })) => {
                // Merge fermo: resta in corso, il branch non si è mosso e il backup resta. Gli
                // stessi controlli del merge riuscito, come avvisi: il merge in corso deve
                // essere quello dell'upstream su questo branch.
                let mut warnings = Vec::new();
                if self.head_ref_name()?.as_deref() != Some(full.as_str()) {
                    warnings.push(format!(
                        "dopo il pull HEAD non è più su `{full}`: un hook o un altro processo \
                         l'ha cambiata; controllare HEAD prima di continuare il merge"
                    ));
                }
                let now = self.ref_value(&full)?;
                if now != Some(request.expected) {
                    warnings.push(format!(
                        "`{full}` vale {} invece di {}: un hook o un altro processo l'ha \
                         spostato; il branch com'era è in `{backup}`",
                        now.map_or_else(|| "niente".to_owned(), |o| o.to_string()),
                        request.expected
                    ));
                }
                let incoming = self.read_operation_state().incoming;
                if incoming != [request.upstream] {
                    warnings.push(format!(
                        "il merge in corso unisce {} invece di `{}` ({}): un hook o un altro \
                         processo l'ha cambiato; controllarlo prima di continuarlo",
                        if incoming.is_empty() {
                            "niente".to_owned()
                        } else {
                            incoming
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(", ")
                        },
                        request.upstream_name,
                        request.upstream
                    ));
                }
                return Ok(PullOutcome {
                    after: request.expected,
                    backup,
                    in_progress: true,
                    conflicts,
                    message,
                    warnings,
                });
            }
            Err(e) => {
                // Niente cambiato: il backup non serve (si toglie solo se il branch è com'era e
                // non è rimasta un'operazione in corso, es. un `rebase --abort` non riuscito).
                let now = self.ref_value(&full)?;
                let in_progress = self.operation().is_some();
                if now == Some(request.expected) && !in_progress {
                    self.drop_backup(&backup, &request.expected);
                    return Err(e);
                }
                // Qualcosa è cambiato nonostante l'errore: il backup resta e l'errore lo dice;
                // niente nel diario (non è il risultato atteso del pull).
                let note = if in_progress {
                    format!(
                        "il repository è rimasto in un'operazione in corso; il branch com'era è \
                         in `{backup}`"
                    )
                } else {
                    format!(
                        "`{full}` però si è spostato da {} a {}: git ha scritto qualcosa prima \
                         di fallire. Il pull non entra nel diario; il branch com'era è in \
                         `{backup}`",
                        request.expected,
                        now.map_or_else(|| "niente".to_owned(), |o| o.to_string())
                    )
                };
                return Err(with_note(e, &note));
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
            in_progress: false,
            conflicts: Vec::new(),
            message: String::new(),
            warnings,
        })
    }

    /// L'integrazione vera e propria (vedi il modulo): `Ok(None)` se il branch è stato
    /// spostato, `Ok(Some(..))` se il merge è fermo e resta in corso.
    fn integrate(
        &self,
        request: &PullRequest,
        full: &str,
        expected: &str,
        upstream: &str,
    ) -> Result<Option<Stopped>, EngineError> {
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
                Ok(None)
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
                    None => Ok(None),
                    Some(EngineError::Git { stderr, .. }) => Err(overwrite_error(
                        &stderr, "il pull",
                    )
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

    /// Esegue un merge o un rebase. Un merge fermo (conflitto, hook, firma) resta in corso:
    /// `Ok(Some(..))` con i file in conflitto. Un rebase fermo si annulla e l'errore è
    /// [`EngineError::PullConflict`] con i file in conflitto, altrimenti quello di git.
    fn run_stopping(
        &self,
        request: &PullRequest,
        args: &[&OsStr],
        env: &[(&str, &OsStr)],
        operation: PullOperation,
    ) -> Result<Option<Stopped>, EngineError> {
        let out = self.git.output_os_env(Some(&self.workdir), args, env)?;
        if out.status.success() && self.operation().is_none() {
            return Ok(None);
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
            if let Some(error) = overwrite_error(&text, "il pull") {
                return Err(error);
            }
            return Err(EngineError::Git {
                command: operation.to_string(),
                stderr: text,
            });
        }
        let conflicts = self.stopped_conflicts(&text);
        if operation == PullOperation::Merge {
            // Come `merge.start`: il merge resta in corso, si conclude con `merge.continue` o si
            // annulla con `merge.abort` (il worktree era pulito: torna com'era).
            return Ok(Some(Stopped {
                conflicts,
                message: text,
            }));
        }
        let files: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
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
                     corso",
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

    /// I file in conflitto di un merge o rebase fermo: quelli ancora non uniti nello stato e
    /// quelli che git ha detto in conflitto nell'output `text` (anche se `rerere.autoUpdate` li
    /// ha già risolti e messi in stage con una risoluzione registrata: l'operazione è comunque
    /// ferma e va conclusa a mano). Usato anche da `merge.start`.
    pub(super) fn stopped_conflicts(&self, text: &str) -> Vec<RepoPath> {
        let mut files: Vec<RepoPath> = self
            .status_for_write()
            .map(|s| {
                s.entries
                    .iter()
                    .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
                    .map(|e| e.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        add_output_conflicts(&mut files, conflicted_in_output(text));
        files
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

/// Aggiunge a `files` (i percorsi dallo stato, byte esatti) i nomi che git ha detto in conflitto
/// nell'output (testo convertito con `from_utf8_lossy`): solo quelli che nessun percorso dello
/// stato ha già, confrontando il testo convertito. Così un nome non UTF-8 non compare due volte
/// (una con i byte veri, una con i caratteri sostituiti).
fn add_output_conflicts(files: &mut Vec<RepoPath>, output: Vec<String>) {
    for file in output {
        if !files.iter().any(|p| p.to_string() == file) {
            files.push(RepoPath::from(file));
        }
    }
}

/// `e` con in fondo `note` (che dice cosa è rimasto cambiato e dov'è il backup): nel testo di git
/// o nel motivo del rifiuto; gli altri errori diventano [`EngineError::Git`] con il loro testo.
fn with_note(e: EngineError, note: &str) -> EngineError {
    match e {
        EngineError::Git { command, stderr } => EngineError::Git {
            command,
            stderr: format!("{stderr}\n({note})"),
        },
        EngineError::NotAllowed(reason) => EngineError::NotAllowed(format!("{reason} ({note})")),
        other => EngineError::Git {
            command: "remote.pull".to_owned(),
            stderr: format!("{other} ({note})"),
        },
    }
}

/// Un merge del pull fermo e rimasto in corso: i file in conflitto e il testo di git.
struct Stopped {
    conflicts: Vec<RepoPath>,
    message: String,
}

/// I file che l'output di merge/rebase dice in conflitto: `CONFLICT (...): Merge conflict in
/// <file>` e, con `rerere`, `Resolved '<file>' using previous resolution.` / `Staged '<file>'
/// using previous resolution.`.
pub(super) fn conflicted_in_output(text: &str) -> Vec<String> {
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
pub(super) fn overwrite_error(stderr: &str, what: &str) -> Option<EngineError> {
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
        "{what} sovrascriverebbe modifiche locali o file non tracciati ({}): committarli, \
         metterli da parte o spostarli e riprovare; il branch non è stato toccato",
        files.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_utf8_conflict_is_listed_once() {
        let raw = RepoPath::from(&b"caf\xe9.txt"[..]);
        let mut files = vec![raw.clone()];
        let output = conflicted_in_output(&format!(
            "CONFLICT (content): Merge conflict in {raw}\nCONFLICT (content): Merge conflict in \
             b.txt"
        ));
        add_output_conflicts(&mut files, output);
        assert_eq!(files, vec![raw, RepoPath::from("b.txt")]);
    }

    #[test]
    fn the_note_names_the_backup() {
        let e = with_note(
            EngineError::Git {
                command: "rebase".into(),
                stderr: "fermo".into(),
            },
            "backup in `refs/project/backup/pull-1`",
        );
        assert!(e.to_string().contains("refs/project/backup/pull-1"), "{e}");
        let e = with_note(EngineError::NotAllowed("no".into()), "nota");
        assert!(
            matches!(&e, EngineError::NotAllowed(m) if m == "no (nota)"),
            "{e}"
        );
    }
}
