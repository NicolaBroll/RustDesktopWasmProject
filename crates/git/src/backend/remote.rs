//! Remoti sul [`CliBackend`] (`remote.fetch`, S25), secondo §5 e §6:
//!
//! - `git fetch --progress --end-of-options <remote> [refs/heads/<branch>]`: il remoto è il nome
//!   di un remoto configurato (controllato dal comando) e, come il branch, arriva dopo
//!   `--end-of-options`, così un remoto che si chiama `-x` o `--upload-pack=...` non diventa
//!   un'opzione. Il branch va come `refs/heads/<branch>`, senza `:` né `+`: nessun ref locale
//!   come destinazione, e il branch remoto si aggiorna con il refspec configurato;
//! - `-c protocol.ext.allow=never -c protocol.fd.allow=never` (anche per il push): un remoto
//!   configurato a mano con `ext::`/`fd::` non esegue comandi, qualunque sia `protocol.allow`;
//! - niente `--prune`, `--force`, `--tags`, `--update-head-ok`: tag e pulizia come dice la
//!   configurazione (`fetch.prune`, `remote.<nome>.tagOpt`, ...);
//! - stdin chiuso, `GIT_TERMINAL_PROMPT=0` e nessun terminale di controllo (`setsid`): il fetch
//!   non aspetta mai una risposta da tastiera (vedi `Git::run_streaming`); credential helper,
//!   `ssh-agent` e `SSH_ASKPASS` funzionano come sempre;
//! - stderr letto mentre git gira: le righe di avanzamento (`\r`) vanno al `JobContext`, le
//!   altre righe nel risultato o nell'errore.
//!
//! Push (`remote.push`, S26): `git push --porcelain --progress --no-follow-tags
//! --recurse-submodules=check [--set-upstream] [--force-with-lease=refs/heads/<dst>:<oid>]
//! --end-of-options <remote> refs/heads/<branch>:refs/heads/<dst>`, con lo stesso ambiente del
//! fetch. Il refspec esplicito pubblica solo quel branch (`push.default`, `remote.<nome>.push`
//! non contano), `--no-follow-tags` neutralizza `push.followTags`, `--recurse-submodules=check`
//! un `push.recurseSubmodules=on-demand` che pubblicherebbe anche i sottomoduli (e rifiuta un
//! push che riferisce commit di sottomoduli non pubblicati). Mai `--force`, `+refspec`,
//! `--mirror`, `--all`, `--tags`, `--no-verify`: gli hook `pre-push` girano. L'esito si legge
//! dallo stdout di `--porcelain` (`<flag>\t<da>:<a>\t<riassunto>`).

use super::CliBackend;
use crate::command::{Segment, failure};
use crate::progress::{parse_progress, remote_error};
use project_engine::{
    EngineError, FetchOutcome, FetchRequest, JobContext, PushKind, PushOutcome, PushRejection,
    PushRequest,
};
use std::process::Output;

