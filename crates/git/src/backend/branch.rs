//! Branch locali sul [`CliBackend`] (`branch.create`, `branch.checkout`, `branch.delete`,
//! `branch.rename` e i loro undo/redo, S24), secondo §5:
//!
//! - i nomi arrivano a git solo come `refs/heads/<nome>` (`update-ref`) o dopo
//!   `--end-of-options` (`switch`, `branch -m`): un branch che si chiama
//!   `-x` non diventa mai un'opzione;
//! - un nome nuovo lo controlla anche git (`check-ref-format --branch`, che deve restituire
//!   il nome identico: niente espansioni come `@{-1}`);
//! - nessun ref si sovrascrive: creazione con `update-ref <ref> <id> ""` (solo se non esiste),
//!   eliminazione con `update-ref -d <ref> <id>` (solo se vale ancora `<id>`), rinomina con
//!   `git branch -m` senza `-M` (rifiuta se il nome nuovo esiste);
//! - il checkout è `git switch` senza `--force`, `--discard-changes` né `--merge`, con
//!   `--no-overwrite-ignore` (git di suo sovrascrive senza dirlo i file ignorati) e
//!   `--no-guess` (nessun branch creato da un remoto), e `core.fsmonitor=false` (un fsmonitor
//!   non aggiornato farebbe sembrare pulito un file modificato). Git controlla tutto prima di
//!   scrivere: se un file verrebbe sovrascritto non cambia niente.
//!
//! Limiti noti: un file `--assume-unchanged` modificato verrebbe sovrascritto da git senza
//! avviso; il checkout lo rifiuta se il file è diverso fra HEAD e la destinazione. Un branch su
//! cui è fermo un rebase o un bisect in un altro worktree (HEAD staccata lì) non risulta "aperto"
//! e si può eliminare: il commit resta nel backup.

