//! Stash sul [`CliBackend`] (`stash.list`, `stash.push`, `stash.apply`, `stash.pop`,
//! `stash.drop`, S28), secondo §5:
//!
//! - elenco: `git log --walk-reflogs -z --format=tformat:%H%x00%P%x00%ct%x00%gs
//!   --end-of-options refs/stash --` (id, genitori, data, messaggio del reflog: 4 campi per
//!   record, ognuno terminato da NUL; il quarto NUL lo mette `-z`), solo se `refs/stash` esiste;
//! - push: `git -c core.fsmonitor=false [--literal-pathspecs] stash push [--include-untracked]
//!   --keep-index|--no-keep-index [--message=<messaggio>] [--pathspec-from-file=-
//!   --pathspec-file-nul]`. `git stash` non legge il messaggio da stdin né da un file: va in un
//!   solo argomento `--message=<testo>` (una riga, senza caratteri di controllo: lo controlla il
//!   motore), che git non può leggere come un'altra opzione. Mai `--all` (gli ignorati restano);
//!   lo stash si riconosce dal valore di `refs/stash` prima e dopo (git esce con 0 anche quando
//!   non c'è niente da mettere da parte);
//! - apply: `git -c core.fsmonitor=false stash apply [--index] --end-of-options <id>`, con l'id
//!   e non `stash@{n}` (la posizione può cambiare); prima i controlli di `git/stash.rs` più il
//!   file system: un file che lo stash aggiunge, o un non tracciato che rimette, non deve
//!   esistere nel worktree (nemmeno ignorato, né un file al posto di una sua cartella), e i file
//!   `--assume-unchanged` che lo stash cambia sono rifiutati (git li sovrascriverebbe senza
//!   controllarli);
//! - drop: backup `refs/project/backup/<tipo>-...` sull'id, ricontrollo che `stash@{n}` sia
//!   ancora quell'id, `git stash drop --quiet --end-of-options stash@{n}`; resta l'istante fra il
//!   ricontrollo e `drop` (git non ha un drop con il valore atteso), e dopo si confronta l'elenco
//!   con quello atteso (un altro stash tolto per errore è nei `warnings`);
//! - store (undo di un drop): `git stash store --quiet --message=<messaggio> <id>` (`store` non
//!   accetta `--end-of-options`; l'id è esadecimale e non può sembrare un'opzione).

