//! Merge (`merge.start`, `merge.abort`, `merge.continue`, S29) e stato "in corso"
//! (`repo.operation`): tipi e controlli comuni ai backend (`impl-docs/git/merge.md`).
//!
//! Il comando risolve la rev, decide fra fast-forward e commit di merge (con
//! [`GitBackend::unmerged_count`](super::GitBackend::unmerged_count)), controlla lo stato
//! ([`check_merge_state`]) e scrive il messaggio di default ([`merge_source`],
//! [`MergeSource::message`]); il backend esegue ([`GitBackend::merge`](super::GitBackend::merge)).
//! **Un merge fermo in conflitto resta in corso** (come il pull in merge; il pull in rebase
//! invece si annulla): si
//! conclude con `merge.continue` (che usa [`GitBackend::commit`](super::GitBackend::commit),
//! con `MERGE_HEAD` git crea il commit di merge) o si annulla con `merge.abort`
//! ([`GitBackend::merge_abort`](super::GitBackend::merge_abort), con un backup di indice e
//! worktree prima).

use super::{HEADS_PREFIX, REMOTES_PREFIX, check_no_operation};
use crate::EngineError;
use project_domain::{EntryKind, Oid, Operation, RefInfo, RefKind, RepoPath, Status};
use serde::{Deserialize, Serialize};

const TAGS_PREFIX: &str = "refs/tags/";

/// Come integrare il commit in HEAD ([`GitBackend::merge`](super::GitBackend::merge)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMode {
    /// Il branch avanza sul commit (`git merge --ff-only`): HEAD ne è un antenato.
    FastForward,
    /// Un commit di merge (genitori: HEAD e il commit), anche quando si potrebbe fare
    /// fast-forward se `no_ff` (`--no-ff`).
    Merge { no_ff: bool },
}

impl MergeMode {
    /// Il nome nel risultato (`strategy`): `fast_forward` o `merge`.
    pub fn name(&self) -> &'static str {
        match self {
            MergeMode::FastForward => "fast_forward",
            MergeMode::Merge { .. } => "merge",
        }
    }
}

/// Cosa chiede `merge.start` al backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRequest {
    /// Il branch di HEAD (nome breve): il merge non parte con HEAD staccata.
    pub branch: String,
    /// Il valore del branch letto prima: il backend rifiuta se è cambiato.
    pub expected: Oid,
    /// Il commit da unire (la rev già risolta: a git arriva solo l'id).
    pub target: Oid,
    /// Per i messaggi: il nome della rev (`feature`, `origin/x`, `v1.0`) o l'id.
    pub target_name: String,
    pub mode: MergeMode,
    /// Il messaggio del commit di merge (solo con [`MergeMode::Merge`]).
    pub message: String,
}

/// Esito di [`GitBackend::merge`](super::GitBackend::merge).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeOutcome {
    /// Il valore del branch dopo: il commit di merge, il commit del fast-forward o, se il merge
    /// è fermo, quello di prima.
    pub after: Oid,
    /// Il ref di backup del branch prima del merge (`refs/project/backup/merge-...`).
    pub backup: String,
    /// Vero se il merge si è fermato (conflitti, o un hook `pre-merge-commit` che ha rifiutato):
    /// il repo è in merge (`MERGE_HEAD`), da concludere con `merge.continue` o annullare con
    /// `merge.abort`.
    pub in_progress: bool,
    /// I file in conflitto (vuoto se il merge è concluso): quelli ancora non uniti e quelli che
    /// git ha detto in conflitto (anche se `rerere` li ha già risolti).
    pub conflicts: Vec<RepoPath>,
    /// Il testo di git (stdout e stderr) di un merge fermo; vuoto altrimenti.
    pub message: String,
    /// Avvisi scoperti dopo un merge riuscito (es. un hook che ha spostato HEAD).
    pub warnings: Vec<String>,
}

