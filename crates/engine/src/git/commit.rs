//! Commit (`commit.create`): parametri, risultato e controlli comuni ai backend
//! (`impl-docs/ARCHITETTURA-git.md` §4 e §7).
//!
//! I backend chiamano gli stessi controlli prima di scrivere, così `MemoryBackend` e
//! `CliBackend` rifiutano gli stessi casi con gli stessi errori: messaggio vuoto, conflitti
//! nell'indice, rebase/cherry-pick/revert in corso, niente da committare, `amend` senza commit
//! o durante un merge, `amend` di un commit già pubblicato senza `"confirm": true`.

use crate::EngineError;
use project_domain::{Commit, EntryKind, Oid, Operation, Status};
use serde::{Deserialize, Serialize};

/// Id del comando, usato negli errori dei parametri.
const COMMAND: &str = "commit.create";

/// Parametri di `commit.create`: `{"message": string, "amend"?: bool, "confirm"?: bool}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitRequest {
    /// Il messaggio, passato a git com'è (via stdin, `git commit -F -`). La pulizia la fa git
    /// secondo `commit.cleanup` dell'utente: senza configurazione, con `-F` vale `whitespace`
    /// (spazi in fondo alle righe, righe vuote iniziali, finali e ripetute tolte; le righe che
    /// iniziano con `#` **restano**, es. `#123 corregge ...`).
    pub message: String,
    /// Riscrive l'ultimo commit (`git commit --amend`) invece di crearne uno nuovo: stesso
    /// autore e stessi genitori, albero dall'indice, messaggio nuovo.
    #[serde(default)]
    pub amend: bool,
    /// Necessario solo con `amend` di un commit già pubblicato (raggiungibile da un branch
    /// remoto): senza, [`EngineError::ConfirmRequired`]. Altrove è ignorato.
    #[serde(default)]
    pub confirm: bool,
}

impl CommitRequest {
    /// Un commit nuovo con `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            amend: false,
            confirm: false,
        }
    }

    /// Lo stesso, come `amend` dell'ultimo commit.
    pub fn amend(mut self) -> Self {
        self.amend = true;
        self
    }

    /// Lo stesso, con `"confirm": true`.
    pub fn confirmed(mut self) -> Self {
        self.confirm = true;
        self
    }

    /// [`EngineError::InvalidParams`] per un messaggio vuoto o di soli spazi (git lo
    /// rifiuterebbe comunque: niente `--allow-empty-message`) o con un byte NUL (git non lo
    /// accetta in un messaggio). Un messaggio di soli commenti passa: con la pulizia
    /// `whitespace` (il default con `-F`) è un messaggio valido; se `commit.cleanup` dell'utente
    /// toglie i commenti e non resta niente, git rifiuta il commit e il backend dà lo stesso
    /// errore ([`EngineError::empty_commit_message`]).
    pub fn check_message(&self) -> Result<(), EngineError> {
        let reason = if self.message.trim().is_empty() {
            "il messaggio è vuoto: scrivere almeno l'oggetto del commit"
        } else if self.message.contains('\0') {
            "il messaggio contiene un byte NUL"
        } else {
            return Ok(());
        };
        Err(EngineError::InvalidParams {
            id: COMMAND,
            reason: reason.to_owned(),
        })
    }
}

/// Risultato di [`GitBackend::commit`](crate::GitBackend::commit). `commit.create` aggiunge lo
/// stato (`status`).
///
/// Ha i valori prima e dopo che servono al diario delle operazioni (§7): il ref spostato
/// (`branch`, o HEAD staccata), il suo valore prima (`previous`) e dopo (`commit.id`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitOutcome {
    /// Il commit creato, come in `repo.log`.
    pub commit: Commit,
    /// HEAD prima del commit; `None` per il primo commit di un branch. Con `amend`, il commit
    /// riscritto.
    pub previous: Option<Oid>,
    /// Il branch spostato, nome completo (es. `refs/heads/main`); `None` con HEAD staccata.
    pub branch: Option<String>,
    /// Solo con `amend`: il ref di backup (`refs/project/backup/amend-...`) che punta al commit
    /// riscritto, salvato prima di scrivere. `None` anche se l'`amend` ha prodotto lo stesso
    /// commit (`commit.id == previous`: niente è cambiato e il backup è stato tolto).
    pub backup: Option<String>,
    /// Solo con `amend`: gli altri branch locali e i tag (nomi completi) che contengono il
    /// commit riscritto e che quindi, dopo l'`amend`, restano sulla storia vecchia (divergono
    /// da HEAD). Vuoto se nessuno. Campo aggiunto dopo la prima versione: chi lo legge da un
    /// JSON più vecchio lo trova vuoto.
    #[serde(default)]
    pub also_on: Vec<String>,
}

