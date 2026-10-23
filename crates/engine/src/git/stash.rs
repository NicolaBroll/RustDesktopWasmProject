//! Stash (`stash.list`, `stash.push`, `stash.apply`, `stash.pop`, `stash.drop`, S28): tipi e
//! controlli comuni ai backend (`impl-docs/ARCHITETTURA-git.md` §4, §5 e §7).
//!
//! Scelte:
//!
//! - **Uno stash si indica con la posizione e l'id.** `stash@{n}` cambia a ogni push e drop;
//!   i comandi ricevono la posizione `index` (quella mostrata) e, facoltativo ma consigliato,
//!   l'id completo `expected` visto da chi chiama. Al backend arriva sempre l'id: la posizione
//!   attuale la ricalcola lui subito prima di scrivere.
//! - **`apply` e `pop` non sovrascrivono mai niente.** Prima di chiamare git: niente operazione
//!   in corso, niente file in conflitto, nessuna modifica locale (in stage, fuori stage, non
//!   tracciata o ignorata) ai file che lo stash cambia, e nessun file al posto dei non
//!   tracciati che lo stash rimette ([`stash_local_changes`]). Git farebbe da sé parte di questi
//!   controlli, ma non tutti prima di scrivere: con un non tracciato già presente rimette i
//!   file tracciati e poi fallisce ("could not restore untracked files"), e con una modifica
//!   locale rifiuta dopo aver già rimesso i non tracciati.
//! - **Conflitti: lasciati come li lascia git.** Lo stash si applica con un merge a tre vie
//!   (base dello stash, HEAD, stash): se HEAD ha cambiato gli stessi file il merge si ferma con
//!   i file in conflitto nell'indice. Non c'è un'operazione in corso da annullare (`git stash`
//!   non lascia `MERGE_HEAD`), e i file in conflitto non avevano modifiche locali (controllo
//!   sopra): la versione "nostra" è quella di HEAD e lo stash resta nell'elenco (`pop` non lo
//!   elimina), quindi niente si perde. Il risultato lo dice in `conflicts`; si risolvono con
//!   `index.stage` del file (e poi con l'editor dei conflitti, S30).
//! - **Diario** (§7): `stash.push` e `stash.drop` ci entrano, `stash.apply` e `stash.pop` no
//!   (vedi [`StashRecord`]).

use super::branch::check_no_operation;
use crate::EngineError;
use project_domain::{Change, EntryKind, Oid, RepoPath, Status};
use serde::{Deserialize, Serialize};

/// Il ref dello stash: punta allo stash più recente, il suo reflog è l'elenco.
pub const STASH_REF: &str = "refs/stash";

/// Uno stash dell'elenco (`stash.list`), dal più recente.
///
/// JSON: `{"index": 0, "name": "stash@{0}", "id", "base", "message": "On main: prova",
/// "branch": "main" | null, "time", "untracked": bool}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashEntry {
    /// La posizione nell'elenco: `n` di `stash@{n}`, 0 il più recente.
    pub index: usize,
    /// `stash@{n}`.
    pub name: String,
    /// Il commit dello stash (il worktree salvato); il suo albero, confrontato con `base`, dà
    /// le modifiche dei file tracciati (`commit.show`/`diff.commit` con questo id).
    pub id: Oid,
    /// Il commit di HEAD quando lo stash è stato creato (il primo genitore): la UI lo disegna
    /// accanto a questo commit nel grafo.
    pub base: Oid,
    /// Il messaggio del reflog come lo mostra `git stash list`: `On <branch>: <messaggio>` o
    /// `WIP on <branch>: <id breve> <oggetto>`.
    pub message: String,
    /// Il branch su cui è stato creato, letto da `message`; `None` con HEAD staccata (`(no
    /// branch)`) o con un messaggio di altro formato (`git stash store -m`).
    pub branch: Option<String>,
    /// Data del commit dello stash (secondi Unix).
    pub time: i64,
    /// Vero se contiene anche file non tracciati (terzo genitore, `include_untracked`).
    pub untracked: bool,
}

