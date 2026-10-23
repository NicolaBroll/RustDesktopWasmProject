//! Scartare le modifiche non in stage, con backup (`worktree.discard`,
//! `impl-docs/git/discard.md`).
//!
//! Scartare riporta i file del worktree alla versione in stage (come `git restore --worktree`):
//! quello che c'era nel worktree non esiste più da nessun'altra parte. Prima di scrivere i
//! backend salvano il worktree con `git stash create` (un commit che non tocca né l'indice né il
//! worktree) e un ref `refs/project/backup/discard-...` che lo tiene, così il gc non lo
//! cancella; la voce del diario ([`WorktreeBackup`]) permette a `edit.undo` di rimettere i file
//! com'erano.
//!
//! Scelte, comuni ai backend ([`discard_plan`]):
//!
//! - **Solo il worktree.** L'indice non si tocca: un file `MM` torna `M.` (la versione in stage
//!   resta). Per togliere anche lo stage c'è `index.unstage`.
//! - **Solo file tracciati con modifiche fuori stage** (Y ∉ {`.`, `?`, `!`}): modificati,
//!   eliminati, cambiati di tipo. Un file non tracciato chiesto per nome è rifiutato: scartarlo
//!   vorrebbe dire cancellarlo, e `git stash create` non salva i file non tracciati, quindi non
//!   ci sarebbe un backup. Dentro una cartella i non tracciati e gli ignorati restano dove sono.
//! - **Sottomoduli**: rifiutati se chiesti per nome, saltati dentro una cartella (`git restore`
//!   non entra nel sottomodulo).
//! - **Quando `git stash create` non può salvare il backup**, niente si scarta: branch senza
//!   commit, file in conflitto (anche fuori dai percorsi chiesti: "Cannot save the current
//!   index state") e file `intent-to-add` (`git add -N`, "not uptodate. Cannot merge").
//! - I file `--assume-unchanged`/`--skip-worktree` non hanno Y ≠ `.` e non si toccano.

use super::path_matches;
use crate::EngineError;
use project_domain::{Change, EntryKind, Oid, RepoPath, Status};
use serde::{Deserialize, Serialize};

/// Un file scartato: il percorso, che modifica fuori stage aveva (`Modified`, `Deleted`,
/// `TypeChanged`) e la voce dell'indice (modo e id) a cui il worktree è stato riportato.
///
/// JSON: `{"path", "change", "mode", "oid"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscardedFile {
    pub path: RepoPath,
    pub change: Change,
    /// Modo della voce nell'indice (es. `0o100644`).
    pub mode: u32,
    /// Id del blob nell'indice.
    pub oid: Oid,
}

/// Esito di [`GitBackend::discard`](crate::GitBackend::discard).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscardOutcome {
    /// I file riportati alla versione in stage, in ordine di percorso.
    pub files: Vec<DiscardedFile>,
    /// Il ref di backup (`refs/project/backup/discard-...`) che tiene il commit di
    /// `git stash create`.
    pub backup: String,
    /// Il commit di `git stash create`: il suo albero ha il contenuto del worktree dei file
    /// tracciati prima dello scarto (`git show <stash>:<percorso>`).
    pub stash: Oid,
    /// Solo se per qualche file i byte del worktree non sono quelli che `git restore` ricreerebbe
    /// dal commit `stash` (filtri `clean`, conversioni di fine riga come `core.autocrlf=input`):
    /// un commit (figlio di `stash`) con i soli file salvati byte per byte, senza filtri. L'undo
    /// riscrive quei byte esatti. Il ref di backup punta a questo commit quando c'è.
    #[serde(default)]
    pub raw: Option<Oid>,
}

/// Il backup del worktree in una voce del diario: il commit di `git stash create` e i file
/// scartati. `edit.undo` rimette quei file com'erano nel commit, `edit.redo` li scarta di
/// nuovo.
///
/// JSON: `{"stash": oid, "files": [DiscardedFile]}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeBackup {
    pub stash: Oid,
    pub files: Vec<DiscardedFile>,
    /// Vedi [`DiscardOutcome::raw`].
    #[serde(default)]
    pub raw: Option<Oid>,
}

