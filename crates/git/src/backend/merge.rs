//! Merge sul [`CliBackend`] (`merge.start`, `merge.abort`, S29) e lettura dell'operazione in
//! corso (`repo.operation`), secondo §5 e `impl-docs/git/merge.md`.
//!
//! - fast-forward: `git -c core.fsmonitor=false merge --ff-only --no-autostash
//!   --no-overwrite-ignore --no-edit --end-of-options <oid>` (git rifiuta da sé, prima di
//!   scrivere, di sovrascrivere modifiche locali e non tracciati);
//! - commit di merge: `git -c core.fsmonitor=false merge --no-autostash --no-overwrite-ignore
//!   --no-edit --commit --no-squash --ff|--no-ff -F <file> --end-of-options <oid>`, il messaggio
//!   da un file temporaneo nella cartella git (git non legge `-F -` da stdin per `merge`);
//! - **fermo** (conflitti, hook `pre-merge-commit`): il merge **resta in corso**, i file in
//!   conflitto dallo stato e dall'output di git; nessun `--abort`;
//! - non partito (un file che verrebbe sovrascritto): niente scritto, il backup si toglie.
//!
//! Prima di tutto il backup `refs/project/backup/merge-...` sul branch com'era; si toglie solo
//! se niente è cambiato. `GIT_REFLOG_ACTION=project: merge.start`, `GIT_EDITOR=:`.
//!
//! `merge.abort`: indice e worktree salvati in un commit come uno stash (genitori HEAD e il
//! commit dell'indice, costruiti in indici temporanei), il ref
//! `refs/project/backup/merge-abort-...`, poi `git merge --abort`.

use super::sync::{TempFile, overwrite_error};
use super::{BACKUP_IDENTITY, CliBackend, TempIndex};
use crate::command::os_from_bytes;
use project_domain::{Oid, Operation, RepoPath};
use project_engine::{
    EngineError, MergeAborted, MergeMode, MergeOutcome, MergeRequest, OperationState, branch_ref,
    check_merge_state, check_merging,
};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::time::{SystemTime, UNIX_EPOCH};

/// Il messaggio del reflog delle scritture di `merge.start`.
const REFLOG_ACTION: &str = "project: merge.start";

/// Opzioni per un indice temporaneo semplice (come i backup dell'indice): niente split index,
/// fsmonitor o indice sparso, qualunque sia la configurazione del repo.
const PLAIN_INDEX: [&str; 6] = [
    "-c",
    "core.splitIndex=false",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "index.sparse=false",
];

/// Gli id, uno per riga, di un file come `MERGE_HEAD`; le righe non valide si saltano.
fn oids(bytes: &[u8]) -> Vec<Oid> {
    bytes
        .split(|&b| b == b'\n')
        .map(<[u8]>::trim_ascii)
        .filter(|l| !l.is_empty())
        .filter_map(|l| Oid::from_hex(l).ok())
        .collect()
}