/// Esito di [`GitBackend::merge_abort`](super::GitBackend::merge_abort).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeAborted {
    /// Il ref di backup di indice e worktree prima dell'annullamento
    /// (`refs/project/backup/merge-abort-...`): un commit con i file del worktree (conflitti
    /// e risoluzioni compresi) che ha per genitori HEAD e un commit con l'indice, come uno
    /// stash (si recupera con `git stash apply <ref>` o file per file con `git show
    /// <ref>:<percorso>`).
    pub backup: String,
    /// I file salvati nel backup perché diversi da HEAD (nell'indice o nel worktree).
    pub files: Vec<RepoPath>,
    /// Avvisi dopo un annullamento riuscito.
    pub warnings: Vec<String>,
}

/// L'operazione in corso e i suoi dati, per il banner della UI (`repo.operation`). Letta dai
/// file che git lascia nella sua cartella.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationState {
    /// Come [`Status::operation`].
    pub operation: Option<Operation>,
    /// I commit in ingresso: `MERGE_HEAD` (uno o più), `CHERRY_PICK_HEAD`, `REVERT_HEAD`.
    /// Vuoto per rebase, bisect, `git am` e senza operazione.
    pub incoming: Vec<Oid>,
    /// Il messaggio preparato da git (`MERGE_MSG`: merge, cherry-pick, revert), com'è nel
    /// file (con le righe di commento `# Conflicts:`); `None` se non c'è.
    pub message: Option<String>,
    /// Rebase: il commit su cui si riapplica (`rebase-merge/onto` o `rebase-apply/onto`).
    pub onto: Option<Oid>,
    /// Rebase: il branch che si sta riscrivendo (`head-name`, nome completo), `None` se il
    /// rebase è partito da HEAD staccata.
    pub rebase_branch: Option<String>,
    /// Vero se c'è una sequenza di cherry-pick o revert di più commit avviata da terminale
    /// (`.git/sequencer`): i comandi di S31 la lasciano al terminale (`check_picking`).
    #[serde(default)]
    pub sequencer: bool,
}

/// Da dove viene il commit da unire, per il messaggio di default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeSourceKind {
    Branch,
    RemoteBranch,
    Tag,
    Commit,
}

/// Il commit da unire con il suo nome ([`merge_source`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeSource {
    pub kind: MergeSourceKind,
    /// Nome breve (`feature`, `origin/x`, `v1.0`) o, per un commit, l'id completo.
    pub name: String,
    /// Il nome completo del ref, se è un ref.
    pub full: Option<String>,
}

impl MergeSource {
    /// Il messaggio di default del commit di merge in `branch`, come lo scrive `git merge`:
    /// `Merge branch 'x'`, `Merge remote-tracking branch 'origin/x'`, `Merge tag 'v1'`, `Merge
    /// commit '<id>'`, con ` into <branch>` se il branch non è `main` o `master` (come git con
    /// `merge.suppressDest` di default).
    pub fn message(&self, branch: &str) -> String {
        let what = match self.kind {
            MergeSourceKind::Branch => "branch",
            MergeSourceKind::RemoteBranch => "remote-tracking branch",
            MergeSourceKind::Tag => "tag",
            MergeSourceKind::Commit => "commit",
        };
        let mut message = format!("Merge {what} '{}'", self.name);
        if branch != "main" && branch != "master" {
            message.push_str(&format!(" into {branch}"));
        }
        message
    }
}