impl StashEntry {
    /// La voce in posizione `index` del commit `id` con i genitori `parents`; `None` se il
    /// commit non ha genitori (non è uno stash).
    pub fn new(
        index: usize,
        id: Oid,
        parents: &[Oid],
        time: i64,
        message: impl Into<String>,
    ) -> Option<Self> {
        let message = message.into();
        Some(Self {
            index,
            name: format!("stash@{{{index}}}"),
            id,
            base: *parents.first()?,
            branch: stash_branch(&message),
            message,
            time,
            untracked: parents.len() >= 3,
        })
    }
}

/// Il branch di un messaggio di stash (`WIP on main: ...`, `On feature/x: ...`); `None` per
/// `(no branch)` o un altro formato. Un nome di ref non contiene `:`, quindi il primo `: `
/// chiude il nome.
pub fn stash_branch(message: &str) -> Option<String> {
    let rest = message
        .strip_prefix("WIP on ")
        .or_else(|| message.strip_prefix("On "))?;
    let (branch, _) = rest.split_once(": ")?;
    (!branch.is_empty() && branch != "(no branch)" && !branch.contains(' '))
        .then(|| branch.to_owned())
}

/// Lo stash con id `id` nell'elenco, se c'è.
pub fn find_stash<'a>(list: &'a [StashEntry], id: &Oid) -> Option<&'a StashEntry> {
    list.iter().find(|s| s.id == *id)
}

/// Parametri di `stash.push`: `{"message"?, "include_untracked"?, "keep_index"?, "paths"?}`.
/// Anche la voce del diario li tiene, per il redo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StashPush {
    /// Il messaggio (una riga); senza, git scrive `WIP on <branch>: <id> <oggetto>`.
    pub message: Option<String>,
    /// Anche i file non tracciati (non gli ignorati): `--include-untracked`, che dopo averli
    /// salvati li toglie dal worktree.
    pub include_untracked: bool,
    /// Le modifiche in stage restano anche nell'indice e nel worktree (`--keep-index`); lo
    /// stash le contiene comunque.
    pub keep_index: bool,
    /// Solo questi file o cartelle (alla lettera, come `index.stage`); `None` = tutti.
    pub paths: Option<Vec<RepoPath>>,
}

impl StashPush {
    /// Errore se il messaggio non si può passare a git così com'è: una riga sola, senza NUL né
    /// caratteri di controllo (finisce in un argomento `--message=` e nel reflog, che lo
    /// ridurrebbe a una riga). Un messaggio vuoto o di soli spazi vale come assente.
    pub fn check(&mut self) -> Result<(), EngineError> {
        if let Some(message) = &self.message {
            if message.chars().any(char::is_control) {
                return Err(EngineError::InvalidParams {
                    id: "stash.push",
                    reason: "il messaggio dello stash è una riga sola, senza a capo né \
                             caratteri di controllo"
                        .to_owned(),
                });
            }
            if message.trim().is_empty() {
                self.message = None;
            }
        }
        Ok(())
    }

    /// Vero se la voce `path` dello stato rientra nei `paths` richiesti (tutte senza `paths`).
    fn selects(&self, path: &RepoPath) -> bool {
        self.paths
            .as_ref()
            .is_none_or(|paths| paths.iter().any(|p| super::path_matches(path, p)))
    }
}