/// Cosa chiede `edit.undo`/`edit.redo` di uno scarto al backend
/// ([`GitBackend::restore_worktree`](crate::GitBackend::restore_worktree)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRestore {
    /// `edit.undo` o `edit.redo`, per gli errori.
    pub command: &'static str,
    /// `false` (undo): i file devono essere ancora come li ha lasciati lo scarto (worktree
    /// uguale all'indice, voce dell'indice quella registrata) e tornano come in `stash`.
    /// `true` (redo): i file devono essere come in `stash` (con la stessa voce dell'indice) e
    /// tornano alla versione in stage; `stash` li tiene già, nessun backup nuovo.
    pub redo: bool,
    pub stash: Oid,
    pub files: Vec<DiscardedFile>,
    /// Vedi [`DiscardOutcome::raw`].
    pub raw: Option<Oid>,
}

impl WorktreeRestore {
    /// I percorsi dei file, nell'ordine della voce.
    pub fn paths(&self) -> Vec<RepoPath> {
        self.files.iter().map(|f| f.path.clone()).collect()
    }

    /// Le differenze fra l'indice atteso e quello attuale (`index` dà modo e id della voce di
    /// un percorso, `None` se non c'è), una per riga, per [`EngineError::UndoConflict`].
    pub fn index_conflicts(
        &self,
        mut index: impl FnMut(&RepoPath) -> Option<(u32, Oid)>,
    ) -> Vec<String> {
        self.files
            .iter()
            .filter(|f| index(&f.path) != Some((f.mode, f.oid)))
            .map(|f| {
                format!(
                    "`{}`: la versione in stage è cambiata dopo lo scarto",
                    f.path
                )
            })
            .collect()
    }

    /// [`EngineError::UndoConflict`] con `conflicts`, se non è vuoto.
    pub fn conflict_error(&self, conflicts: Vec<String>) -> Result<(), EngineError> {
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(EngineError::UndoConflict {
                command: self.command,
                refs: conflicts,
            })
        }
    }

    /// La riga di conflitto di un file del worktree che non è come atteso.
    pub fn worktree_conflict(&self, path: &RepoPath) -> String {
        if self.redo {
            format!("`{path}`: il file è cambiato dopo l'annullamento dello scarto")
        } else {
            format!("`{path}`: il file è cambiato dopo lo scarto")
        }
    }
}

