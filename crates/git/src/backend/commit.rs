//! `commit.create` sul [`CliBackend`]: `git commit` con il messaggio su stdin.
//!
//! Hook (`pre-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit`, `post-rewrite`) e
//! firma (`commit.gpgSign`, `gpg.format`, `user.signingKey`) seguono la configurazione
//! dell'utente: mai `--no-verify` né `--no-gpg-sign`. La pulizia del messaggio è quella di
//! `commit.cleanup` (senza configurazione, con `-F` git usa `whitespace`: le righe `#`
//! restano). Nessun editor: `--no-edit` e `GIT_EDITOR=:` (`:` è il valore con cui git non
//! avvia nessun editor), così né `core.editor` né `commit.verbose`, `commit.template` o
//! `commit.status` possono aprire un editor che aspetterebbe un terminale che non c'è (con `-F`
//! il template è comunque ignorato).
//!
//! Un hook o la firma possono essere lenti: il comando resta sincrono come gli altri (§6) e
//! stdout e stderr si leggono insieme mentre stdin si scrive da un altro thread, quindi un
//! hook che stampa molto non blocca git. Non c'è un timeout: una pinentry grafica di gpg (o la
//! conferma di una chiave ssh) aspetta legittimamente l'utente; senza terminale né pinentry
//! grafica gpg fallisce subito (stdin di git è una pipe, `GIT_TERMINAL_PROMPT=0`).

use super::CliBackend;
use crate::command::{failure, stderr_text};
use project_domain::{Commit, LOG_ARGS, Oid, parse_log};
use project_engine::{
    CommitOutcome, CommitRequest, EngineError, check_amend_published, check_commit_state,
};
use std::ffi::OsStr;

impl CliBackend {
    /// [`GitBackend::commit`](project_engine::GitBackend::commit).
    pub(super) fn create_commit(
        &mut self,
        request: &CommitRequest,
    ) -> Result<CommitOutcome, EngineError> {
        request.check_message()?;
        let status = self.status_for_write()?;
        let previous = self.resolve("HEAD")?;
        if previous.is_none() {
            // HEAD che non si risolve: va bene solo per un branch senza commit, non per un
            // ref rotto (sarebbe un commit radice al posto della storia).
            self.index_source()?;
        }
        check_commit_state(&status, previous.is_some(), request.amend)?;
        let branch = self.head_ref_name()?;

        let mut backup = None;
        if request.amend
            && let Some(old) = previous
        {
            check_amend_published(request, &self.remotes_containing(&old)?)?;
            // Il commit riscritto resta nel reflog, ma il reflog scade: un ref lo tiene.
            let reflog = format!("project: backup di {old} prima di commit.create con amend");
            backup = Some(self.create_backup_ref("amend", &old.to_string(), &reflog)?);
        }

        let mut args = vec![
            // Il messaggio arriva da JSON, quindi è sempre UTF-8: un `i18n.commitEncoding`
            // diverso lo etichetterebbe con la codifica sbagliata.
            "-c",
            "i18n.commitEncoding=UTF-8",
            "commit",
            "--no-edit",
            "-F",
            "-",
        ];
        if request.amend {
            args.push("--amend");
        }
        let env = [("GIT_EDITOR", OsStr::new(":"))];
        let out = self.git.output_with_input_env(
            Some(&self.workdir),
            &args,
            request.message.as_bytes(),
            &env,
        )?;
        if !out.status.success() {
            // Niente committato: il backup del commit riscritto non serve (HEAD è ancora lì).
            if let (Some(name), Some(old)) = (&backup, previous)
                && self.resolve("HEAD")? == Some(old)
            {
                let old = old.to_string();
                let _ = self
                    .git
                    .output(Some(&self.workdir), &["update-ref", "-d", name, &old]);
            }
            return Err(commit_failure(&args, &out));
        }

        let head = self.resolve("HEAD")?.ok_or_else(|| EngineError::Git {
            command: "commit".to_owned(),
            stderr: "dopo il commit HEAD non punta a un commit".to_owned(),
        })?;
        let commit = self.read_commit(&head)?;
        // HEAD deve essere il commit appena creato sopra quello letto prima: se un altro
        // processo (o un hook `post-commit`) lo ha spostato, lo si dice invece di restituire
        // valori prima/dopo sbagliati.
        let expected_parents = match (request.amend, previous) {
            (true, Some(old)) => self.read_commit(&old)?.parents,
            (_, previous) => previous.into_iter().collect(),
        };
        let parents_match = if request.amend {
            commit.parents == expected_parents
        } else {
            // Un commit di merge ha in più `MERGE_HEAD`.
            commit.parents.first() == expected_parents.first()
        };
        if Some(head) == previous || !parents_match {
            return Err(EngineError::Git {
                command: "commit".to_owned(),
                stderr: format!(
                    "il commit è stato creato, ma HEAD ({head}) non è il commit atteso sopra {}: \
                     un altro processo git o un hook lo ha spostato. Controllare il log",
                    previous.map_or_else(|| "nessun commit".to_owned(), |p| p.to_string())
                ),
            });
        }
        Ok(CommitOutcome {
            commit,
            previous,
            branch,
            backup,
        })
    }