/// Come si dividono i ref che contengono il commit da riscrivere con `amend` (`containing`,
/// nomi completi da `refs/heads/`, `refs/remotes/`, `refs/tags/`; `branch` è quello corrente):
/// `(pubblicati, anche_su)`.
///
/// - Pubblicati (chiedono `"confirm": true`, [`check_amend_published`]): i branch remoti e i
///   tag. Un tag resterebbe sul commit vecchio. Un push senza ref di tracciamento (push per
///   URL, o ref remoti cancellati) non si vede: il commit risulta non pubblicato.
/// - Anche su (`also_on` del risultato): gli altri branch locali e i tag, che dopo l'`amend`
///   divergono da HEAD.
pub fn amend_impact(containing: &[String], branch: Option<&str>) -> (Vec<String>, Vec<String>) {
    let published = containing
        .iter()
        .filter(|n| n.starts_with("refs/remotes/") || n.starts_with("refs/tags/"))
        .cloned()
        .collect();
    let also_on = containing
        .iter()
        .filter(|n| {
            (n.starts_with("refs/heads/") && Some(n.as_str()) != branch)
                || n.starts_with("refs/tags/")
        })
        .cloned()
        .collect();
    (published, also_on)
}

/// I controlli sullo stato prima di un commit, uguali per tutti i backend. `has_head` dice se
/// HEAD ha già un commit. Niente è stato modificato quando danno errore.
///
/// - File in conflitto nell'indice → [`EngineError::NotAllowed`] (git rifiuterebbe comunque).
/// - Rebase, cherry-pick, revert o `git am` in corso → [`EngineError::NotAllowed`]: `git commit`
///   concluderebbe o cambierebbe l'operazione in modi poco visibili (un cherry-pick prenderebbe
///   l'autore del commit originale e chiuderebbe l'operazione; durante un rebase il commit
///   finirebbe nella sequenza). Si concludono con i comandi dell'operazione (step futuri) o da
///   terminale.
/// - Merge in corso: il commit è il **commit di merge** (git aggiunge `MERGE_HEAD` ai genitori),
///   anche con l'indice uguale a HEAD (merge senza modifiche, es. `-s ours`). Con `amend` è
///   rifiutato, come fa git.
/// - Bisect in corso: niente di speciale, il commit si fa.
/// - `amend` senza commit in HEAD → [`EngineError::NotAllowed`].
/// - Senza `amend`, niente in stage (e nessun merge in corso) → [`EngineError::NothingToCommit`]:
///   mai un commit vuoto. Un file `intent-to-add` (`git add -N`) non conta, come per git.
pub fn check_commit_state(status: &Status, has_head: bool, amend: bool) -> Result<(), EngineError> {
    let conflicts: Vec<String> = status
        .entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
        .map(|e| format!("`{}`", e.path))
        .collect();
    if !conflicts.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "ci sono file in conflitto, niente committato: risolverli e metterli in stage \
             (`index.stage`) prima del commit: {}",
            conflicts.join(", ")
        )));
    }
    match status.operation {
        Some(
            op @ (Operation::Rebase | Operation::CherryPick | Operation::Revert | Operation::Am),
        ) => {
            let name = match op {
                Operation::Rebase => "un rebase",
                Operation::CherryPick => "un cherry-pick",
                Operation::Am => "un `git am`",
                _ => "un revert",
            };
            return Err(EngineError::NotAllowed(format!(
                "è in corso {name}: un commit da qui lo cambierebbe in modo poco visibile, \
                 niente committato. Concludere o annullare l'operazione (anche da terminale) e \
                 riprovare"
            )));
        }
        Some(Operation::Merge) if amend => {
            return Err(EngineError::NotAllowed(
                "è in corso un merge: non si può fare `amend` dell'ultimo commit, niente \
                 committato. Il commit senza `amend` conclude il merge"
                    .to_owned(),
            ));
        }
        _ => {}
    }
    if amend && !has_head {
        return Err(EngineError::NotAllowed(
            "`amend` senza un commit da riscrivere: il branch non ha ancora commit, niente \
             committato"
                .to_owned(),
        ));
    }
    let merging = status.operation == Some(Operation::Merge);
    if !amend && !merging && !status.entries.iter().any(|e| e.is_staged()) {
        return Err(EngineError::NothingToCommit);
    }
    Ok(())
}