impl CliBackend {
    /// I nomi dei remoti (`git remote`, uno per riga), in ordine. Un nome non UTF-8 si salta:
    /// non si potrebbe indicare in JSON.
    pub(super) fn remote_names(&self) -> Result<Vec<String>, EngineError> {
        let out = self.run(&["remote"])?;
        let mut names: Vec<String> = out
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| std::str::from_utf8(line).ok())
            .map(str::to_owned)
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    pub(super) fn fetch_remote(
        &self,
        request: &FetchRequest,
        job: &JobContext,
    ) -> Result<FetchOutcome, EngineError> {
        let refspec = request.branch.as_ref().map(|b| format!("refs/heads/{b}"));
        let mut args = vec![
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "protocol.fd.allow=never",
            "fetch",
            "--progress",
            "--end-of-options",
            &request.remote,
        ];
        args.extend(refspec.as_deref());

        let mut lines = Vec::new();
        let streamed = self.git.run_streaming(
            &self.workdir,
            &args,
            false,
            |Segment { text, newline }| match parse_progress(&text) {
                Some(progress) => job.report(&progress),
                // Una riga riscritta con `\r` che non è avanzamento (raro) non si tiene: la
                // versione finale arriva con `\n`.
                None if newline && !text.trim().is_empty() => {
                    lines.push(text.trim_end().to_owned())
                }
                None => {}
            },
            &|| job.is_cancelled(),
        )?;
        if streamed.cancelled {
            return Err(EngineError::Cancelled {
                command: "remote.fetch",
            });
        }
        if !streamed.status.success() {
            let stderr = lines.join("\n");
            if let Some(error) = remote_error(&request.remote, &stderr) {
                return Err(error);
            }
            // Il testo vuoto lo sostituisce `failure` con il codice di uscita.
            return Err(failure(
                &args,
                &Output {
                    status: streamed.status,
                    stdout: Vec::new(),
                    stderr: stderr.into_bytes(),
                },
            ));
        }
        Ok(FetchOutcome { messages: lines })
    }
}

impl CliBackend {
    /// [`GitBackend::push`](project_engine::GitBackend::push).
    pub(super) fn push_branch(
        &self,
        request: &PushRequest,
        job: &JobContext,
    ) -> Result<PushOutcome, EngineError> {
        let dst = format!("refs/heads/{}", request.remote_branch);
        let refspec = format!("refs/heads/{}:{dst}", request.branch);
        let lease = request.lease.map(|expected| {
            format!(
                "--force-with-lease={dst}:{}",
                expected.map(|o| o.to_string()).unwrap_or_default()
            )
        });
        let mut args = vec![
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "protocol.fd.allow=never",
            "push",
            "--porcelain",
            "--progress",
            "--no-follow-tags",
            "--recurse-submodules=check",
        ];
        if request.set_upstream {
            args.push("--set-upstream");
        }
        args.extend(lease.as_deref());
        args.extend(["--end-of-options", &request.remote, &refspec]);

        // Prima di un force, i commit che il remoto perderebbe (come li conosce il branch
        // remoto locale) in un ref di backup.
        let backup = match request.backup {
            Some(oid) => Some(self.create_backup_ref(
                "push",
                &oid.to_string(),
                &format!(
                    "project: backup di {}/{} prima di un force push",
                    request.remote, request.remote_branch
                ),
            )?),
            None => None,
        };
        let drop_backup = |backup: &Option<String>| {
            if let (Some(name), Some(oid)) = (backup, request.backup) {
                let oid = oid.to_string();
                let _ = self
                    .git
                    .output(Some(&self.workdir), &["update-ref", "-d", name, &oid]);
            }
        };

        let mut lines = Vec::new();
        let streamed = self.git.run_streaming(
            &self.workdir,
            &args,
            true,
            |Segment { text, newline }| match parse_progress(&text) {
                Some(progress) => job.report(&progress),
                None if newline && !text.trim().is_empty() => {
                    lines.push(text.trim_end().to_owned())
                }
                None => {}
            },
            &|| job.is_cancelled(),
        )?;
        if streamed.cancelled {
            // Il remoto potrebbe aver già aggiornato il branch: il backup resta.
            return Err(EngineError::Cancelled {
                command: "remote.push",
            });
        }
        let stdout = String::from_utf8_lossy(&streamed.stdout).into_owned();
        let status = porcelain_status(&stdout, &dst);
        let stderr = lines.join("\n");
        if streamed.status.success()
            && let Some((flag, summary)) = &status
        {
            let kind = match flag {
                '*' => PushKind::New,
                '+' => PushKind::Forced,
                '=' => PushKind::UpToDate,
                _ => PushKind::FastForward,
            };
            if kind != PushKind::Forced {
                drop_backup(&backup);
            }
            let mut messages = lines;
            messages.extend(
                stdout
                    .lines()
                    .filter(|l| l.starts_with("To "))
                    .map(str::to_owned),
            );
            return Ok(PushOutcome {
                kind,
                summary: summary.clone(),
                messages,
                backup: if kind == PushKind::Forced {
                    backup
                } else {
                    None
                },
            });
        }
        drop_backup(&backup);
        let reject = |reason, message: String| EngineError::PushRejected {
            remote: request.remote.clone(),
            branch: request.remote_branch.clone(),
            reason,
            message,
        };
        match status {
            Some(('!', summary)) => {
                let reason = rejection(&summary);
                let message = if stderr.is_empty() {
                    summary
                } else {
                    format!("{summary}\n{stderr}")
                };
                Err(reject(reason, message))
            }
            _ => {
                if let Some(error) = remote_error(&request.remote, &stderr) {
                    return Err(error);
                }
                if stderr.contains("failed to push some refs") {
                    let reason = if stderr.contains("submodule") {
                        PushRejection::Other
                    } else {
                        PushRejection::Hook
                    };
                    return Err(reject(reason, stderr));
                }
                Err(failure(
                    &args,
                    &Output {
                        status: streamed.status,
                        stdout: Vec::new(),
                        stderr: stderr.into_bytes(),
                    },
                ))
            }
        }
    }
}

