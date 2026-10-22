//! Backup dell'indice prima di stage e unstage (`impl-docs/ARCHITETTURA-git.md` §1 e §7).
//!
//! Stage e unstage riscrivono la versione in stage di un file. Se quella versione non è né in
//! HEAD né nel worktree (codice XY di `git status` con X ∉ {`.`, `?`} e Y ≠ `.`: `MM`, `AM`,
//! `AD`, `MD`, `RM`, ...) dopo la scrittura resterebbe solo in un blob irraggiungibile, che il
//! garbage collector di git prima o poi cancella. Prima di scrivere, i backend salvano quelle
//! versioni in un commit raggiungibile da un ref sotto [`BACKUP_PREFIX`]; se il backup non
//! riesce l'indice non si tocca.

use project_domain::{Change, EntryKind, Oid, RepoPath, Status};
use std::collections::BTreeSet;

/// Prefisso dei ref di backup: `refs/project/backup/index-<data>`.
pub const BACKUP_PREFIX: &str = "refs/project/backup/";

/// Una versione in stage da salvare prima di riscrivere l'indice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedVersion {
    pub path: RepoPath,
    /// Modo nell'indice (es. `0o100644`).
    pub mode: u32,
    /// Id del blob nell'indice.
    pub oid: Oid,
}

/// Vero se `path` è `wanted` o un file sotto la cartella `wanted`, come un pathspec letterale
/// di git.
pub fn path_matches(path: &RepoPath, wanted: &RepoPath) -> bool {
    let (path, wanted) = (path.as_bytes(), wanted.as_bytes());
    path == wanted || (path.starts_with(wanted) && path.get(wanted.len()) == Some(&b'/'))
}

/// Le versioni in stage dei file toccati da `paths` (anche dentro una cartella richiesta) che
/// esistono solo nell'indice: diverse da HEAD (X ∉ {`.`, `?`}) e dal worktree (Y ≠ `.`). Sono
/// quelle che uno stage o un unstage farebbe perdere. I file in conflitto non contano: l'unstage
/// li rifiuta e lo stage li risolve lasciando le loro versioni nell'estensione REUC
/// dell'indice (`git checkout -m` le ripristina).
pub fn versions_at_risk(status: &Status, paths: &[RepoPath]) -> Vec<StagedVersion> {
    versions_at_risk_hidden(status, paths, &BTreeSet::new())
}

/// Come [`versions_at_risk`], ma per i file in `hidden` il worktree non è affidabile: con
/// `--assume-unchanged` o `--skip-worktree` `git status` dà Y = `.` anche se il file è
/// diverso. Per loro basta che la versione in stage sia diversa da HEAD (X ∉ {`.`, `?`}).
pub fn versions_at_risk_hidden(
    status: &Status,
    paths: &[RepoPath],
    hidden: &BTreeSet<RepoPath>,
) -> Vec<StagedVersion> {
    status
        .entries
        .iter()
        .filter_map(|entry| {
            let info = match &entry.kind {
                EntryKind::Ordinary(info)
                | EntryKind::Renamed { info, .. }
                | EntryKind::Copied { info, .. } => info,
                _ => return None,
            };
            let touched = paths.iter().any(|p| {
                path_matches(&entry.path, p)
                    || matches!(&entry.kind, EntryKind::Renamed { original, .. } if path_matches(original, p))
            });
            let at_risk = entry.index.is_change()
                && (entry.worktree != Change::Unmodified || hidden.contains(&entry.path))
                && info.mode_index != 0;
            (touched && at_risk).then(|| StagedVersion {
                path: entry.path.clone(),
                mode: info.mode_index,
                oid: info.oid_index,
            })
        })
        .collect()
}

/// Messaggio del commit di backup: l'operazione e i percorsi salvati.
pub fn backup_message(operation: &str, versions: &[StagedVersion]) -> String {
    let mut message = format!(
        "Backup dell'indice prima di {operation}\n\nVersioni in stage che non erano né in HEAD \
         né nel worktree:\n\n"
    );
    for v in versions {
        message.push_str(&format!("{}\n", v.path));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitBackend, MemoryBackend};

    #[test]
    fn only_staged_versions_missing_from_head_and_worktree() {
        let status = MemoryBackend::sample().status().unwrap();
        let p = |s: &str| RepoPath::from(s);
        // README.md è `MM`: a rischio. docs/guida.md `A.`, src/lib.rs `.M`, note.txt `??`: no.
        let all = [p("README.md"), p("docs"), p("src"), p("note.txt")];
        let risky: Vec<_> = versions_at_risk(&status, &all)
            .into_iter()
            .map(|v| v.path)
            .collect();
        assert_eq!(risky, [p("README.md")]);
        assert!(versions_at_risk(&status, &[p("docs"), p("src/lib.rs")]).is_empty());
        assert!(versions_at_risk(&status, &[p("READ")]).is_empty());
        // docs/guida.md è `A.`: a rischio solo se il worktree non è affidabile.
        let hidden = BTreeSet::from([p("docs/guida.md")]);
        let risky: Vec<_> = versions_at_risk_hidden(&status, &all, &hidden)
            .into_iter()
            .map(|v| v.path)
            .collect();
        assert_eq!(risky, [p("README.md"), p("docs/guida.md")]);
    }

    #[test]
    fn matches_files_and_folders() {
        let p = |s: &str| RepoPath::from(s);
        assert!(path_matches(&p("a/b.txt"), &p("a")));
        assert!(path_matches(&p("a"), &p("a")));
        assert!(!path_matches(&p("ab/c"), &p("a")));
        assert!(!path_matches(&p("a"), &p("a/b")));
    }
}