use super::CliBackend;
use crate::command::failure;
use project_domain::{EntryKind, Oid, RepoPath};
use project_engine::{
    BranchDeleted, ConfigEntry, EngineError, HEADS_PREFIX, HeadState, RestoreOutcome, branch_ref,
    check_new_branch_name, check_no_operation,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

impl CliBackend {
    /// `git check-ref-format --branch <nome>` deve accettare `name` e restituirlo identico (con
    /// `@{-1}` o `@` git darebbe il nome di un altro branch). Prima [`check_new_branch_name`].
    fn check_new_name(&self, id: &'static str, name: &str) -> Result<(), EngineError> {
        check_new_branch_name(id, name)?;
        let out = self.git.output(
            Some(&self.workdir),
            // `--branch <nome>` prende l'argomento alla lettera (`check-ref-format` non ha
            // `--end-of-options`); un `-` iniziale è già rifiutato da `check_new_branch_name`.
            &["check-ref-format", "--branch", name],
        )?;
        let printed = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
        if out.status.success() && printed == name.as_bytes() {
            Ok(())
        } else {
            Err(EngineError::InvalidParams {
                id,
                reason: format!(
                    "git non accetta `{}` come nome di branch",
                    name.escape_debug()
                ),
            })
        }
    }

    /// Il worktree (diverso da questo) in cui il branch `full` è il branch di HEAD, se c'è
    /// (`git worktree list --porcelain`).
    pub(super) fn checked_out_elsewhere(&self, full: &str) -> Result<Option<PathBuf>, EngineError> {
        let out = self.run(&["worktree", "list", "--porcelain"])?;
        let here = canonical(&self.workdir);
        let mut current: Option<PathBuf> = None;
        for line in out.split(|&b| b == b'\n') {
            if let Some(path) = line.strip_prefix(b"worktree ") {
                current = Some(PathBuf::from(String::from_utf8_lossy(path).into_owned()));
            } else if let Some(name) = line.strip_prefix(b"branch ")
                && name == full.as_bytes()
                && let Some(path) = &current
                && canonical(path) != here
            {
                return Ok(Some(path.clone()));
            }
        }
        Ok(None)
    }

    /// [`EngineError::NotAllowed`] se `full` non vale `expected`.
    fn check_branch_value(&self, full: &str, expected: &Oid) -> Result<(), EngineError> {
        match self.ref_value(full)? {
            Some(now) if now == *expected => Ok(()),
            Some(now) => Err(EngineError::NotAllowed(format!(
                "`{full}` è cambiato: doveva essere {expected}, ora è {now}; niente modificato"
            ))),
            None => Err(EngineError::NotAllowed(format!(
                "non esiste un branch `{full}`, niente modificato"
            ))),
        }
    }

    /// [`GitBackend::unmerged_count`](project_engine::GitBackend::unmerged_count):
    /// `rev-list --count <tip> ^<into>...`, senza i replace object (conta la storia vera).
    pub(super) fn count_unmerged(&self, tip: &Oid, into: &[Oid]) -> Result<usize, EngineError> {
        let revs: Vec<String> = std::iter::once(tip.to_string())
            .chain(into.iter().map(|o| format!("^{o}")))
            .collect();
        let mut args = vec![
            "--no-replace-objects",
            "rev-list",
            "--count",
            "--end-of-options",
        ];
        args.extend(revs.iter().map(String::as_str));
        let out = self.run(&args)?;
        let text = String::from_utf8_lossy(&out);
        text.trim().parse().map_err(|_| EngineError::Git {
            command: "rev-list".to_owned(),
            stderr: format!("output inatteso: `{}`", text.trim()),
        })
    }

    /// [`GitBackend::create_branch`](project_engine::GitBackend::create_branch).
    pub(super) fn create_local_branch(
        &mut self,
        name: &str,
        target: &Oid,
    ) -> Result<(), EngineError> {
        self.check_new_name("branch.create", name)?;
        let full = branch_ref(name);
        let oid = target.to_string();
        let reflog = format!("project: branch.create {name} su {oid}");
        // Valore vecchio vuoto: il ref si crea solo se non esiste. `--no-deref`: un ref
        // simbolico `refs/heads/<nome>` (anche verso un ref che non c'è) non si segue, quindi si
        // crea o si rifiuta esattamente `refs/heads/<nome>`.
        let args = ["update-ref", "--no-deref", "-m", &reflog, &full, &oid, ""];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        if self.ref_value(&full)?.is_some()
            || stderr.contains("already exists")
            || stderr.contains("exists; cannot create")
            || stderr.contains("reference already exists")
        {
            // Esiste già, è un ref simbolico, oppure un ref lo contiene o ne è contenuto
            // (`a` per `a/b`): il messaggio di git dice quale.
            return Err(EngineError::NotAllowed(format!(
                "il nome `{}` è già preso, niente modificato (un ref non si sovrascrive): {}",
                name.escape_debug(),
                stderr.trim()
            )));
        }
        Err(failure(&args, &out))
    }

    /// [`GitBackend::checkout`](project_engine::GitBackend::checkout) con `git switch`.
    pub(super) fn switch_to(&mut self, target: &HeadState) -> Result<RestoreOutcome, EngineError> {
        check_no_operation("branch.checkout", "cambierebbe branch", self.operation())?;
        let status = self.status_for_write()?;
        let unmerged: Vec<String> = status
            .entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| e.path.to_string())
            .collect();
        if !unmerged.is_empty() {
            return Err(EngineError::NotAllowed(format!(
                "ci sono file in conflitto ({}): risolverli prima di cambiare branch, niente \
                 modificato",
                unmerged.join(", ")
            )));
        }
        let (label, oid, detach) = match target {
            HeadState::Branch(full) => {
                let name = full.strip_prefix(HEADS_PREFIX).ok_or_else(|| {
                    EngineError::NotAllowed(format!("`{full}` non è un branch locale"))
                })?;
                let oid = self.ref_value(full)?.ok_or_else(|| {
                    EngineError::NotAllowed(format!(
                        "non esiste un branch locale `{}`, niente modificato",
                        name.escape_debug()
                    ))
                })?;
                (name.to_owned(), oid, false)
            }
            HeadState::Detached(oid) => {
                let found = self.resolve(&oid.to_string())?;
                if found != Some(*oid) {
                    return Err(EngineError::UnknownRev(oid.to_string()));
                }
                (oid.to_string(), *oid, true)
            }
        };
        self.check_assume_unchanged(&status.head.target(), &oid)?;

        let mut args = vec![
            "-c",
            "core.fsmonitor=false",
            "switch",
            "--no-guess",
            "--no-overwrite-ignore",
        ];
        if detach {
            args.push("--detach");
        }
        args.extend(["--end-of-options", &label]);
        let out = self.git.output(Some(&self.workdir), &args)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            // Git elenca i file con un tab davanti: modifiche locali o non tracciati che il
            // checkout sovrascriverebbe o toglierebbe, cartelle con non tracciati che
            // diventerebbero file, e (con `submodule.recurse`) i file dei sottomoduli che il
            // loro checkout sovrascriverebbe ("would be overwritten by merge").
            if stderr.contains("would be overwritten by")
                || stderr.contains("would be removed by")
                || stderr.contains("would lose untracked files")
                || stderr.contains("would be lost")
            {
                let files = stderr
                    .lines()
                    .filter_map(|l| l.strip_prefix('\t'))
                    .map(str::to_owned)
                    .collect();
                return Err(EngineError::CheckoutConflict {
                    target: label,
                    files,
                    stderr: stderr.trim().to_owned(),
                });
            }
            return Err(failure(&args, &out));
        }
        let mut warnings = Vec::new();
        match self.head_state() {
            Ok(head) if head == *target => {}
            Ok(head) => warnings.push(format!(
                "dopo il checkout HEAD è {head} invece di {target}: un hook `post-checkout` o un \
                 altro processo l'ha cambiata; controllare HEAD prima di continuare"
            )),
            Err(e) => warnings.push(format!("dopo il checkout HEAD non si rilegge ({e})")),
        }
        Ok(RestoreOutcome {
            backups: Vec::new(),
            warnings,
        })
    }

    /// Git non controlla i file `--assume-unchanged` (lettera minuscola in `ls-files -v`): un
    /// checkout ne sovrascriverebbe le modifiche senza avviso. Rifiuta se uno di loro è
    /// diverso fra HEAD (`head`, `None` senza commit) e `target`.
    fn check_assume_unchanged(&self, head: &Option<Oid>, target: &Oid) -> Result<(), EngineError> {
        let listing = self.index_listing()?;
        let hidden: BTreeSet<&RepoPath> = listing
            .iter()
            .filter(|(_, e)| e.tag.is_ascii_lowercase())
            .map(|(p, _)| p)
            .collect();
        if hidden.is_empty() {
            return Ok(());
        }
        let from = head.map_or_else(|| self.empty_tree.to_owned(), |h| h.to_string());
        let to = target.to_string();
        let args = [
            "diff-tree",
            "-r",
            "-z",
            "--name-only",
            "--no-renames",
            "--end-of-options",
            &from,
            &to,
        ];
        let out = self.run(&args)?;
        let hit: Vec<String> = out
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| RepoPath::from(p.to_vec()))
            .filter(|p| hidden.contains(p))
            .map(|p| format!("`{p}`"))
            .collect();
        if hit.is_empty() {
            Ok(())
        } else {
            Err(EngineError::NotAllowed(format!(
                "file segnati `--assume-unchanged` diversi nel branch di destinazione ({}): git \
                 ne sovrascriverebbe le modifiche senza controllarle. Niente modificato; \
                 togliere il segno (`git update-index --no-assume-unchanged`) e riprovare",
                hit.join(", ")
            )))
        }
    }

    /// [`GitBackend::delete_branch`](project_engine::GitBackend::delete_branch).
    pub(super) fn delete_local_branch(
        &mut self,
        name: &str,
        expected: &Oid,
    ) -> Result<BranchDeleted, EngineError> {
        let full = branch_ref(name);
        if self.head_ref_name()?.as_deref() == Some(full.as_str()) {
            return Err(EngineError::NotAllowed(format!(
                "`{}` è il branch attivo (HEAD), niente eliminato",
                name.escape_debug()
            )));
        }
        if let Some(path) = self.checked_out_elsewhere(&full)? {
            return Err(EngineError::NotAllowed(format!(
                "`{}` è il branch attivo del worktree `{}`, niente eliminato",
                name.escape_debug(),
                path.display()
            )));
        }
        self.check_branch_value(&full, expected)?;
        // Prima di toccare qualcosa: la configurazione da togliere, salvata per l'undo.
        let (config, mut warnings) = self.read_branch_config(name)?;
        let oid = expected.to_string();
        let backup = self.create_backup_ref(
            "delete",
            &oid,
            &format!("project: backup di {full} prima di branch.delete"),
        )?;
        let reflog = format!("project: branch.delete {name} (era {oid})");
        let args = ["update-ref", "--no-deref", "-m", &reflog, "-d", &full, &oid];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if !out.status.success() {
            // Niente eliminato: il backup non serve (si toglie solo se punta ancora lì).
            let _ = self
                .git
                .output(Some(&self.workdir), &["update-ref", "-d", &backup, &oid]);
            self.check_branch_value(&full, expected)?;
            return Err(failure(&args, &out));
        }
        // Come `git branch -d`: via anche upstream e il resto di `branch.<nome>.*`.
        if !config.is_empty()
            && let Err(e) = self.remove_branch_config(name)
        {
            warnings.push(format!(
                "branch eliminato, ma la configurazione `branch.{name}.*` non si è tolta: {e}"
            ));
        }
        Ok(BranchDeleted {
            backup,
            config,
            warnings,
        })
    }

    /// [`GitBackend::rename_branch`](project_engine::GitBackend::rename_branch) con `git
    /// branch -m` (mai `-M`).
    pub(super) fn rename_local_branch(
        &mut self,
        from: &str,
        to: &str,
        expected: &Oid,
    ) -> Result<RestoreOutcome, EngineError> {
        self.check_new_name("branch.rename", to)?;
        let (old, new) = (branch_ref(from), branch_ref(to));
        self.check_branch_value(&old, expected)?;
        if self.ref_value(&new)?.is_some() {
            return Err(EngineError::NotAllowed(format!(
                "esiste già un branch `{}`, niente modificato (un branch non si sovrascrive)",
                to.escape_debug()
            )));
        }
        let args = ["branch", "-m", "--end-of-options", from, to];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        let mut warnings = Vec::new();
        match self.ref_value(&new)? {
            Some(now) if now == *expected => {}
            now => warnings.push(format!(
                "dopo la rinomina `{new}` vale {} invece di {expected}: un altro processo ha \
                 spostato il branch durante l'operazione",
                now.map_or_else(|| "niente".to_owned(), |o| o.to_string())
            )),
        }
        Ok(RestoreOutcome {
            backups: Vec::new(),
            warnings,
        })
    }
}