    /// Il nome completo del ref a cui punta HEAD (es. `refs/heads/main`), `None` se HEAD è
    /// staccata.
    fn head_ref_name(&self) -> Result<Option<String>, EngineError> {
        let args = ["symbolic-ref", "--quiet", "HEAD"];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            let name = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
            return Ok(Some(String::from_utf8_lossy(name).into_owned()));
        }
        // Codice 1 senza messaggi: HEAD staccata.
        if out.status.code() == Some(1) && out.stderr.is_empty() {
            Ok(None)
        } else {
            Err(failure(&args, &out))
        }
    }

    /// I branch remoti (`refs/remotes/`, senza i ref simbolici come `origin/HEAD`) da cui il
    /// commit `id` è raggiungibile.
    fn remotes_containing(&self, id: &Oid) -> Result<Vec<String>, EngineError> {
        let id = id.to_string();
        let args = [
            "for-each-ref",
            "--format=%(refname)%00%(symref)",
            "--contains",
            &id,
            "refs/remotes/",
        ];
        let out = self.run(&args)?;
        Ok(out
            .split(|&b| b == b'\n')
            .filter_map(|line| {
                let mut fields = line.split(|&b| b == 0);
                let name = fields.next().filter(|n| !n.is_empty())?;
                let symref = fields.next().unwrap_or_default();
                symref
                    .is_empty()
                    .then(|| String::from_utf8_lossy(name).into_owned())
            })
            .collect())
    }

    /// Il commit `id`, letto come in `repo.log`.
    fn read_commit(&self, id: &Oid) -> Result<Commit, EngineError> {
        let hex = id.to_string();
        let mut args = vec!["log"];
        args.extend_from_slice(LOG_ARGS);
        args.extend(["--max-count=1", "--end-of-options", &hex, "--"]);
        parse_log(&self.run(&args)?)?
            .into_iter()
            .next()
            .ok_or_else(|| EngineError::Git {
                command: "log".to_owned(),
                stderr: format!("nessun commit letto per `{hex}`"),
            })
    }
}

/// L'errore di un `git commit` non riuscito: indice bloccato → [`EngineError::IndexLocked`];
/// messaggio reso vuoto dalla pulizia di `commit.cleanup` → [`EngineError::InvalidParams`];
/// altrimenti [`EngineError::Git`] con stderr e stdout di git **e degli hook** (git manda
/// l'output degli hook su stderr), così chi chiama vede perché un hook o la firma hanno
/// rifiutato il commit.
fn commit_failure(args: &[&str], out: &std::process::Output) -> EngineError {
    let locked = failure(args, out);
    if matches!(locked, EngineError::IndexLocked { .. }) {
        return locked;
    }
    let stderr = stderr_text(out);
    if stderr.contains("Aborting commit due to empty commit message") {
        return EngineError::empty_commit_message();
    }
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let text = match (stderr.is_empty(), stdout.is_empty()) {
        (true, true) => format!(
            "terminato con {} senza messaggi (un hook che rifiuta senza scrivere niente?)",
            out.status
        ),
        (false, true) => stderr,
        (true, false) => stdout,
        (false, false) => format!("{stderr}\n{stdout}"),
    };
    EngineError::Git {
        command: "commit".to_owned(),
        stderr: text,
    }
}