/// I controlli di `stash.push` sullo stato letto prima di scrivere, comuni ai backend:
/// operazione in corso, file in conflitto, branch senza commit, file `intent-to-add` (con
/// questi git non salva lo stash: "Cannot save the current index state/worktree state") e
/// niente da mettere da parte fra i file scelti (modifiche ai file tracciati, in stage o no, e
/// con `include_untracked` i non tracciati). Errori [`EngineError::NotAllowed`], niente cambia.
pub fn check_stash_push(status: &Status, request: &StashPush) -> Result<(), EngineError> {
    const ID: &str = "stash.push";
    check_no_operation(ID, "metterebbe da parte le modifiche", status.operation)?;
    if status.head.target().is_none() {
        return Err(EngineError::NotAllowed(
            "il branch non ha ancora commit: uno stash si appoggia a HEAD, niente messo da parte"
                .to_owned(),
        ));
    }
    let unmerged = paths_where(status, |e| matches!(e.kind, EntryKind::Unmerged(_)));
    if !unmerged.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "ci sono file in conflitto ({}): risolverli prima di mettere da parte le modifiche, \
             niente modificato",
            unmerged.join(", ")
        )));
    }
    let intent = paths_where(status, |e| {
        tracked(&e.kind) && e.worktree == Change::Added && e.index == Change::Unmodified
    });
    if !intent.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "ci sono file `intent-to-add` (`git add -N`: {}): git non riesce a salvarli in uno \
             stash. Metterli in stage o toglierli dallo stage e riprovare, niente modificato",
            intent.join(", ")
        )));
    }
    let any = status.entries.iter().any(|e| {
        request.selects(&e.path)
            && match e.kind {
                EntryKind::Untracked => request.include_untracked,
                EntryKind::Ignored | EntryKind::Unmerged(_) => false,
                _ => e.index.is_change() || e.worktree.is_change(),
            }
    });
    if !any {
        let what = if request.include_untracked {
            "nessuna modifica né file non tracciato"
        } else {
            "nessuna modifica ai file tracciati (i non tracciati entrano solo con \
             `include_untracked`)"
        };
        let scope = if request.paths.is_some() {
            " fra i percorsi scelti"
        } else {
            ""
        };
        return Err(EngineError::NotAllowed(format!(
            "niente da mettere da parte: {what}{scope}"
        )));
    }
    Ok(())
}

/// I controlli di `stash.apply`/`stash.pop` sullo stato: operazione in corso, file in
/// conflitto. [`EngineError::NotAllowed`], niente cambia.
pub fn check_stash_apply(command: &'static str, status: &Status) -> Result<(), EngineError> {
    check_no_operation(command, "applicherebbe lo stash", status.operation)?;
    let unmerged = paths_where(status, |e| matches!(e.kind, EntryKind::Unmerged(_)));
    if !unmerged.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "ci sono file in conflitto ({}): risolverli prima di applicare uno stash, niente \
             modificato",
            unmerged.join(", ")
        )));
    }
    Ok(())
}

/// I file dello stato che impediscono di applicare uno stash che cambia i file tracciati
/// `tracked` e rimette i non tracciati `untracked`: ogni voce (modifica in stage o fuori,
/// non tracciato, ignorato) su uno di quei percorsi, o su una cartella che li contiene, o
/// dentro uno di loro. Il `CliBackend` aggiunge i file del worktree che lo stato non mostra (un
/// ignorato, un file al posto di una cartella).
///
/// Non blocca un file tracciato che ha **già** la versione dello stash: worktree uguale
/// all'indice (nessuna modifica fuori stage) e `in_stash(percorso, modo, id)` vero per la sua
/// voce dell'indice, cioè la stessa voce nell'indice salvato dallo stash e nel suo worktree
/// salvato (modo 0: assente da tutti e due). È il caso delle modifiche in stage rimaste dopo un
/// push con `keep_index` (o con `paths`, che salva comunque tutto l'indice): il merge non ha
/// niente da cambiare in quel file, e senza questa eccezione l'undo del push e il pop dopo un
/// `keep_index` sarebbero sempre rifiutati. Un file con anche modifiche fuori stage resta
/// bloccato.
pub fn stash_local_changes(
    status: &Status,
    tracked: &[RepoPath],
    untracked: &[RepoPath],
    in_stash: impl Fn(&RepoPath, u32, Oid) -> bool,
) -> Vec<String> {
    let targets: Vec<&RepoPath> = tracked.iter().chain(untracked).collect();
    let near = |p: &RepoPath, t: &RepoPath| super::path_matches(p, t) || super::path_matches(t, p);
    let mut hits: Vec<String> = status
        .entries
        .iter()
        .filter(|e| {
            let changed = match &e.kind {
                EntryKind::Untracked | EntryKind::Ignored | EntryKind::Unmerged(_) => true,
                // Solo per i file tracciati dello stash: un non tracciato da rimettere non deve
                // esserci affatto (con modo 0 `in_stash` sarebbe vero anche per lui).
                EntryKind::Ordinary(info)
                    if e.worktree == Change::Unmodified
                        && !untracked.iter().any(|t| near(&e.path, t))
                        && in_stash(&e.path, info.mode_index, info.oid_index) =>
                {
                    false
                }
                _ => e.index.is_change() || e.worktree.is_change(),
            };
            changed && targets.iter().any(|t| near(&e.path, t))
        })
        .map(|e| e.path.to_string())
        .collect();
    hits.dedup();
    hits
}

