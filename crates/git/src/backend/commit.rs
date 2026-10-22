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
//! grafica gpg fallisce subito (stdin di git è una pipe, `GIT_TERMINAL_PROMPT=0`). Possono
//! invece aspettare: un hook che lascia processi in background con stdout/stderr aperti (il
//! comando finisce quando li chiudono); una firma ssh con passphrase senza agent o un hook che
//! legge `/dev/tty`, sul terminale di controllo se c'è (CLI o MCP avviati da un terminale).
//!
//! Dopo un commit riuscito ogni problema è [`EngineError::PostCommit`], con il commit creato
//! (se HEAD è ancora quello) e il backup: il commit non va ripetuto.

use super::CliBackend;
use crate::command::{failure, stderr_text};
use project_domain::{Commit, LOG_ARGS, Oid, parse_log};
use project_engine::{
    CommitOutcome, CommitRequest, EngineError, amend_impact, check_amend_published,
    check_commit_state,
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
        let mut also_on = Vec::new();
        if request.amend
            && let Some(old) = previous
        {
            let (published, diverging) =
                amend_impact(&self.refs_containing(&old)?, branch.as_deref());
            check_amend_published(request, &published)?;
            also_on = diverging;
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
            let mut err = commit_failure(&args, &out);
            if let (Some(name), Some(old)) = (&backup, previous) {
                if self.resolve("HEAD")? == Some(old) {
                    // Niente committato e HEAD è ancora il commit letto prima: il backup non
                    // serve. `update-ref -d <ref> <vecchio>` toglie il ref solo se punta ancora
                    // a quel commit (resta solo l'istante fra la lettura di HEAD e questo, in
                    // cui un altro processo potrebbe spostare HEAD: il backup sparirebbe ma il
                    // commit sarebbe ancora nel reflog).
                    let old = old.to_string();
                    let _ = self
                        .git
                        .output(Some(&self.workdir), &["update-ref", "-d", name, &old]);
                } else if let EngineError::Git { stderr, .. } = &mut err {
                    // HEAD è cambiato (un hook o un altro processo): il backup resta.
                    stderr.push_str(&format!(
                        "\n\nHEAD non è più {old}: il commit da riscrivere è salvato in `{name}`"
                    ));
                }
            }
            return Err(err);
        }

        // Da qui il commit è fatto: ogni errore è `PostCommit`, con il commit e il backup.
        let post = |commit: Option<Oid>, reason: String| EngineError::PostCommit {
            commit,
            backup: backup.clone(),
            reason,
        };
        let head = self
            .resolve("HEAD")
            .map_err(|e| post(None, format!("HEAD non si rilegge ({e})")))?
            .ok_or_else(|| post(None, "HEAD non punta a un commit".to_owned()))?;
        if request.amend && Some(head) == previous {
            // Stesso albero, messaggio, autore e date (es. due `amend` uguali nello stesso
            // secondo): git ha riscritto il commit identico. Niente è cambiato, il backup punta
            // allo stesso commit e si toglie.
            if let Some(name) = &backup {
                let old = head.to_string();
                let _ = self
                    .git
                    .output(Some(&self.workdir), &["update-ref", "-d", name, &old]);
            }
            let commit = self
                .read_commit(&head)
                .map_err(|e| post(Some(head), format!("il commit non si legge ({e})")))?;
            return Ok(CommitOutcome {
                commit,
                previous,
                branch,
                backup: None,
                also_on,
            });
        }
        // Se HEAD non è il commit creato, quel commit non si conosce: `commit` resta `None` e
        // il motivo nomina l'HEAD attuale.
        self.check_moved_from(branch.as_deref(), head, previous)
            .map_err(|reason| post(None, reason))?;
        let commit = self
            .read_commit(&head)
            .map_err(|e| post(Some(head), format!("il commit non si legge ({e})")))?;
        // Controllo in più sui genitori (vale anche senza reflog).
        let expected_parents = match (request.amend, previous) {
            (true, Some(old)) => {
                self.read_commit(&old)
                    .map_err(|e| {
                        post(
                            Some(head),
                            format!("il commit riscritto non si legge ({e})"),
                        )
                    })?
                    .parents
            }
            (_, previous) => previous.into_iter().collect(),
        };
        let parents_match = if request.amend {
            commit.parents == expected_parents
        } else {
            // Un commit di merge ha in più `MERGE_HEAD`.
            commit.parents.first() == expected_parents.first()
        };
        if Some(head) == previous || !parents_match {
            return Err(post(None, moved_text(previous, head)));
        }
        Ok(CommitOutcome {
            commit,
            previous,
            branch,
            backup,
            also_on,
        })
    }

    /// Dopo il commit: l'ultima voce del reflog del ref aggiornato (`branch`, o HEAD se
    /// staccata) deve portare da `previous` a `head`. Così si scopre anche un altro processo che
    /// ha spostato HEAD fra la lettura di `previous` e `git commit` (git avrebbe riscritto quel
    /// commit, non quello letto e salvato nel backup). Il valore vecchio è `<ref>@{1}`, il nuovo
    /// `<ref>@{0}`; senza un valore precedente (primo commit) `@{1}` non si risolve.
    ///
    /// Limite: senza reflog (`core.logAllRefUpdates=false` e nessun file di log per il ref)
    /// non si può verificare e resta solo il controllo sui genitori.
    fn check_moved_from(
        &self,
        branch: Option<&str>,
        head: Oid,
        previous: Option<Oid>,
    ) -> Result<(), String> {
        let name = branch.unwrap_or("HEAD");
        let at = |n: u8| {
            self.resolve(&format!("{name}@{{{n}}}"))
                .map_err(|e| format!("il reflog di `{name}` non si legge ({e})"))
        };
        let Some(newest) = at(0)? else {
            // Nessun reflog: niente da verificare.
            return Ok(());
        };
        if newest != head || at(1)? != previous {
            return Err(moved_text(previous, head));
        }
        Ok(())
    }

    /// Il nome completo del ref a cui punta HEAD (es. `refs/heads/main`), `None` se HEAD è
    /// staccata.
    pub(super) fn head_ref_name(&self) -> Result<Option<String>, EngineError> {
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

    /// I branch locali, i branch remoti e i tag (senza i ref simbolici come `origin/HEAD`) da
    /// cui il commit `id` è raggiungibile, nomi completi.
    fn refs_containing(&self, id: &Oid) -> Result<Vec<String>, EngineError> {
        let id = id.to_string();
        let args = [
            "for-each-ref",
            "--format=%(refname)%00%(symref)",
            "--contains",
            &id,
            "refs/heads/",
            "refs/remotes/",
            "refs/tags/",
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

/// Il motivo di [`EngineError::PostCommit`] quando HEAD non è il commit atteso.
fn moved_text(previous: Option<Oid>, head: Oid) -> String {
    format!(
        "HEAD ({head}) non è il commit appena creato sopra {}: un altro processo git o un hook \
         (es. `post-commit`) ha spostato HEAD prima o dopo il commit",
        previous.map_or_else(|| "nessun commit".to_owned(), |p| p.to_string())
    )
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