/// La riga di `git push --porcelain` per il ref `dst` del remoto: il flag (` ` fast-forward,
/// `+` forzato, `*` nuovo, `=` già aggiornato, `!` rifiutato, `-` eliminato) e il riassunto.
fn porcelain_status(stdout: &str, dst: &str) -> Option<(char, String)> {
    stdout.lines().find_map(|line| {
        let mut fields = line.splitn(3, '\t');
        let flag = fields.next()?;
        let refs = fields.next()?;
        let summary = fields.next().unwrap_or_default();
        let (_, to) = refs.rsplit_once(':')?;
        let mut chars = flag.chars();
        let flag = chars.next()?;
        (to == dst && chars.next().is_none()).then(|| (flag, summary.to_owned()))
    })
}

/// Il motivo di un rifiuto dal riassunto di `--porcelain` (`[rejected] (non-fast-forward)`,
/// `[rejected] (stale info)`, `[remote rejected] (pre-receive hook declined)`, ...).
fn rejection(summary: &str) -> PushRejection {
    if summary.contains("non-fast-forward") || summary.contains("fetch first") {
        PushRejection::NonFastForward
    } else if summary.contains("stale info") {
        PushRejection::StaleLease
    } else if summary.contains("remote rejected") || summary.contains("remote failure") {
        PushRejection::RemoteRejected
    } else {
        PushRejection::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_lines() {
        let out = "To ../r.git\n \trefs/heads/main:refs/heads/main\t7291e58..b4f1e36\nDone\n";
        assert_eq!(
            porcelain_status(out, "refs/heads/main"),
            Some((' ', "7291e58..b4f1e36".to_owned()))
        );
        assert_eq!(porcelain_status(out, "refs/heads/x"), None);
        let out = "To r\n!\trefs/heads/a:refs/heads/a\t[rejected] (non-fast-forward)\nDone\n";
        let (flag, summary) = porcelain_status(out, "refs/heads/a").unwrap();
        assert_eq!(
            (flag, rejection(&summary)),
            ('!', PushRejection::NonFastForward)
        );
        assert_eq!(
            rejection("[rejected] (fetch first)"),
            PushRejection::NonFastForward
        );
        assert_eq!(
            rejection("[rejected] (stale info)"),
            PushRejection::StaleLease
        );
        assert_eq!(
            rejection("[remote rejected] (pre-receive hook declined)"),
            PushRejection::RemoteRejected
        );
        let out = "*\trefs/heads/-x:refs/heads/-x\t[new branch]\n";
        assert_eq!(
            porcelain_status(out, "refs/heads/-x"),
            Some(('*', "[new branch]".to_owned()))
        );
    }
}