/// Inizio del messaggio di [`EngineError::NotAllowed`] quando la parte in stage di uno stash non
/// si applica sopra l'indice attuale (`restore_index`, "conflicts in index"): l'undo di
/// `stash.push` lo riconosce per dare un messaggio suo.
pub const INDEX_NOT_RESTORED: &str = "con `restore_index` le modifiche in stage di";

/// [`EngineError::NotAllowed`] di `stash.apply`/`stash.pop` quando dei file locali sarebbero
/// sovrascritti (`files`): niente è stato modificato.
pub fn stash_blocked(command: &str, stash: &str, files: &[String]) -> EngineError {
    EngineError::NotAllowed(format!(
        "`{command}` di `{stash}` sovrascriverebbe file con modifiche locali, non tracciati o \
         ignorati ({}): niente modificato. Committarli, metterli da parte o spostarli e \
         riprovare",
        files
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Richiesta a [`GitBackend::stash_apply`](crate::GitBackend::stash_apply).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashApply {
    /// Il comando, per i messaggi (`stash.apply`, `stash.pop`, `edit.undo`).
    pub command: &'static str,
    /// Lo stash da applicare: deve essere ancora nell'elenco.
    pub id: Oid,
    /// Rimette anche la divisione fra in stage e non (`--index`); senza, tutto torna fuori
    /// stage, tranne i file nuovi, che git lascia in stage.
    pub restore_index: bool,
}

/// Esito di [`GitBackend::stash_apply`](crate::GitBackend::stash_apply).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashApplied {
    /// I file rimasti in conflitto (il merge dello stash con HEAD si è fermato): vuoto se lo
    /// stash si è applicato del tutto.
    pub conflicts: Vec<String>,
    /// Avvisi scoperti dopo l'applicazione.
    pub warnings: Vec<String>,
}

/// Richiesta a [`GitBackend::stash_drop`](crate::GitBackend::stash_drop).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashDrop {
    /// Il comando, per i messaggi e il reflog del backup.
    pub command: &'static str,
    /// Lo stash da eliminare: deve essere ancora nell'elenco (in qualunque posizione).
    pub id: Oid,
    /// Il tipo del ref di backup da creare prima (`refs/project/backup/<tipo>-...`), `None`
    /// per non crearlo (il redo di un drop: il backup c'è già).
    pub backup: Option<&'static str>,
}

/// Esito di [`GitBackend::stash_drop`](crate::GitBackend::stash_drop).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashDropped {
    /// Il ref di backup sul commit dello stash, se creato.
    pub backup: Option<String>,
    /// Avvisi scoperti dopo l'eliminazione (es. l'elenco cambiato da un altro processo).
    pub warnings: Vec<String>,
}