impl CliBackend {
    /// [`GitBackend::merge`](project_engine::GitBackend::merge).
    pub(super) fn merge_into_head(
        &mut self,
        request: &MergeRequest,
    ) -> Result<MergeOutcome, EngineError> {
        let status = self.status_for_write()?;
        check_merge_state(&status, &request.mode)?;
        let full = branch_ref(&request.branch);
        if self.head_ref_name()?.as_deref() != Some(full.as_str()) {
            return Err(EngineError::NotAllowed(
                "HEAD è cambiata mentre si preparava il merge: niente modificato, riprovare"
                    .to_owned(),
            ));
        }
        self.check_branch_value(&full, &request.expected)?;
        self.check_assume_unchanged(&Some(request.expected), &request.target)?;
        self.check_incoming_paths(&request.expected, &request.target, false, "il merge")?;
        let expected = request.expected.to_string();
        let target = request.target.to_string();
        let backup = self.create_backup_ref(
            "merge",
            &expected,
            &format!("project: backup di {full} prima di merge.start"),
        )?;
        let env: [(&str, &OsStr); 2] = [
            ("GIT_REFLOG_ACTION", OsStr::new(REFLOG_ACTION)),
            ("GIT_EDITOR", OsStr::new(":")),
        ];
        let mut args: Vec<&OsStr> = ["-c", "core.fsmonitor=false", "merge"]
            .into_iter()
            .map(OsStr::new)
            .collect();
        // Il file del messaggio vive fino alla fine della funzione, dopo `git merge`.
        let file;
        match request.mode {
            MergeMode::FastForward => {
                args.extend(
                    [
                        "--ff-only",
                        "--no-squash",
                        "--commit",
                        "--no-autostash",
                        "--no-overwrite-ignore",
                        "--no-edit",
                    ]
                    .map(OsStr::new),
                );
            }
            MergeMode::Merge { no_ff } => {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos());
                file = TempFile(
                    self.git_dir
                        .join(format!("project-merge-msg-{}-{nanos}", std::process::id())),
                );
                if let Err(e) = std::fs::write(&file.0, request.message.as_bytes()) {
                    self.drop_merge_backup(&backup, &full, &request.expected);
                    return Err(EngineError::Git {
                        command: "merge".to_owned(),
                        stderr: format!("messaggio del merge non scritto: {e}"),
                    });
                }
                args.extend(
                    [
                        "--no-autostash",
                        "--no-overwrite-ignore",
                        "--no-edit",
                        "--commit",
                        "--no-squash",
                        if no_ff { "--no-ff" } else { "--ff" },
                        "-F",
                    ]
                    .map(OsStr::new),
                );
                // Il percorso come `OsStr`: la cartella git può non essere UTF-8.
                args.push(file.0.as_os_str());
            }
        }
        args.extend(["--end-of-options", target.as_str()].map(OsStr::new));
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
                command: "merge".to_owned(),
                stderr: format!("dopo il merge `{full}` non esiste più"),
            })?;
            if after == request.expected {
                // Git dice di aver finito ma il branch non si è mosso (es. un `--squash` dalla
                // configurazione, o un hook): non è un merge riuscito. Il backup resta.
                return Err(EngineError::Git {
                    command: "merge".to_owned(),
                    stderr: format!(
                        "`git merge` è terminato senza errori ma `{full}` non si è spostato \
                         ({text}): controllare indice e worktree; il branch com'era è in \
                         `{backup}`"
                    ),
                });
            }
            let mut warnings = Vec::new();
            if self.head_ref_name()?.as_deref() != Some(full.as_str()) {
                warnings.push(format!(
                    "dopo il merge HEAD non è più su `{full}`: un hook o un altro processo l'ha \
                     cambiata; controllare HEAD prima di continuare"
                ));
            }
            if !self.is_ancestor(&request.target, &after)? {
                warnings.push(format!(
                    "dopo il merge `{full}` ({after}) non contiene `{}`: un hook o un altro \
                     processo ha spostato il branch durante l'operazione",
                    request.target_name
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
        if operation == Some(Operation::Merge) {
            // Fermo: resta in corso. I file in conflitto sono quelli ancora non uniti e quelli
            // che git ha detto in conflitto (anche se `rerere` li ha già risolti).
            let conflicts = self.stopped_conflicts(&text);
            return Ok(MergeOutcome {
                after: request.expected,
                backup,
                in_progress: true,
                conflicts,
                message: text,
                warnings: Vec::new(),
            });
        }
        // Niente è partito (es. un file non tracciato che verrebbe sovrascritto).
        self.drop_merge_backup(&backup, &full, &request.expected);
        if let Some(error) = overwrite_error(&text, "il merge") {
            return Err(error);
        }
        Err(EngineError::Git {
            command: "merge".to_owned(),
            stderr: text,
        })
    }

    /// Toglie il backup `name` se il branch `full` vale ancora `expected` (niente è cambiato) e
    /// il backup punta ancora lì.
    fn drop_merge_backup(&self, name: &str, full: &str, expected: &Oid) {
        if self.ref_value(full).ok().flatten() == Some(*expected) {
            let oid = expected.to_string();
            let _ = self
                .git
                .output(Some(&self.workdir), &["update-ref", "-d", name, &oid]);
        }
    }

    /// [`GitBackend::merge_abort`](project_engine::GitBackend::merge_abort).
    pub(super) fn abort_merge(&mut self) -> Result<MergeAborted, EngineError> {
        let status = self.status_for_write()?;
        check_merging("merge.abort", &status)?;
        self.check_index_unlocked()?;
        let head = self.resolve("HEAD")?.ok_or_else(|| {
            EngineError::NotAllowed("HEAD non ha un commit: niente annullato".to_owned())
        })?;
        let files: Vec<RepoPath> = status
            .entries
            .iter()
            .filter(|e| e.is_conflict() || e.is_staged() || e.is_unstaged())
            .map(|e| e.path.clone())
            .collect();
        // `git merge --abort` (`reset --merge`) rifiuta un file in stage con anche modifiche
        // fuori stage: si controlla prima, come il `MemoryBackend`, così niente viene scritto.
        let mixed: Vec<String> = status
            .entries
            .iter()
            .filter(|e| e.is_staged() && e.is_unstaged())
            .map(|e| format!("`{}`", e.path))
            .collect();
        if !mixed.is_empty() {
            return Err(EngineError::NotAllowed(format!(
                "git non annulla il merge: file in stage con anche modifiche fuori stage ({}). \
                 Metterle in stage (`index.stage`) o scartarle (`worktree.discard`) e \
                 riprovare; niente modificato, il merge è ancora in corso",
                mixed.join(", ")
            )));
        }
        let backup = self.save_merge_backup(&head, &files)?;
        let env = [("GIT_REFLOG_ACTION", OsStr::new("project: merge.abort"))];
        let args = ["-c", "core.fsmonitor=false", "merge", "--abort"];
        let out = self
            .git
            .output_with_input_env(Some(&self.workdir), &args, b"", &env)?;
        let still_merging = self.operation() == Some(Operation::Merge);
        if !out.status.success() || still_merging {
            // Il backup non si toglie mai: anche quando git rifiuta, `reset --merge` può aver
            // già riscritto una parte dei file (un file che non si cancella, una cartella in
            // sola lettura) con `MERGE_HEAD` e HEAD invariati.
            let stderr = String::from_utf8_lossy(&out.stderr)
                .trim()
                .trim_end_matches('.')
                .to_owned();
            let hint = if stderr.contains("not uptodate") {
                " Succede quando un file in stage ha anche modifiche fuori stage: metterle in \
                 stage (`index.stage`) o scartarle (`worktree.discard`) e riprovare."
            } else {
                ""
            };
            return Err(EngineError::Git {
                command: "merge".to_owned(),
                stderr: format!(
                    "`git merge --abort` non è riuscito ({stderr}).{hint} Il merge {} in corso e \
                     alcuni file potrebbero essere già stati riscritti: rileggere lo stato. \
                     Indice e worktree di prima sono salvati in `{backup}` (`git stash apply \
                     {backup}`)",
                    if still_merging {
                        "è ancora"
                    } else {
                        "potrebbe non essere più"
                    }
                ),
            });
        }
        let mut warnings = Vec::new();
        if self.resolve("HEAD")? != Some(head) {
            warnings.push(format!(
                "dopo l'annullamento HEAD non è più {head}: un hook o un altro processo l'ha \
                 spostata"
            ));
        }
        Ok(MergeAborted {
            backup,
            files,
            warnings,
        })
    }

    /// Salva indice e worktree prima di `merge.abort`, come uno stash: un commit con l'indice
    /// (le voci di stadio 0, e per i file in conflitto la versione nostra, stadio 2) con
    /// genitore HEAD, e un commit con l'albero dell'indice più i file `files` com'erano nel
    /// worktree (conflitti con i marcatori e risoluzioni comprese), con genitori HEAD e il
    /// commit dell'indice. Tutto in indici temporanei: l'indice vero, con i conflitti, non si
    /// tocca. Poi il ref nuovo `refs/project/backup/merge-abort-...` sul secondo commit.
    fn save_merge_backup(&self, head: &Oid, files: &[RepoPath]) -> Result<String, EngineError> {
        // L'indice vero, con gli stadi.
        let listing = self.run(&["ls-files", "-s", "-z", "--"])?;
        let mut info = Vec::new();
        let mut in_index: BTreeSet<Vec<u8>> = BTreeSet::new();
        for record in listing.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            let Some(tab) = record.iter().position(|&b| b == b'\t') else {
                continue;
            };
            let (meta, path) = (&record[..tab], &record[tab + 1..]);
            let mut fields = meta.split(|&b| b == b' ');
            let (Some(mode), Some(oid), Some(stage)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if stage != b"0" && stage != b"2" {
                continue;
            }
            info.extend_from_slice(mode);
            info.push(b' ');
            info.extend_from_slice(oid);
            info.push(b'\t');
            info.extend_from_slice(path);
            info.push(0);
            in_index.insert(path.to_vec());
        }
        let identity: Vec<(&str, &OsStr)> = BACKUP_IDENTITY
            .iter()
            .map(|(k, v)| (*k, OsStr::new(v)))
            .collect();
        let head = head.to_string();
        let merging = std::fs::read(self.git_dir.join("MERGE_HEAD"))
            .map(|b| {
                oids(&b)
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();

        let index = TempIndex::new(&self.git_dir);
        let env = [("GIT_INDEX_FILE", index.0.as_os_str())];
        let update = [&PLAIN_INDEX[..], &["update-index", "-z", "--index-info"]].concat();
        self.run_input(&update, &info, &env)?;
        let write = [&PLAIN_INDEX[..], &["write-tree"]].concat();
        let index_tree = self.run_input(&write, b"", &env)?;
        let index_tree = String::from_utf8_lossy(index_tree.trim_ascii_end()).into_owned();
        let message = format!("project: indice prima di merge.abort (merge di {merging})");
        let args = [
            "commit-tree",
            "--no-gpg-sign",
            "-p",
            &head,
            "-F",
            "-",
            &index_tree,
        ];
        let index_commit = self.run_input(&args, message.as_bytes(), &identity)?;
        let index_commit = String::from_utf8_lossy(index_commit.trim_ascii_end()).into_owned();

        // Il worktree: sopra l'indice, i file cambiati com'erano su disco (cancellati compresi;
        // un file che non c'è né su disco né nell'indice non ha niente da salvare).
        let paths: Vec<&RepoPath> = files
            .iter()
            .filter(|p| {
                in_index.contains(p.as_bytes())
                    || std::fs::symlink_metadata(self.workdir.join(os_from_bytes(p.as_bytes())))
                        .is_ok()
            })
            .collect();
        let worktree_tree = if paths.is_empty() {
            index_tree.clone()
        } else {
            let read = [&PLAIN_INDEX[..], &["read-tree", &index_tree]].concat();
            self.run_input(&read, b"", &env)?;
            let mut input = Vec::new();
            for path in &paths {
                input.extend_from_slice(path.as_bytes());
                input.push(0);
            }
            let add = [
                &["--literal-pathspecs"][..],
                &PLAIN_INDEX[..],
                &[
                    "add",
                    "-A",
                    "-f",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ],
            ]
            .concat();
            self.run_input(&add, &input, &env)?;
            let tree = self.run_input(&write, b"", &env)?;
            String::from_utf8_lossy(tree.trim_ascii_end()).into_owned()
        };
        let message = format!(
            "project: backup di indice e worktree prima di merge.abort (merge di {merging})"
        );
        let args = [
            "commit-tree",
            "--no-gpg-sign",
            "-p",
            &head,
            "-p",
            &index_commit,
            "-F",
            "-",
            &worktree_tree,
        ];
        let commit = self.run_input(&args, message.as_bytes(), &identity)?;
        let commit = String::from_utf8_lossy(commit.trim_ascii_end()).into_owned();
        self.create_backup_ref(
            "merge-abort",
            &commit,
            "project: backup di indice e worktree prima di merge.abort",
        )
    }

    /// [`GitBackend::operation_state`](project_engine::GitBackend::operation_state): dai file
    /// della cartella git (`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `MERGE_MSG`,
    /// `rebase-merge/` o `rebase-apply/` con `onto` e `head-name`).
    pub(super) fn read_operation_state(&self) -> OperationState {
        let read = |name: &str| std::fs::read(self.git_dir.join(name)).ok();
        let operation = self.operation();
        let incoming = match operation {
            Some(Operation::Merge) => read("MERGE_HEAD"),
            Some(Operation::CherryPick) => read("CHERRY_PICK_HEAD"),
            Some(Operation::Revert) => read("REVERT_HEAD"),
            _ => None,
        }
        .map(|b| oids(&b))
        .unwrap_or_default();
        let message = match operation {
            Some(Operation::Merge | Operation::CherryPick | Operation::Revert) => {
                read("MERGE_MSG").map(|b| String::from_utf8_lossy(&b).into_owned())
            }
            _ => None,
        };
        let (mut onto, mut rebase_branch) = (None, None);
        if operation == Some(Operation::Rebase) {
            let dir = if self.git_dir.join("rebase-merge").exists() {
                "rebase-merge"
            } else {
                "rebase-apply"
            };
            onto = read(&format!("{dir}/onto")).and_then(|b| oids(&b).into_iter().next());
            rebase_branch = read(&format!("{dir}/head-name"))
                .map(|b| String::from_utf8_lossy(b.trim_ascii()).into_owned())
                .filter(|n| n.starts_with("refs/"));
        }
        OperationState {
            operation,
            incoming,
            message,
            onto,
            rebase_branch,
        }
    }
}