/// Il nome della rev `rev` che si è risolta in `target`: il primo ref, nell'ordine in cui git
/// risolve un nome (`<rev>` completo, `refs/<rev>`, `refs/tags/<rev>`, `refs/heads/<rev>`,
/// `refs/remotes/<rev>`), che esiste e punta a `target`; altrimenti il commit stesso (un id,
/// `HEAD~2`, un ref che punta altrove).
pub fn merge_source(rev: &str, refs: &[RefInfo], target: &Oid) -> MergeSource {
    let candidates = [
        rev.to_owned(),
        format!("refs/{rev}"),
        format!("{TAGS_PREFIX}{rev}"),
        format!("{HEADS_PREFIX}{rev}"),
        format!("{REMOTES_PREFIX}{rev}"),
    ];
    for name in &candidates {
        let Some(r) = refs.iter().find(|r| r.name == *name) else {
            continue;
        };
        if r.target != *target {
            break;
        }
        let (kind, short) = match r.kind {
            RefKind::LocalBranch => (
                MergeSourceKind::Branch,
                name.strip_prefix(HEADS_PREFIX).unwrap_or(name),
            ),
            RefKind::RemoteBranch => (
                MergeSourceKind::RemoteBranch,
                name.strip_prefix(REMOTES_PREFIX).unwrap_or(name),
            ),
            RefKind::Tag => (
                MergeSourceKind::Tag,
                name.strip_prefix(TAGS_PREFIX).unwrap_or(name),
            ),
            RefKind::Stash => break,
        };
        return MergeSource {
            kind,
            name: short.to_owned(),
            full: Some(name.clone()),
        };
    }
    MergeSource {
        kind: MergeSourceKind::Commit,
        name: target.to_string(),
        full: None,
    }
}

/// I file in conflitto dello stato (voci `u`).
pub fn conflicted_paths(status: &Status) -> Vec<RepoPath> {
    status
        .entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
        .map(|e| e.path.clone())
        .collect()
}

fn quoted(paths: &[RepoPath]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// I controlli prima di `merge.start`, uguali per i backend, sullo stato letto ora:
///
/// - un'operazione in corso (anche un altro merge) → [`EngineError::NotAllowed`];
/// - file in conflitto → [`EngineError::NotAllowed`];
/// - per un commit di merge, **qualunque** modifica ai file tracciati (in stage o no) →
///   [`EngineError::NotAllowed`] (scelta, come il pull): un merge che si ferma in conflitto
///   mescolerebbe le modifiche locali con quelle del merge, e `git merge --abort` non sa
///   sempre ricostruirle. Mai `--autostash`. I non tracciati vanno bene (git non li
///   sovrascrive). Il fast-forward accetta modifiche che non toccano i file cambiati: git
///   rifiuta da sé, prima di scrivere, di sovrascrivere le altre.
pub fn check_merge_state(status: &Status, mode: &MergeMode) -> Result<(), EngineError> {
    check_no_operation(
        "merge.start",
        "inizierebbe un altro merge",
        status.operation,
    )?;
    let unmerged = conflicted_paths(status);
    if !unmerged.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "ci sono file in conflitto ({}): risolverli prima del merge; niente modificato",
            quoted(&unmerged)
        )));
    }
    if matches!(mode, MergeMode::FastForward) {
        return Ok(());
    }
    let changed: Vec<RepoPath> = status
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                EntryKind::Ordinary(_) | EntryKind::Renamed { .. } | EntryKind::Copied { .. }
            ) && (e.index.is_change() || e.worktree.is_change())
        })
        .map(|e| e.path.clone())
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    Err(EngineError::NotAllowed(format!(
        "il merge creerebbe un commit di merge ma ci sono modifiche locali ai file tracciati \
         ({}): committarle o metterle da parte (`stash.push`) e riprovare (niente \
         `--autostash`); niente modificato",
        quoted(&changed)
    )))
}

/// Il controllo comune a `merge.abort` e `merge.continue`: deve essere in corso un merge.
pub fn check_merging(command: &str, status: &Status) -> Result<(), EngineError> {
    check_in_progress(command, Operation::Merge, status)
}

/// Il nome di un'operazione per i messaggi (`un merge`, `un cherry-pick`, ...).
pub(crate) fn operation_name(op: Operation) -> &'static str {
    match op {
        Operation::Rebase => "un rebase",
        Operation::CherryPick => "un cherry-pick",
        Operation::Revert => "un revert",
        Operation::Bisect => "un bisect",
        Operation::Am => "un `git am`",
        Operation::Merge => "un merge",
    }
}