/// I file che `worktree.discard` di `paths` riporterebbe alla versione in stage, in ordine di
/// percorso, con i controlli comuni ai backend (vedi il modulo). Niente è cambiato quando dà
/// errore:
///
/// - branch senza commit, file in conflitto o `intent-to-add` nel repo →
///   [`EngineError::NotAllowed`]: `git stash create` non salverebbe il backup;
/// - un file non tracciato o ignorato, o un sottomodulo con modifiche, chiesto per nome →
///   [`EngineError::NotAllowed`];
/// - nessuna modifica fuori stage nei percorsi → [`EngineError::NotAllowed`].
pub fn discard_plan(
    status: &Status,
    paths: &[RepoPath],
) -> Result<Vec<DiscardedFile>, EngineError> {
    if paths.is_empty() {
        return Err(EngineError::NotAllowed(
            "nessun percorso: niente da scartare".to_owned(),
        ));
    }
    if status.head.target().is_none() {
        return Err(EngineError::NotAllowed(
            "il branch non ha ancora commit: il backup del worktree (`git stash create`) non si \
             può creare, niente scartato"
                .to_owned(),
        ));
    }
    let conflicts: Vec<&RepoPath> = status
        .entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
        .map(|e| &e.path)
        .collect();
    if !conflicts.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "ci sono file in conflitto ({}): il backup del worktree (`git stash create`) non si \
             può creare con un indice in conflitto, niente scartato. Risolvere prima i conflitti",
            list(&conflicts)
        )));
    }
    let intent: Vec<&RepoPath> = status
        .entries
        .iter()
        .filter(|e| tracked(&e.kind) && e.worktree == Change::Added)
        .map(|e| &e.path)
        .collect();
    if !intent.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "ci sono file `intent-to-add` (`git add -N`: {}): il backup del worktree (`git stash \
             create`) non si può creare, niente scartato. Metterli in stage (`index.stage`) o \
             toglierli (`index.unstage`) e riprovare",
            list(&intent)
        )));
    }

    let mut files = Vec::new();
    let mut untracked = Vec::new();
    let mut submodules = Vec::new();
    for entry in &status.entries {
        if !paths.iter().any(|p| path_matches(&entry.path, p)) {
            continue;
        }
        let named = paths.contains(&entry.path);
        let info = match &entry.kind {
            EntryKind::Ordinary(info)
            | EntryKind::Renamed { info, .. }
            | EntryKind::Copied { info, .. } => info,
            EntryKind::Untracked | EntryKind::Ignored => {
                if named {
                    untracked.push(&entry.path);
                }
                continue;
            }
            EntryKind::Unmerged(_) => continue,
        };
        if !entry.worktree.is_change() {
            continue;
        }
        if info.submodule.is_some() {
            if named {
                submodules.push(&entry.path);
            }
            continue;
        }
        files.push(DiscardedFile {
            path: entry.path.clone(),
            change: entry.worktree,
            mode: info.mode_index,
            oid: info.oid_index,
        });
    }
    // Un non tracciato o ignorato al posto di una cartella del percorso di un file da scartare
    // (es. `top/d` creato dopo aver cancellato la cartella con `top/d/x`), o al posto del file
    // stesso (una cartella non tracciata `x/`): `git restore` lo cancellerebbe per ricreare il
    // file, e il backup non lo contiene.
    let in_the_way: Vec<&RepoPath> = status
        .entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Untracked | EntryKind::Ignored))
        .map(|e| &e.path)
        .filter(|u| {
            let bytes = u.as_bytes();
            let bare = RepoPath::from_bytes(bytes.strip_suffix(b"/").unwrap_or(bytes));
            files.iter().any(|f| path_matches(&f.path, &bare))
        })
        .collect();
    if !in_the_way.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "file non tracciati o ignorati dove andrebbero i file da scartare: {} (ricreare i \
             file li cancellerebbe, e il backup non li contiene). Niente scartato: spostarli o \
             scartare altri percorsi",
            list(&in_the_way)
        )));
    }
    if !untracked.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "file non tracciati, niente scartato: scartarli vorrebbe dire cancellarli, e il \
             backup del worktree (`git stash create`) non li salva: {}",
            list(&untracked)
        )));
    }
    if !submodules.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "sottomoduli, niente scartato: le modifiche di un sottomodulo si scartano dentro il \
             sottomodulo: {}",
            list(&submodules)
        )));
    }
    if files.is_empty() {
        let wanted: Vec<&RepoPath> = paths.iter().collect();
        return Err(EngineError::NotAllowed(format!(
            "nessuna modifica fuori stage da scartare in {}",
            list(&wanted)
        )));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// [`EngineError::NotAllowed`] se il piano calcolato ora (`now`) non è quello confermato
/// (`expected`).
pub fn check_same_plan(
    now: &[DiscardedFile],
    expected: &[DiscardedFile],
) -> Result<(), EngineError> {
    if now == expected {
        return Ok(());
    }
    let names = |files: &[DiscardedFile]| -> Vec<String> {
        files.iter().map(|f| format!("`{}`", f.path)).collect()
    };
    Err(EngineError::NotAllowed(format!(
        "i file da scartare sono cambiati dalla conferma (confermati: {}; ora: {}): niente \
         scartato, rileggere lo stato e confermare di nuovo",
        names(expected).join(", "),
        names(now).join(", ")
    )))
}

/// La descrizione di uno scarto per il diario e per le conferme: `Scarta le modifiche di
/// «a.txt»` o `Scarta le modifiche di 3 file («a», «b», «c»)` (al massimo tre nomi).
pub fn discard_description(files: &[DiscardedFile]) -> String {
    match files {
        [one] => format!("Scarta le modifiche di «{}»", one.path),
        _ => {
            let mut names: Vec<String> = files
                .iter()
                .take(3)
                .map(|f| format!("«{}»", f.path))
                .collect();
            if files.len() > 3 {
                names.push("...".to_owned());
            }
            format!(
                "Scarta le modifiche di {} file ({})",
                files.len(),
                names.join(", ")
            )
        }
    }
}

fn tracked(kind: &EntryKind) -> bool {
    matches!(
        kind,
        EntryKind::Ordinary(_) | EntryKind::Renamed { .. } | EntryKind::Copied { .. }
    )
}

fn list(paths: &[&RepoPath]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitBackend, MemoryBackend};
    use project_domain::{Head, StatusEntry, TrackedInfo, UnmergedInfo};

    fn p(s: &str) -> RepoPath {
        RepoPath::from(s)
    }

    #[test]
    fn plan_of_the_sample() {
        // README.md `MM`, docs/guida.md `A.`, src/lib.rs `.M`, note.txt `??`.
        let status = MemoryBackend::sample().status().unwrap();
        let files = discard_plan(&status, &[p("README.md"), p("src"), p("docs")]).unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, [p("README.md"), p("src/lib.rs")]);
        assert!(files.iter().all(|f| f.change == Change::Modified));

        // Un non tracciato chiesto per nome: rifiutato; nient'altro da scartare: rifiutato.
        let err = discard_plan(&status, &[p("note.txt"), p("README.md")]).unwrap_err();
        assert!(err.to_string().contains("non tracciati"), "{err}");
        let err = discard_plan(&status, &[p("docs")]).unwrap_err();
        assert!(err.to_string().contains("nessuna modifica"), "{err}");
        // Un prefisso che non è una cartella non comprende niente.
        assert!(discard_plan(&status, &[p("READ")]).is_err());
    }

    fn entry(path: &str, worktree: Change, kind: EntryKind) -> StatusEntry {
        StatusEntry {
            path: p(path),
            index: Change::Unmodified,
            worktree,
            kind,
        }
    }

    fn info() -> TrackedInfo {
        TrackedInfo {
            submodule: None,
            mode_head: 0o100644,
            mode_index: 0o100644,
            mode_worktree: 0o100644,
            oid_head: MemoryBackend::fake_oid(1),
            oid_index: MemoryBackend::fake_oid(1),
        }
    }

    #[test]
    fn plan_refusals() {
        let mut status = MemoryBackend::sample().status().unwrap();
        status.entries = vec![
            entry("-a", Change::Deleted, EntryKind::Ordinary(info())),
            entry("d/n", Change::Untracked, EntryKind::Untracked),
        ];
        let files = discard_plan(&status, &[p("-a"), p("d")]);
        // `d` ha solo un non tracciato (saltato), `-a` è eliminato nel worktree.
        assert_eq!(files.unwrap()[0].change, Change::Deleted);

        let mut unborn = status.clone();
        unborn.head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        assert!(matches!(
            discard_plan(&unborn, &[p("-a")]),
            Err(EngineError::NotAllowed(m)) if m.contains("non ha ancora commit")
        ));

        let mut conflict = status.clone();
        conflict.entries.push(StatusEntry {
            path: p("z"),
            index: Change::Unmerged,
            worktree: Change::Unmerged,
            kind: EntryKind::Unmerged(UnmergedInfo {
                conflict: project_domain::Conflict::BothModified,
                submodule: None,
                mode_base: 0,
                mode_ours: 0,
                mode_theirs: 0,
                mode_worktree: 0,
                oid_base: MemoryBackend::fake_oid(1),
                oid_ours: MemoryBackend::fake_oid(1),
                oid_theirs: MemoryBackend::fake_oid(1),
            }),
        });
        assert!(matches!(
            discard_plan(&conflict, &[p("-a")]),
            Err(EngineError::NotAllowed(m)) if m.contains("conflitto")
        ));

        let mut intent = status.clone();
        intent
            .entries
            .push(entry("i", Change::Added, EntryKind::Ordinary(info())));
        assert!(matches!(
            discard_plan(&intent, &[p("-a")]),
            Err(EngineError::NotAllowed(m)) if m.contains("intent-to-add")
        ));

        let mut sub = status.clone();
        let mut sub_info = info();
        sub_info.submodule = Some(project_domain::SubmoduleStatus {
            commit_changed: true,
            modified: false,
            untracked: false,
        });
        sub.entries
            .push(entry("s", Change::Modified, EntryKind::Ordinary(sub_info)));
        assert!(matches!(
            discard_plan(&sub, &[p("s")]),
            Err(EngineError::NotAllowed(m)) if m.contains("sottomoduli")
        ));
    }

    #[test]
    fn untracked_in_the_way_is_refused() {
        let mut status = MemoryBackend::sample().status().unwrap();
        status.entries = vec![
            entry("top/d/x", Change::Deleted, EntryKind::Ordinary(info())),
            entry("top/d", Change::Untracked, EntryKind::Untracked),
        ];
        for wanted in ["top", "top/d/x"] {
            assert!(matches!(
                discard_plan(&status, &[p(wanted)]),
                Err(EngineError::NotAllowed(m)) if m.contains("`top/d`")
            ));
        }
        // Una cartella non tracciata al posto del file.
        status.entries = vec![
            entry("x", Change::Deleted, EntryKind::Ordinary(info())),
            entry("x/", Change::Untracked, EntryKind::Untracked),
        ];
        assert!(discard_plan(&status, &[p("x")]).is_err());
        // Un non tracciato accanto (non sul percorso) non conta.
        status.entries = vec![
            entry("top/d/x", Change::Deleted, EntryKind::Ordinary(info())),
            entry("top/dd", Change::Untracked, EntryKind::Untracked),
        ];
        assert!(discard_plan(&status, &[p("top")]).is_ok());
    }

    #[test]
    fn descriptions() {
        let file = |s: &str| DiscardedFile {
            path: p(s),
            change: Change::Modified,
            mode: 0o100644,
            oid: MemoryBackend::fake_oid(1),
        };
        assert_eq!(
            discard_description(&[file("a")]),
            "Scarta le modifiche di «a»"
        );
        assert_eq!(
            discard_description(&[file("a"), file("b"), file("c"), file("d")]),
            "Scarta le modifiche di 4 file («a», «b», «c», ...)"
        );
    }

    #[test]
    fn restore_conflicts() {
        let f = DiscardedFile {
            path: p("a"),
            change: Change::Modified,
            mode: 0o100644,
            oid: MemoryBackend::fake_oid(1),
        };
        let r = WorktreeRestore {
            command: "edit.undo",
            redo: false,
            stash: MemoryBackend::fake_oid(9),
            files: vec![f],
            raw: None,
        };
        assert!(
            r.index_conflicts(|_| Some((0o100644, MemoryBackend::fake_oid(1))))
                .is_empty()
        );
        assert_eq!(r.index_conflicts(|_| None).len(), 1);
        assert!(r.conflict_error(Vec::new()).is_ok());
        assert!(matches!(
            r.conflict_error(vec!["x".into()]),
            Err(EngineError::UndoConflict {
                command: "edit.undo",
                ..
            })
        ));
    }

    #[test]
    fn json_shapes() {
        let f = DiscardedFile {
            path: p("a"),
            change: Change::Deleted,
            mode: 0o100644,
            oid: MemoryBackend::fake_oid(1),
        };
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            serde_json::json!({
                "path": "a",
                "change": "Deleted",
                "mode": 0o100644,
                "oid": MemoryBackend::fake_oid(1).to_string(),
            })
        );
    }
}
