//! Remoti sul [`CliBackend`] (`remote.fetch`, S25), secondo §5 e §6:
//!
//! - `git fetch --progress --end-of-options <remote> [refs/heads/<branch>]`: il remoto è il nome
//!   di un remoto configurato (controllato dal comando) e, come il branch, arriva dopo
//!   `--end-of-options`, così un remoto che si chiama `-x` o `--upload-pack=...` non diventa
//!   un'opzione. Il branch va come `refs/heads/<branch>`, senza `:` né `+`: nessun ref locale
//!   come destinazione, e il branch remoto si aggiorna con il refspec configurato;
//! - niente `--prune`, `--force`, `--tags`, `--update-head-ok`: tag e pulizia come dice la
//!   configurazione (`fetch.prune`, `remote.<nome>.tagOpt`, ...);
//! - stdin chiuso, `GIT_TERMINAL_PROMPT=0` e nessun terminale di controllo (`setsid`): il fetch
//!   non aspetta mai una risposta da tastiera (vedi `Git::run_streaming`); credential helper,
//!   `ssh-agent` e `SSH_ASKPASS` funzionano come sempre;
//! - stderr letto mentre git gira: le righe di avanzamento (`\r`) vanno al `JobContext`, le
//!   altre righe nel risultato o nell'errore.

use super::CliBackend;
use crate::command::{Segment, failure};
use crate::progress::{parse_progress, remote_error};
use project_engine::{EngineError, FetchOutcome, FetchRequest, JobContext};
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
        let mut args = vec!["fetch", "--progress", "--end-of-options", &request.remote];
        args.extend(refspec.as_deref());

        let mut lines = Vec::new();
        let streamed = self.git.run_streaming(
            &self.workdir,
            &args,
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