/// Lo stash di una voce del diario (`stash.push`, `stash.drop`).
///
/// - **`stash.push`**: undo = `pop` di quello stash con `restore_index` (come
///   `git stash pop --index`), solo se HEAD è ancora quella della voce e sul commit `base`
///   (così il merge non può andare in conflitto) e lo stash è ancora nell'elenco; il commit
///   dello stash resta in un backup `refs/project/backup/undo-...`. Redo = di nuovo
///   `stash.push` con gli stessi parametri (`push`) e stessa HEAD: lo stash nuovo ha un altro
///   id (la voce si aggiorna) e contiene le modifiche che ci sono al momento del redo.
/// - **`stash.drop`**: undo = `git stash store` dello stesso commit (tenuto dal backup), solo se
///   non è già nell'elenco: torna **in cima** (`stash@{0}`), non nella posizione di prima
///   (`git stash store` non sa inserire altrove). Redo = di nuovo il drop, ovunque sia ora.
/// - **`stash.apply` e `stash.pop` non entrano**: cambiano solo indice e worktree e annullarli
///   vorrebbe dire togliere dai file quello che lo stash ha aggiunto, mescolato alle modifiche
///   che c'erano già o fatte dopo; lo stash applicato resta (apply) o è nel backup
///   `refs/project/backup/stash-...` (pop), quindi niente si perde.
///
/// JSON: `{"id", "message", "base", "push": StashPush | null}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashRecord {
    pub id: Oid,
    pub message: String,
    pub base: Oid,
    /// Solo per `stash.push`: i parametri, per il redo.
    #[serde(default)]
    pub push: Option<StashPush>,
}

/// Esito di [`pop_stash`].
#[derive(Debug)]
pub struct StashPopped {
    pub applied: StashApplied,
    /// L'esito del drop, se fatto (niente con conflitti o se il drop è fallito).
    pub dropped: Option<StashDropped>,
    /// Il drop è fallito **dopo** un apply riuscito: le modifiche sono nei file, lo stash è
    /// ancora nell'elenco. Non è un errore dell'operazione (l'apply non va ripetuto).
    pub drop_error: Option<EngineError>,
}

/// `apply` dello stash `id` e, se non restano conflitti, il suo drop (con un backup del tipo
/// `backup`): `stash.pop` e l'undo di `stash.push`. Con conflitti lo stash resta nell'elenco
/// (come `git stash pop`) e il drop non si fa. Un errore dell'apply è l'errore (niente è
/// cambiato); un errore del drop no: l'apply è fatto, ed è in [`StashPopped::drop_error`].
pub fn pop_stash(
    repo: &mut dyn crate::GitBackend,
    command: &'static str,
    id: Oid,
    restore_index: bool,
    backup: Option<&'static str>,
) -> Result<StashPopped, EngineError> {
    let applied = repo.stash_apply(&StashApply {
        command,
        id,
        restore_index,
    })?;
    if !applied.conflicts.is_empty() {
        return Ok(StashPopped {
            applied,
            dropped: None,
            drop_error: None,
        });
    }
    Ok(
        match repo.stash_drop(&StashDrop {
            command,
            id,
            backup,
        }) {
            Ok(dropped) => StashPopped {
                applied,
                dropped: Some(dropped),
                drop_error: None,
            },
            Err(e) => StashPopped {
                applied,
                dropped: None,
                drop_error: Some(e),
            },
        },
    )
}

fn tracked(kind: &EntryKind) -> bool {
    matches!(
        kind,
        EntryKind::Ordinary(_) | EntryKind::Renamed { .. } | EntryKind::Copied { .. }
    )
}