impl CliBackend {
    /// Le voci `branch.<name>.*` della configurazione del repo (`git config --local -z
    /// --get-regexp`, solo il file del repo: niente configurazione globale né `include`),
    /// nell'ordine del file, e gli avvisi per i valori non UTF-8 (non salvati).
    fn read_branch_config(
        &self,
        name: &str,
    ) -> Result<(Vec<ConfigEntry>, Vec<String>), EngineError> {
        let pattern = format!("^branch\\.{}\\.[^.]+$", regex_escape(name));
        let args = [
            "config",
            "--local",
            "-z",
            "--get-regexp",
            "--end-of-options",
            &pattern,
        ];
        let out = self.git.output(Some(&self.workdir), &args)?;
        // Codice 1 senza messaggi: nessuna voce.
        if out.status.code() == Some(1) && out.stderr.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        let mut entries = Vec::new();
        let mut warnings = Vec::new();
        for record in out.stdout.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            let (key, value) = match record.iter().position(|&b| b == b'\n') {
                Some(i) => (&record[..i], Some(&record[i + 1..])),
                None => (record, None),
            };
            let key = String::from_utf8_lossy(key).into_owned();
            match value.map(|v| String::from_utf8(v.to_vec())) {
                Some(Ok(v)) => entries.push(ConfigEntry {
                    key,
                    value: Some(v),
                }),
                None => entries.push(ConfigEntry { key, value: None }),
                Some(Err(_)) => warnings.push(format!(
                    "il valore di `{key}` non è UTF-8: non salvato, `edit.undo` non lo rimetterà"
                )),
            }
        }
        Ok((entries, warnings))
    }

    /// `git config --local --remove-section branch.<name>`; nessuna sezione non è un errore.
    fn remove_branch_config(&self, name: &str) -> Result<(), EngineError> {
        let section = format!("branch.{name}");
        let args = [
            "config",
            "--local",
            "--remove-section",
            "--end-of-options",
            &section,
        ];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() || String::from_utf8_lossy(&out.stderr).contains("no such section")
        {
            Ok(())
        } else {
            Err(failure(&args, &out))
        }
    }

    /// [`GitBackend::restore_branch_config`](project_engine::GitBackend::restore_branch_config).
    pub(super) fn set_branch_config(
        &mut self,
        name: &str,
        entries: &[ConfigEntry],
        remove: bool,
    ) -> Vec<String> {
        let current = match self.read_branch_config(name) {
            Ok((current, _)) => current,
            Err(e) => {
                return vec![format!(
                    "la configurazione `branch.{name}.*` non si legge ({e}): non toccata"
                )];
            }
        };
        if remove {
            if current.is_empty() {
                return Vec::new();
            }
            if current != entries {
                return vec![format!(
                    "la configurazione `branch.{name}.*` è cambiata dopo l'undo: lasciata com'è"
                )];
            }
            return match self.remove_branch_config(name) {
                Ok(()) => Vec::new(),
                Err(e) => vec![format!(
                    "la configurazione `branch.{name}.*` non si è tolta: {e}"
                )],
            };
        }
        if entries.is_empty() {
            return Vec::new();
        }
        if !current.is_empty() {
            return vec![format!(
                "esiste già una configurazione `branch.{name}.*`: quella salvata con \
                 l'eliminazione non è stata riscritta ({})",
                describe(entries)
            )];
        }
        for entry in entries {
            // Una chiave senza valore vale `true`.
            let value = entry.value.as_deref().unwrap_or("true");
            let args = [
                "config",
                "--local",
                "--add",
                "--end-of-options",
                &entry.key,
                value,
            ];
            match self.git.output(Some(&self.workdir), &args) {
                Ok(out) if out.status.success() => {}
                Ok(out) => {
                    return vec![format!(
                        "la configurazione `branch.{name}.*` è stata riscritta solo in parte: {} \
                         (valori salvati: {})",
                        failure(&args, &out),
                        describe(entries)
                    )];
                }
                Err(e) => return vec![format!("configurazione non riscritta: {e}")],
            }
        }
        Vec::new()
    }
}

/// `key=value, ...` per gli avvisi.
fn describe(entries: &[ConfigEntry]) -> String {
    entries
        .iter()
        .map(|e| format!("{}={}", e.key, e.value.as_deref().unwrap_or("true")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `name` con i caratteri speciali delle espressioni regolari estese preceduti da `\`.
fn regex_escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if r"\.^$|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Il percorso senza link simbolici, se si risolve (per confrontare i worktree).
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