/// Deve essere in corso l'operazione `expected` (per i comandi che la concludono o la
/// annullano), altrimenti [`EngineError::NotAllowed`].
pub(crate) fn check_in_progress(
    command: &str,
    expected: Operation,
    status: &Status,
) -> Result<(), EngineError> {
    match status.operation {
        Some(op) if op == expected => Ok(()),
        None => Err(EngineError::NotAllowed(format!(
            "non c'è {} in corso: `{command}` non ha niente da fare, niente modificato",
            operation_name(expected)
        ))),
        Some(other) => Err(EngineError::NotAllowed(format!(
            "è in corso {}, non {}: `{command}` non lo tocca, niente modificato. \
             Concluderlo o annullarlo con i suoi comandi (o da terminale)",
            operation_name(other),
            operation_name(expected)
        ))),
    }
}

/// Il messaggio di `MERGE_MSG` senza il blocco dei conflitti che git aggiunge in fondo
/// (`# Conflicts:` e le righe `#\t<file>` dopo, con il carattere di commento di
/// `core.commentChar`): `git commit -F` non toglie i commenti con la pulizia `whitespace`, e
/// finirebbero nel messaggio. Le altre righe restano com'erano.
pub fn clean_merge_message(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| {
        let mut chars = l.chars();
        matches!(chars.next(), Some(c) if !c.is_alphanumeric() && !c.is_whitespace())
            && chars.as_str() == " Conflicts:"
    });
    let kept = match start {
        Some(i) => {
            let marker = lines[i].chars().next().expect("riga non vuota");
            let rest_ok = lines[i + 1..]
                .iter()
                .all(|l| l.starts_with(marker) || l.trim().is_empty());
            if rest_ok { &lines[..i] } else { &lines[..] }
        }
        None => &lines[..],
    };
    kept.join("\n").trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(name: &str, kind: RefKind, target: Oid) -> RefInfo {
        RefInfo {
            name: name.to_owned(),
            kind,
            target,
            tag_object: None,
            upstream: None,
        }
    }

    #[test]
    fn sources_and_messages() {
        let a = Oid::from_bytes([1; Oid::LEN]);
        let b = Oid::from_bytes([2; Oid::LEN]);
        let refs = [
            r("refs/heads/feature", RefKind::LocalBranch, a),
            r("refs/remotes/origin/x", RefKind::RemoteBranch, a),
            r("refs/tags/v1", RefKind::Tag, a),
            r("refs/heads/v1", RefKind::LocalBranch, b),
        ];
        let s = merge_source("feature", &refs, &a);
        assert_eq!(s.kind, MergeSourceKind::Branch);
        assert_eq!(s.message("main"), "Merge branch 'feature'");
        assert_eq!(s.message("dev"), "Merge branch 'feature' into dev");
        let s = merge_source("refs/heads/feature", &refs, &a);
        assert_eq!(
            (s.kind, s.name.as_str()),
            (MergeSourceKind::Branch, "feature")
        );
        let s = merge_source("origin/x", &refs, &a);
        assert_eq!(
            s.message("master"),
            "Merge remote-tracking branch 'origin/x'"
        );
        // Il tag viene prima del branch con lo stesso nome, come in git.
        let s = merge_source("v1", &refs, &a);
        assert_eq!(s.message("main"), "Merge tag 'v1'");
        let s = merge_source(&b.to_string()[..8], &refs, &b);
        assert_eq!(s.kind, MergeSourceKind::Commit);
        assert_eq!(s.message("main"), format!("Merge commit '{b}'"));
    }

    #[test]
    fn merge_msg_cleanup() {
        assert_eq!(
            clean_merge_message("Merge branch 'x'\n\n# Conflicts:\n#\ta.txt\n#\tb c.txt\n"),
            "Merge branch 'x'"
        );
        assert_eq!(
            clean_merge_message("Titolo\n\n#123 resta\n\n; Conflicts:\n;\tf\n"),
            "Titolo\n\n#123 resta"
        );
        // Testo dopo il blocco: non è il blocco di git, resta tutto.
        assert_eq!(
            clean_merge_message("T\n# Conflicts:\naltro\n"),
            "T\n# Conflicts:\naltro"
        );
        assert_eq!(clean_merge_message("Solo titolo\n"), "Solo titolo");
    }
}