fn paths_where(
    status: &Status,
    keep: impl Fn(&project_domain::StatusEntry) -> bool,
) -> Vec<String> {
    status
        .entries
        .iter()
        .filter(|e| keep(e))
        .map(|e| format!("`{}`", e.path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitBackend, MemoryBackend};

    #[test]
    fn branch_from_message() {
        assert_eq!(stash_branch("On main: prova"), Some("main".to_owned()));
        assert_eq!(
            stash_branch("WIP on feature/x: abc1234 Oggetto: con due punti"),
            Some("feature/x".to_owned())
        );
        assert_eq!(stash_branch("WIP on (no branch): abc1234 x"), None);
        assert_eq!(stash_branch("messaggio libero"), None);
        assert_eq!(stash_branch("On : x"), None);
    }

    #[test]
    fn entry_from_parents() {
        let a = MemoryBackend::fake_oid(1);
        let b = MemoryBackend::fake_oid(2);
        let e = StashEntry::new(2, a, &[b, a, a], 5, "On main: x").unwrap();
        assert_eq!(e.name, "stash@{2}");
        assert_eq!((e.base, e.untracked), (b, true));
        assert!(!StashEntry::new(0, a, &[b, a], 5, "x").unwrap().untracked);
        assert_eq!(StashEntry::new(0, a, &[], 5, "x"), None);
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["name"], "stash@{2}");
        assert_eq!(json["branch"], "main");
        assert_eq!(json["base"], b.to_string());
    }

    #[test]
    fn push_checks() {
        let status = MemoryBackend::sample().status().unwrap();
        assert!(check_stash_push(&status, &StashPush::default()).is_ok());
        // Solo un non tracciato: serve `include_untracked`.
        let only_note = StashPush {
            paths: Some(vec![RepoPath::from("note.txt")]),
            ..StashPush::default()
        };
        let err = check_stash_push(&status, &only_note).unwrap_err();
        assert!(err.to_string().contains("include_untracked"), "{err}");
        let with_untracked = StashPush {
            include_untracked: true,
            ..only_note
        };
        assert!(check_stash_push(&status, &with_untracked).is_ok());

        let mut busy = status.clone();
        busy.operation = Some(project_domain::Operation::Rebase);
        assert!(matches!(
            check_stash_push(&busy, &StashPush::default()),
            Err(EngineError::NotAllowed(_))
        ));
        let empty = MemoryBackend::new().status().unwrap();
        let err = check_stash_push(&empty, &StashPush::default()).unwrap_err();
        assert!(err.to_string().contains("non ha ancora commit"), "{err}");

        let mut m = StashPush {
            message: Some("  ".to_owned()),
            ..StashPush::default()
        };
        m.check().unwrap();
        assert_eq!(m.message, None);
        let mut m = StashPush {
            message: Some("a\nb".to_owned()),
            ..StashPush::default()
        };
        assert!(matches!(m.check(), Err(EngineError::InvalidParams { .. })));
    }

    #[test]
    fn local_changes_block_the_stash_files() {
        let none = |_: &RepoPath, _: u32, _: Oid| false;
        let status = MemoryBackend::sample().status().unwrap();
        let p = |s: &str| RepoPath::from(s);
        assert_eq!(
            stash_local_changes(&status, &[p("README.md"), p("altro.rs")], &[], none),
            ["README.md"]
        );
        // Una cartella dello stash contiene un file modificato; un non tracciato da rimettere
        // è già lì.
        assert_eq!(
            stash_local_changes(&status, &[p("src")], &[p("note.txt")], none),
            ["src/lib.rs", "note.txt"]
        );
        assert!(stash_local_changes(&status, &[p("Cargo.toml")], &[], none).is_empty());
        // `docs/guida.md` (`A.`) ha già la versione dello stash: non blocca. `README.md` (`MM`)
        // ha modifiche fuori stage: blocca anche se l'indice coincide.
        let all = |_: &RepoPath, _: u32, _: Oid| true;
        assert_eq!(
            stash_local_changes(&status, &[p("README.md"), p("docs/guida.md")], &[], all),
            ["README.md"]
        );
        // L'eccezione non vale per un non tracciato da rimettere: `docs/guida.md` lì blocca.
        assert_eq!(
            stash_local_changes(&status, &[], &[p("docs/guida.md")], all),
            ["docs/guida.md"]
        );
    }
}