use super::CliBackend;
use super::sync::conflicted_in_output;
use crate::command::{failure, stderr_text};
use project_domain::{EntryKind, Oid, RepoPath};
use project_engine::{
    EngineError, INDEX_NOT_RESTORED, STASH_REF, StashApplied, StashApply, StashDrop, StashDropped,
    StashEntry, StashPush, check_no_operation, check_stash_apply, check_stash_push, find_stash,
    path_matches, stash_blocked, stash_local_changes,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

impl CliBackend {
    /// [`GitBackend::stashes`](project_engine::GitBackend::stashes).
    pub(super) fn stash_entries(&self) -> Result<Vec<StashEntry>, EngineError> {
        if self.ref_value(STASH_REF)?.is_none() {
            return Ok(Vec::new());
        }
        let args = [
            "log",
            "--walk-reflogs",
            "-z",
            "--no-show-signature",
            "--no-color",
            "--format=tformat:%H%x00%P%x00%ct%x00%gs",
            "--end-of-options",
            STASH_REF,
            "--",
        ];
        let out = self.run(&args)?;
        let bad = || EngineError::Git {
            command: "log".to_owned(),
            stderr: format!(
                "elenco degli stash non riconosciuto: `{}`",
                String::from_utf8_lossy(&out).escape_debug()
            ),
        };
        let fields: Vec<&[u8]> = out.split(|&b| b == 0).collect();
        // L'ultimo NUL chiude l'ultimo record: dopo resta un campo vuoto.
        let Some((last, fields)) = fields.split_last() else {
            return Err(bad());
        };
        if !last.is_empty() || fields.len() % 4 != 0 {
            return Err(bad());
        }
        let mut list = Vec::new();
        for (index, record) in fields.chunks(4).enumerate() {
            let [id, parents, time, message] = record else {
                return Err(bad());
            };
            let id = Oid::from_hex(id)?;
            let parents = parents
                .split(|&b| b == b' ')
                .filter(|p| !p.is_empty())
                .map(Oid::from_hex)
                .collect::<Result<Vec<_>, _>>()?;
            let time = std::str::from_utf8(time)
                .ok()
                .and_then(|t| t.parse().ok())
                .ok_or_else(bad)?;
            let message = String::from_utf8_lossy(message).into_owned();
            list.push(StashEntry::new(index, id, &parents, time, message).ok_or_else(bad)?);
        }
        Ok(list)
    }

    /// [`GitBackend::stash_push`](project_engine::GitBackend::stash_push).
    pub(super) fn push_stash(&mut self, request: &StashPush) -> Result<StashEntry, EngineError> {
        // `git stash push` non vede le modifiche dei file `--assume-unchanged` (non le salva) e
        // poi riporta il worktree a HEAD con un reset: le perderebbe. Rifiutati sempre fra i
        // file scelti, senza guardare il contenuto, prima degli altri controlli (lo stato non li
        // mostra: sembrerebbe che non ci sia niente da mettere da parte).
        let hidden: Vec<String> = self
            .index_listing()?
            .into_iter()
            .filter(|(p, e)| {
                e.tag.is_ascii_lowercase()
                    && request
                        .paths
                        .as_ref()
                        .is_none_or(|ps| ps.iter().any(|w| path_matches(p, w)))
            })
            .map(|(p, _)| format!("`{p}`"))
            .collect();
        if !hidden.is_empty() {
            return Err(EngineError::NotAllowed(format!(
                "file segnati `--assume-unchanged` ({}): git non ne metterebbe da parte le \
                 modifiche e poi le cancellerebbe riportando il worktree a HEAD. Niente \
                 modificato; togliere il segno (`git update-index --no-assume-unchanged`) o \
                 escluderli con `paths`, e riprovare",
                hidden.join(", ")
            )));
        }
        let status = self.status_for_write()?;
        check_stash_push(&status, request)?;
        self.check_index_unlocked()?;
        let before = self.ref_value(STASH_REF)?;
        let message = request.message.as_ref().map(|m| format!("--message={m}"));
        let mut args = vec!["-c", "core.fsmonitor=false", "stash", "push"];
        if request.include_untracked {
            args.push("--include-untracked");
        }
        args.push(if request.keep_index {
            "--keep-index"
        } else {
            "--no-keep-index"
        });
        args.extend(message.as_deref());
        let out = match &request.paths {
            Some(paths) => self.output_with_paths(&args, paths)?,
            None => self.git.output(Some(&self.workdir), &args)?,
        };
        let after = self.ref_value(STASH_REF)?;
        if !out.status.success() {
            if let Some(id) = after.filter(|a| Some(*a) != before) {
                return Err(EngineError::Git {
                    command: "stash".to_owned(),
                    stderr: format!(
                        "lo stash {id} è stato creato, ma poi git si è fermato ({}): le modifiche \
                         sono nello stash; controllare il worktree prima di riprovare",
                        stderr_text(&out)
                    ),
                });
            }
            return Err(failure(&args, &out));
        }
        let Some(id) = after.filter(|a| Some(*a) != before) else {
            let said = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            return Err(EngineError::NotAllowed(format!(
                "niente da mettere da parte: git non ha creato lo stash ({}), niente modificato",
                if said.is_empty() {
                    stderr_text(&out)
                } else {
                    said
                }
            )));
        };
        self.stash_entries()?
            .into_iter()
            .next()
            .filter(|s| s.id == id)
            .ok_or_else(|| EngineError::Git {
                command: "stash".to_owned(),
                stderr: format!(
                    "lo stash {id} è stato creato, ma non è in cima all'elenco (un altro \
                     processo?): rileggere gli stash"
                ),
            })
    }

    /// I file tracciati che lo stash cambia (rispetto alla base, nel worktree salvato e
    /// nell'indice salvato) e i non tracciati che rimette.
    fn stash_targets(
        &self,
        stash: &StashEntry,
    ) -> Result<(Vec<RepoPath>, Vec<RepoPath>), EngineError> {
        let base = stash.base.to_string();
        let id = stash.id.to_string();
        let index = format!("{id}^2");
        let mut tracked = BTreeSet::new();
        for side in [&id, &index] {
            let args = [
                "diff-tree",
                "-r",
                "-z",
                "--name-only",
                "--no-renames",
                "--end-of-options",
                &base,
                side,
                "--",
            ];
            for p in self
                .run(&args)?
                .split(|&b| b == 0)
                .filter(|p| !p.is_empty())
            {
                tracked.insert(RepoPath::from_bytes(p));
            }
        }
        let mut untracked = Vec::new();
        if stash.untracked {
            // Niente `--` in fondo: `ls-tree` lo prenderebbe per un percorso (e non darebbe
            // niente).
            let tree = format!("{id}^3");
            let args = [
                "ls-tree",
                "-r",
                "-z",
                "--name-only",
                "--full-tree",
                "--end-of-options",
                &tree,
            ];
            for p in self
                .run(&args)?
                .split(|&b| b == 0)
                .filter(|p| !p.is_empty())
            {
                untracked.push(RepoPath::from_bytes(p));
            }
        }
        Ok((tracked.into_iter().collect(), untracked))
    }

    /// Le voci (modo, id) dell'albero di `rev`, per percorso (`ls-tree -r -z --full-tree`, senza
    /// `--` in fondo).
    fn tree_entries(&self, rev: &str) -> Result<BTreeMap<RepoPath, (u32, Oid)>, EngineError> {
        let args = [
            "ls-tree",
            "-r",
            "-z",
            "--full-tree",
            "--end-of-options",
            rev,
        ];
        let out = self.run(&args)?;
        let mut entries = BTreeMap::new();
        for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            // `<modo> <tipo> <id>\t<percorso>`
            let bad = || EngineError::Git {
                command: "ls-tree".to_owned(),
                stderr: format!(
                    "riga non riconosciuta: `{}`",
                    String::from_utf8_lossy(record)
                ),
            };
            let tab = record.iter().position(|&b| b == b'\t').ok_or_else(bad)?;
            let fields: Vec<&[u8]> = record[..tab].split(|&b| b == b' ').collect();
            let [mode, _, oid] = fields[..] else {
                return Err(bad());
            };
            let mode = std::str::from_utf8(mode)
                .ok()
                .and_then(|m| u32::from_str_radix(m, 8).ok())
                .ok_or_else(bad)?;
            entries.insert(
                RepoPath::from_bytes(&record[tab + 1..]),
                (mode, Oid::from_hex(oid)?),
            );
        }
        Ok(entries)
    }

    /// Vero se nel worktree c'è qualcosa al posto del file `path` che git dovrebbe creare: il
    /// file stesso (anche una cartella o un link) o, al posto di una sua cartella, qualcosa che
    /// non è una cartella vera.
    fn occupied(&self, path: &RepoPath) -> bool {
        let full = self
            .workdir
            .join(crate::command::os_from_bytes(path.as_bytes()));
        if full.symlink_metadata().is_ok() {
            return true;
        }
        let mut dir: &Path = &full;
        while let Some(parent) = dir.parent() {
            if parent == self.workdir || !parent.starts_with(&self.workdir) {
                break;
            }
            if let Ok(meta) = parent.symlink_metadata()
                && !meta.is_dir()
            {
                return true;
            }
            dir = parent;
        }
        false
    }

    /// [`GitBackend::stash_apply`](project_engine::GitBackend::stash_apply).
    pub(super) fn apply_stash(
        &mut self,
        request: &StashApply,
    ) -> Result<StashApplied, EngineError> {
        let command = request.command;
        check_no_operation(command, "applicherebbe lo stash", self.operation())?;
        let list = self.stash_entries()?;
        let stash = find_stash(&list, &request.id).cloned().ok_or_else(|| {
            EngineError::NotAllowed(format!(
                "`{command}`: lo stash {} non è più nell'elenco, niente modificato",
                request.id
            ))
        })?;
        let status = self.status_for_write()?;
        check_stash_apply(command, &status)?;
        let (tracked, untracked) = self.stash_targets(&stash)?;
        // Le voci dell'indice e del worktree salvati: un file in stage che ha già la stessa
        // voce in tutti e due (es. dopo un push con `keep_index`) non blocca.
        let saved_index = self.tree_entries(&format!("{}^2", stash.id))?;
        let saved_worktree = self.tree_entries(&stash.id.to_string())?;
        let in_stash = |path: &RepoPath, mode: u32, oid: Oid| {
            let (i, w) = (saved_index.get(path), saved_worktree.get(path));
            if mode == 0 {
                i.is_none() && w.is_none()
            } else {
                i == Some(&(mode, oid)) && w == Some(&(mode, oid))
            }
        };
        let mut blocked = stash_local_changes(&status, &tracked, &untracked, in_stash);
        let index = self.index_listing()?;
        let in_status: BTreeSet<&RepoPath> = status.entries.iter().map(|e| &e.path).collect();
        // Un file tracciato che lo stash aggiunge, se è nello stato, l'ha già visto
        // `stash_local_changes`; un non tracciato da rimettere non deve esistere comunque
        // (anche se lo stato lo mostra, es. `D.` con il file ignorato nel worktree).
        let added = tracked
            .iter()
            .filter(|p| !index.contains_key(*p) && !in_status.contains(p));
        for p in added.chain(&untracked) {
            if self.occupied(p) && !blocked.contains(&p.to_string()) {
                blocked.push(p.to_string());
            }
        }
        if !blocked.is_empty() {
            return Err(stash_blocked(command, &stash.name, &blocked));
        }
        let hidden: Vec<String> = tracked
            .iter()
            .filter(|p| index.get(*p).is_some_and(|e| e.tag.is_ascii_lowercase()))
            .map(|p| format!("`{p}`"))
            .collect();
        if !hidden.is_empty() {
            return Err(EngineError::NotAllowed(format!(
                "file segnati `--assume-unchanged` che lo stash cambia ({}): git ne \
                 sovrascriverebbe le modifiche senza controllarle. Niente modificato; togliere il \
                 segno (`git update-index --no-assume-unchanged`) e riprovare",
                hidden.join(", ")
            )));
        }
        self.check_index_unlocked()?;

        let id = stash.id.to_string();
        // `merge.directoryRenames=false`: con la rilevazione delle cartelle rinominate un file
        // che lo stash aggiunge in `old/` finirebbe in `new/` (se HEAD ha rinominato la
        // cartella) e sovrascriverebbe un ignorato che c'è lì, che i controlli sopra non vedono.
        let mut args = vec![
            "-c",
            "core.fsmonitor=false",
            "-c",
            "merge.directoryRenames=false",
            "stash",
            "apply",
        ];
        if request.restore_index {
            args.push("--index");
        }
        args.extend(["--end-of-options", &id]);
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            return Ok(StashApplied::default());
        }
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let after = self.status_for_write()?;
        let mut conflicts: Vec<String> = after
            .entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| e.path.to_string())
            .collect();
        if !conflicts.is_empty() || text.contains("CONFLICT") {
            for f in conflicted_in_output(&text) {
                if !conflicts.contains(&f) {
                    conflicts.push(f);
                }
            }
            return Ok(StashApplied {
                conflicts,
                warnings: Vec::new(),
            });
        }
        if text.contains("onflicts in index") {
            return Err(EngineError::NotAllowed(format!(
                "{INDEX_NOT_RESTORED} `{}` non si applicano sopra l'indice attuale: niente \
                 modificato; riprovare senza `restore_index` (tutto torna fuori stage)",
                stash.name
            )));
        }
        // Solo se il repo è cambiato fra i controlli e git: in questi casi git ha già scritto
        // una parte.
        let quoted = |files: Vec<String>| {
            files
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if text.contains("already exists") {
            // Git ha già fatto il merge dei file tracciati, poi non ha potuto rimettere i non
            // tracciati: `<file> already exists, no checkout`.
            let files: Vec<String> = text
                .lines()
                .filter_map(|l| l.strip_suffix(" already exists, no checkout"))
                .map(str::to_owned)
                .collect();
            return Err(EngineError::NotAllowed(format!(
                "`{command}` di `{}` è rimasto a metà: git ha già applicato le modifiche ai file \
                 tracciati, ma non ha rimesso i non tracciati perché ci sono già ({}). Lo stash \
                 resta nell'elenco; controllare il worktree prima di riprovare",
                stash.name,
                quoted(files)
            )));
        }
        if text.contains("would be overwritten") {
            // I file sono le righe con il tab subito dopo il messaggio (dopo git stampa anche
            // un riepilogo dello stato, con altre righe con il tab).
            let files: Vec<String> = text
                .lines()
                .skip_while(|l| !l.contains("would be overwritten"))
                .skip(1)
                .map_while(|l| l.strip_prefix('\t'))
                .map(str::to_owned)
                .collect();
            return Err(EngineError::NotAllowed(if stash.untracked {
                format!(
                    "`{command}` di `{}` rifiutato da git: sovrascriverebbe modifiche locali a \
                     {}. I file tracciati non sono stati toccati, ma git potrebbe aver già \
                     rimesso i non tracciati dello stash (l'apply può essere rimasto a metà; sono \
                     anche nello stash, che resta nell'elenco)",
                    stash.name,
                    quoted(files)
                )
            } else {
                format!(
                    "`{command}` di `{}` rifiutato da git: sovrascriverebbe modifiche locali a \
                     {}. Niente modificato; committarle, metterle da parte o scartarle e riprovare",
                    stash.name,
                    quoted(files)
                )
            }));
        }
        Err(failure(&args, &out))
    }

    /// [`GitBackend::stash_drop`](project_engine::GitBackend::stash_drop).
    pub(super) fn drop_stash(&mut self, request: &StashDrop) -> Result<StashDropped, EngineError> {
        let command = request.command;
        let list = self.stash_entries()?;
        let stash = find_stash(&list, &request.id).cloned().ok_or_else(|| {
            EngineError::NotAllowed(format!(
                "`{command}`: lo stash {} non è più nell'elenco, niente eliminato",
                request.id
            ))
        })?;
        let id = stash.id.to_string();
        let backup = match request.backup {
            Some(kind) => Some(self.create_backup_ref(
                kind,
                &id,
                &format!("project: backup di {} prima di {command}", stash.name),
            )?),
            None => None,
        };
        let forget = |this: &Self| {
            if let Some(name) = &backup {
                let _ = this
                    .git
                    .output(Some(&this.workdir), &["update-ref", "-d", name, &id]);
            }
        };
        // Subito prima: la posizione è ancora quella.
        if self.resolve(&stash.name)? != Some(stash.id) {
            forget(self);
            return Err(EngineError::NotAllowed(format!(
                "l'elenco degli stash è cambiato mentre si preparava `{command}` (un altro \
                 processo?): lo stash non è stato eliminato, rileggere gli stash e riprovare"
            )));
        }
        let args = ["stash", "drop", "--quiet", "--end-of-options", &stash.name];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if !out.status.success() {
            forget(self);
            return Err(failure(&args, &out));
        }
        let mut warnings = Vec::new();
        let expected: Vec<Oid> = list
            .iter()
            .filter(|s| s.id != stash.id)
            .map(|s| s.id)
            .collect();
        match self.stash_entries() {
            Ok(now) if now.iter().map(|s| s.id).eq(expected.iter().copied()) => {}
            Ok(_) => warnings.push(format!(
                "dopo `{command}` l'elenco degli stash non è quello atteso (un altro processo lo \
                 ha cambiato?): rileggerlo; lo stash eliminato è{}",
                backup
                    .as_ref()
                    .map(|b| format!(" in `{b}`"))
                    .unwrap_or_else(|| " fra i backup".to_owned())
            )),
            Err(e) => warnings.push(format!("dopo `{command}` gli stash non si rileggono ({e})")),
        }
        Ok(StashDropped { backup, warnings })
    }

    /// [`GitBackend::stash_store`](project_engine::GitBackend::stash_store).
    pub(super) fn store_stash(&mut self, id: &Oid, message: &str) -> Result<(), EngineError> {
        let hex = id.to_string();
        let message = format!("--message={message}");
        let args = ["stash", "store", "--quiet", &message, &hex];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        match self.stash_entries()?.first() {
            Some(top) if top.id == *id => Ok(()),
            _ => Err(EngineError::Git {
                command: "stash".to_owned(),
                stderr: format!(
                    "dopo `git stash store` lo stash {id} non è in cima all'elenco: rileggere gli \
                     stash"
                ),
            }),
        }
    }
}