/// Con `amend` di un commit già pubblicato (`published`: i branch remoti e i tag da cui è
/// raggiungibile, vedi [`amend_impact`]) serve `"confirm": true`: chi ha già scaricato il commit
/// si troverebbe una storia diversa, il push successivo andrebbe forzato e un tag resterebbe sul
/// commit vecchio. Senza conferma, [`EngineError::ConfirmRequired`] con i ref.
pub fn check_amend_published(
    request: &CommitRequest,
    published: &[String],
) -> Result<(), EngineError> {
    if !request.amend || request.confirm || published.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = published.iter().map(|n| format!("`{n}`")).collect();
    Err(EngineError::ConfirmRequired {
        command: COMMAND,
        reason: format!(
            "`amend` riscriverebbe un commit già pubblicato o con un tag (raggiungibile da \
             {}): chi lo ha già scaricato vedrebbe una storia diversa, il push andrebbe forzato \
             e i tag resterebbero sul commit vecchio. Il commit riscritto resterebbe in un ref \
             di backup",
            names.join(", ")
        ),
    })
}

/// Il messaggio come lo pulisce git con `--cleanup=whitespace` (il default di `git commit -F`):
/// spazi in fondo alle righe tolti, righe vuote iniziali e finali tolte, più righe vuote di
/// seguito ridotte a una, un a capo finale. Le righe di commento restano. Serve al
/// `MemoryBackend`, che non ha git; il `CliBackend` lascia fare a git.
pub fn cleanup_message(message: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in message.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push('\n');
            blank = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// `(oggetto, corpo)` di un messaggio pulito, come `%s` e `%b` di git: l'oggetto è il primo
/// paragrafo con le righe unite da uno spazio, il corpo il resto dopo le righe vuote, senza a
/// capo finali.
pub fn split_message(message: &str) -> (String, String) {
    let mut lines = message.lines();
    let mut summary = Vec::new();
    for line in lines.by_ref() {
        if line.trim().is_empty() {
            if summary.is_empty() {
                continue;
            }
            break;
        }
        summary.push(line.trim());
    }
    let rest: Vec<&str> = lines.skip_while(|l| l.trim().is_empty()).collect();
    let body = rest.join("\n").trim_end_matches('\n').to_owned();
    (summary.join(" "), body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitBackend, MemoryBackend};
    use project_domain::{Change, RepoPath, StatusEntry};

    #[test]
    fn message_checks() {
        for bad in ["", "  \n\t\n", "a\0b"] {
            let err = CommitRequest::new(bad).check_message().unwrap_err();
            assert!(matches!(err, EngineError::InvalidParams { .. }), "{bad:?}");
        }
        for good in ["x", "# solo un commento", "#123 corregge\n"] {
            CommitRequest::new(good).check_message().unwrap();
        }
    }

    #[test]
    fn cleanup_like_git_whitespace() {
        // `printf '...' | git commit -F -` con `--cleanup=whitespace`.
        assert_eq!(
            cleanup_message("\n# commento\n\nTitolo  \n\n\ncorpo\n# altro\n\n"),
            "# commento\n\nTitolo\n\ncorpo\n# altro\n"
        );
        assert_eq!(cleanup_message("solo"), "solo\n");
        assert_eq!(cleanup_message("  \n"), "");
    }

    #[test]
    fn split_like_git_format() {
        assert_eq!(
            split_message("Titolo\nsu due righe\n\n\nCorpo\n\naltro\n"),
            (
                "Titolo su due righe".to_owned(),
                "Corpo\n\naltro".to_owned()
            )
        );
        assert_eq!(split_message("solo\n"), ("solo".to_owned(), String::new()));
    }

    fn status_with(entries: Vec<StatusEntry>, operation: Option<Operation>) -> Status {
        let mut status = MemoryBackend::sample().status().unwrap();
        status.entries = entries;
        status.operation = operation;
        status
    }

    #[test]
    fn state_checks() {
        let sample = MemoryBackend::sample().status().unwrap();
        check_commit_state(&sample, true, false).unwrap();
        check_commit_state(&sample, true, true).unwrap();
        // Solo modifiche fuori stage e non tracciati: niente da committare.
        let unstaged: Vec<_> = sample
            .entries
            .iter()
            .filter(|e| !e.is_staged())
            .cloned()
            .collect();
        let status = status_with(unstaged.clone(), None);
        assert!(matches!(
            check_commit_state(&status, true, false),
            Err(EngineError::NothingToCommit)
        ));
        // Ma con `amend` si cambia solo il messaggio.
        check_commit_state(&status, true, true).unwrap();
        assert!(matches!(
            check_commit_state(&status, false, true),
            Err(EngineError::NotAllowed(_))
        ));
        // Un merge senza modifiche si conclude; non si fa `amend`.
        let merging = status_with(unstaged, Some(Operation::Merge));
        check_commit_state(&merging, true, false).unwrap();
        assert!(check_commit_state(&merging, true, true).is_err());
        for op in [
            Operation::Rebase,
            Operation::CherryPick,
            Operation::Revert,
            Operation::Am,
        ] {
            let status = status_with(sample.entries.clone(), Some(op));
            let err = check_commit_state(&status, true, false).unwrap_err();
            assert!(matches!(err, EngineError::NotAllowed(_)), "{op:?}");
        }
        let status = status_with(sample.entries.clone(), Some(Operation::Bisect));
        check_commit_state(&status, true, false).unwrap();
        // Un conflitto blocca tutto.
        let mut entries = sample.entries.clone();
        entries.push(StatusEntry {
            path: RepoPath::from("c.txt"),
            index: Change::Unmerged,
            worktree: Change::Unmerged,
            kind: EntryKind::Unmerged(project_domain::UnmergedInfo {
                conflict: project_domain::Conflict::BothModified,
                submodule: None,
                mode_base: 0o100644,
                mode_ours: 0o100644,
                mode_theirs: 0o100644,
                mode_worktree: 0o100644,
                oid_base: Oid::from_bytes([1; Oid::LEN]),
                oid_ours: Oid::from_bytes([2; Oid::LEN]),
                oid_theirs: Oid::from_bytes([3; Oid::LEN]),
            }),
        });
        let err = check_commit_state(&status_with(entries, Some(Operation::Merge)), true, false)
            .unwrap_err();
        assert!(err.to_string().contains("`c.txt`"), "{err}");
    }

    #[test]
    fn amend_impact_splits_published_and_diverging_refs() {
        let refs: Vec<String> = [
            "refs/heads/altro",
            "refs/heads/main",
            "refs/remotes/origin/main",
            "refs/tags/v1",
        ]
        .map(str::to_owned)
        .into();
        let (published, also_on) = amend_impact(&refs, Some("refs/heads/main"));
        assert_eq!(published, ["refs/remotes/origin/main", "refs/tags/v1"]);
        assert_eq!(also_on, ["refs/heads/altro", "refs/tags/v1"]);
        // Con HEAD staccata anche `main` diverge.
        let (_, also_on) = amend_impact(&refs, None);
        assert_eq!(
            also_on,
            ["refs/heads/altro", "refs/heads/main", "refs/tags/v1"]
        );
    }

    #[test]
    fn amend_of_published_commit_needs_confirm() {
        let published = vec!["refs/remotes/origin/main".to_owned()];
        let amend = CommitRequest::new("x").amend();
        let err = check_amend_published(&amend, &published).unwrap_err();
        assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
        assert!(err.to_string().contains("origin/main"), "{err}");
        assert!(err.to_string().contains("\"confirm\": true"), "{err}");
        check_amend_published(&amend.clone().confirmed(), &published).unwrap();
        check_amend_published(&amend, &[]).unwrap();
        check_amend_published(&CommitRequest::new("x"), &published).unwrap();
    }
}
