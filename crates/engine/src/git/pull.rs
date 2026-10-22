//! Pull (`remote.pull`, S26): tipi e controlli comuni ai backend
//! (`impl-docs/ARCHITETTURA-git.md` §4, §6 e §7).
//!
//! Il pull è guidato dal motore in due tempi: prima il fetch del solo branch dell'upstream
//! ([`GitBackend::fetch`](super::GitBackend::fetch), lento e annullabile), poi l'integrazione
//! in locale ([`GitBackend::pull`](super::GitBackend::pull): fast-forward, merge o rebase), che
//! non si annulla a metà. Così il comando sa cosa è arrivato prima di scrivere, decide la
//! strategia ([`PullSettings::policy`], [`pull_mode`]), controlla lo stato del repo
//! ([`check_pull_state`]) e registra nel diario lo spostamento del branch.

use super::{HEADS_PREFIX, check_no_operation};
use crate::EngineError;
use project_domain::{EntryKind, Oid, Status};
use serde::{Deserialize, Serialize};

/// L'upstream configurato di un branch locale (`branch.<nome>.remote` e `branch.<nome>.merge`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamConfig {
    /// Il remoto (`branch.<nome>.remote`); `.` vuol dire il repo stesso (upstream locale).
    pub remote: String,
    /// Il branch sul remoto, nome completo (`branch.<nome>.merge`, es. `refs/heads/main`).
    pub merge: String,
    /// Il branch remoto locale che lo rispecchia secondo il refspec di fetch del remoto (es.
    /// `refs/remotes/origin/main`); `None` se nessun refspec lo mappa.
    pub tracking: Option<String>,
}

impl UpstreamConfig {
    /// Il nome breve del branch sul remoto (`main` per `refs/heads/main`), se è un branch.
    pub fn merge_branch(&self) -> Option<&str> {
        self.merge.strip_prefix(HEADS_PREFIX)
    }
}

/// La configurazione della strategia di pull, con i valori come li scrive l'utente in git
/// config (letti dal backend, interpretati da [`PullSettings::policy`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullSettings {
    /// `branch.<nome>.rebase`, altrimenti `pull.rebase` (`None`: nessuno dei due). Una chiave
    /// senza valore è `""` (vale `true`).
    pub rebase: Option<String>,
    /// `pull.ff` (`true`, `false`, `only`).
    pub ff: Option<String>,
}

/// La strategia chiesta con il parametro `strategy` di `remote.pull`, che sostituisce la
/// configurazione.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullStrategy {
    /// Fast-forward se si può, altrimenti un commit di merge.
    Merge,
    /// Fast-forward se si può, altrimenti rebase dei commit locali sull'upstream.
    Rebase,
    /// Solo fast-forward: se i branch sono divergenti, errore e niente cambia.
    FfOnly,
}

/// Da dove viene la strategia usata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategySource {
    /// Il parametro `strategy`.
    Param,
    /// `pull.rebase`, `branch.<nome>.rebase` o `pull.ff`.
    Config,
    /// Nessuna configurazione: merge (vedi [`PullSettings::policy`]).
    Default,
}

/// La strategia decisa per un pull.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullPolicy {
    pub strategy: PullStrategy,
    /// `pull.ff=false`: anche quando si potrebbe fare fast-forward si crea un commit di merge
    /// (solo con [`PullStrategy::Merge`]).
    pub no_ff: bool,
    /// `pull.rebase=merges`: rebase con `--rebase-merges`.
    pub rebase_merges: bool,
    pub source: StrategySource,
}

/// Un valore booleano di git config (`true`/`yes`/`on`/`1`, `false`/`no`/`off`/`0`; la chiave
/// senza valore vale `true`).
fn config_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "" | "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

impl PullSettings {
    /// La strategia: `explicit` (il parametro `strategy`) se c'è, altrimenti la configurazione
    /// come la legge `git pull`:
    ///
    /// - `branch.<nome>.rebase` / `pull.rebase`: `true` → rebase, `merges` → rebase con
    ///   `--rebase-merges`, `false` → merge; `interactive` → [`EngineError::NotAllowed`] (un
    ///   rebase interattivo vuole un editor);
    /// - `pull.ff`: `only` → solo fast-forward, `false` → sempre un commit di merge (con la
    ///   strategia merge), `true` → di default.
    ///
    /// Senza configurazione: **merge** (scelta). Git dalla 2.33 in poi in quel caso rifiuta il
    /// pull di branch divergenti e chiede di configurare `pull.rebase`; qui si fa come git
    /// prima della 2.33, perché un merge non riscrive niente e si annulla con `edit.undo`, e il
    /// risultato dice la strategia usata e da dove viene (`strategy_source`).
    pub fn policy(&self, explicit: Option<PullStrategy>) -> Result<PullPolicy, EngineError> {
        if let Some(strategy) = explicit {
            return Ok(PullPolicy {
                strategy,
                no_ff: false,
                rebase_merges: false,
                source: StrategySource::Param,
            });
        }
        let invalid = |key: &str, value: &str| {
            EngineError::NotAllowed(format!(
                "valore di `{key}` non riconosciuto: `{}`; niente modificato (correggere la \
                 configurazione o indicare `strategy`)",
                value.escape_debug()
            ))
        };
        let (mut strategy, mut rebase_merges, mut configured) = (PullStrategy::Merge, false, false);
        if let Some(value) = &self.rebase {
            configured = true;
            match value.to_ascii_lowercase().as_str() {
                "merges" | "m" => {
                    strategy = PullStrategy::Rebase;
                    rebase_merges = true;
                }
                "interactive" | "i" => {
                    return Err(EngineError::NotAllowed(
                        "la configurazione chiede un rebase interattivo (`pull.rebase` o \
                         `branch.<nome>.rebase` = `interactive`), che vuole un editor: niente \
                         modificato. Indicare `strategy` (`merge`, `rebase` o `ff_only`)"
                            .to_owned(),
                    ));
                }
                other => match config_bool(other) {
                    Some(true) => strategy = PullStrategy::Rebase,
                    Some(false) => {}
                    None => return Err(invalid("pull.rebase", value)),
                },
            }
        }
        let mut no_ff = false;
        if let Some(value) = &self.ff {
            configured = true;
            match value.to_ascii_lowercase().as_str() {
                "only" => strategy = PullStrategy::FfOnly,
                other => match config_bool(other) {
                    Some(true) => {}
                    Some(false) => no_ff = strategy == PullStrategy::Merge,
                    None => return Err(invalid("pull.ff", value)),
                },
            }
        }
        Ok(PullPolicy {
            strategy,
            no_ff,
            rebase_merges,
            source: if configured {
                StrategySource::Config
            } else {
                StrategySource::Default
            },
        })
    }
}

/// Come integrare l'upstream nel branch ([`GitBackend::pull`](super::GitBackend::pull)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullMode {
    /// Il branch avanza sull'upstream (`git merge --ff-only`, o `update-ref` per un branch che
    /// non è quello di HEAD).
    FastForward,
    /// Un commit di merge (genitori: il branch e l'upstream).
    Merge { no_ff: bool },
    /// I commit locali riscritti sopra l'upstream.
    Rebase { merges: bool },
}

impl PullMode {
    /// Il nome nel risultato (`strategy`): `fast_forward`, `merge`, `rebase`.
    pub fn name(&self) -> &'static str {
        match self {
            PullMode::FastForward => "fast_forward",
            PullMode::Merge { .. } => "merge",
            PullMode::Rebase { .. } => "rebase",
        }
    }
}

/// Cosa fare dati i commit del branch che l'upstream non ha (`ahead`) e quelli dell'upstream
/// che il branch non ha (`behind`): `None` se il branch contiene già l'upstream (niente da
/// fare). Un branch che non è quello di HEAD (`current == false`) si può solo portare avanti
/// (fast-forward), senza toccare il worktree; branch divergenti con `ff_only` →
/// [`EngineError::NotAllowed`].
pub fn pull_mode(
    policy: &PullPolicy,
    branch: &str,
    current: bool,
    ahead: usize,
    behind: usize,
) -> Result<Option<PullMode>, EngineError> {
    if behind == 0 {
        return Ok(None);
    }
    if ahead == 0 {
        if current && policy.strategy == PullStrategy::Merge && policy.no_ff {
            return Ok(Some(PullMode::Merge { no_ff: true }));
        }
        return Ok(Some(PullMode::FastForward));
    }
    if !current {
        return Err(EngineError::NotAllowed(format!(
            "`{}` e il suo upstream sono divergenti ({ahead} commit locali, {behind} \
             dell'upstream) e `{0}` non è il branch attivo: si può solo fare fast-forward. Il \
             fetch è fatto, niente altro modificato; passare al branch (`branch.checkout`) e \
             ripetere il pull",
            branch.escape_debug()
        )));
    }
    match policy.strategy {
        PullStrategy::FfOnly => Err(EngineError::NotAllowed(format!(
            "`{}` e il suo upstream sono divergenti ({ahead} commit locali, {behind} \
             dell'upstream) e la strategia è solo fast-forward (`pull.ff=only` o `strategy` \
             `ff_only`): il fetch è fatto, niente altro modificato. Ripetere con `strategy` \
             `merge` o `rebase`",
            branch.escape_debug()
        ))),
        PullStrategy::Rebase => Ok(Some(PullMode::Rebase {
            merges: policy.rebase_merges,
        })),
        PullStrategy::Merge => Ok(Some(PullMode::Merge {
            no_ff: policy.no_ff,
        })),
    }
}

/// I controlli prima di integrare l'upstream, uguali per i backend, sullo stato letto ora:
///
/// - merge, rebase, cherry-pick, revert, bisect o `git am` in corso → [`EngineError::NotAllowed`];
/// - file in conflitto → [`EngineError::NotAllowed`];
/// - per merge e rebase, **qualunque** modifica ai file tracciati (in stage o no) →
///   [`EngineError::NotAllowed`]: un merge o un rebase che si fermano in conflitto con
///   modifiche locali non si annullano in modo sicuro (`git merge --abort` può non ricostruirle)
///   e un rebase non parte. Mai `--autostash`: le modifiche vanno committate o messe da parte
///   prima. I non tracciati vanno bene (git rifiuta di sovrascriverli). Il fast-forward invece
///   accetta modifiche che non toccano i file cambiati: git rifiuta da sé di sovrascrivere le
///   altre.
pub fn check_pull_state(status: &Status, mode: &PullMode) -> Result<(), EngineError> {
    check_no_operation("remote.pull", "integrerebbe l'upstream", status.operation)?;
    let unmerged: Vec<String> = status
        .entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
        .map(|e| format!("`{}`", e.path))
        .collect();
    if !unmerged.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "ci sono file in conflitto ({}): risolverli prima del pull; il branch non è stato \
             toccato",
            unmerged.join(", ")
        )));
    }
    if matches!(mode, PullMode::FastForward) {
        return Ok(());
    }
    let changed: Vec<String> = status
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                EntryKind::Ordinary(_) | EntryKind::Renamed { .. } | EntryKind::Copied { .. }
            ) && (e.index.is_change() || e.worktree.is_change())
        })
        .map(|e| format!("`{}`", e.path))
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    Err(EngineError::NotAllowed(format!(
        "il pull farebbe un {} ma ci sono modifiche locali ai file tracciati ({}): committarle \
         o metterle da parte e riprovare (niente `--autostash`); il branch non è stato toccato",
        mode.name(),
        changed.join(", ")
    )))
}

/// Il messaggio del commit di merge di un pull: come `git merge <branch remoto>`
/// (`Merge remote-tracking branch 'origin/main'`), con ` into <branch>` se il branch non è
/// `main` o `master` (come fa git con `merge.suppressDest` di default).
pub fn merge_message(upstream: &str, branch: &str) -> String {
    let short = upstream.strip_prefix("refs/remotes/").unwrap_or(upstream);
    let mut message = format!("Merge remote-tracking branch '{short}'");
    if branch != "main" && branch != "master" {
        message.push_str(&format!(" into {branch}"));
    }
    message
}

/// Cosa chiede `remote.pull` al backend dopo il fetch ([`GitBackend::pull`](super::GitBackend::pull)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// Il branch locale (nome breve).
    pub branch: String,
    /// Vero se è il branch di HEAD (indice e worktree si aggiornano); altrimenti solo
    /// fast-forward del ref.
    pub current: bool,
    /// Il valore del branch letto prima: il backend rifiuta se è cambiato.
    pub expected: Oid,
    /// Il commit dell'upstream da integrare (il branch remoto dopo il fetch).
    pub upstream: Oid,
    /// Il nome completo del branch remoto (es. `refs/remotes/origin/main`), per i messaggi.
    pub upstream_name: String,
    pub mode: PullMode,
    /// Il messaggio del commit di merge ([`merge_message`]).
    pub message: String,
}

/// Esito di [`GitBackend::pull`](super::GitBackend::pull).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullOutcome {
    /// Il valore del branch dopo.
    pub after: Oid,
    /// Il ref di backup del branch prima del pull (`refs/project/backup/pull-...`).
    pub backup: String,
    /// Avvisi scoperti dopo un'integrazione riuscita (es. un hook che ha spostato HEAD).
    pub warnings: Vec<String>,
}

/// Quale operazione si è fermata in conflitto ([`EngineError::PullConflict`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullOperation {
    Merge,
    Rebase,
}

impl std::fmt::Display for PullOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PullOperation::Merge => "merge",
            PullOperation::Rebase => "rebase",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Change, Head, RepoPath, StatusEntry, TrackedInfo};

    fn settings(rebase: Option<&str>, ff: Option<&str>) -> PullSettings {
        PullSettings {
            rebase: rebase.map(str::to_owned),
            ff: ff.map(str::to_owned),
        }
    }

    #[test]
    fn policy_from_config() {
        let p = settings(None, None).policy(None).unwrap();
        assert_eq!(
            (p.strategy, p.no_ff, p.source),
            (PullStrategy::Merge, false, StrategySource::Default)
        );
        for v in ["true", "yes", "on", "1", "", "TRUE"] {
            let p = settings(Some(v), None).policy(None).unwrap();
            assert_eq!(p.strategy, PullStrategy::Rebase, "{v}");
            assert_eq!(p.source, StrategySource::Config);
        }
        let p = settings(Some("merges"), None).policy(None).unwrap();
        assert!(p.rebase_merges && p.strategy == PullStrategy::Rebase);
        let p = settings(Some("false"), Some("false")).policy(None).unwrap();
        assert!(p.no_ff && p.strategy == PullStrategy::Merge);
        let p = settings(Some("true"), Some("only")).policy(None).unwrap();
        assert_eq!(p.strategy, PullStrategy::FfOnly);
        let p = settings(Some("true"), Some("false")).policy(None).unwrap();
        assert!(!p.no_ff, "pull.ff=false conta solo per il merge");
        for (rebase, ff) in [
            (Some("interactive"), None),
            (Some("x"), None),
            (None, Some("x")),
        ] {
            let err = settings(rebase, ff).policy(None).unwrap_err();
            assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        }
        // Il parametro sostituisce la configurazione, anche se non valida.
        let p = settings(Some("interactive"), Some("only"))
            .policy(Some(PullStrategy::Merge))
            .unwrap();
        assert_eq!(
            (p.strategy, p.no_ff, p.source),
            (PullStrategy::Merge, false, StrategySource::Param)
        );
    }

    #[test]
    fn mode_from_ahead_and_behind() {
        let policy = |strategy, no_ff| PullPolicy {
            strategy,
            no_ff,
            rebase_merges: false,
            source: StrategySource::Default,
        };
        let merge = policy(PullStrategy::Merge, false);
        assert_eq!(pull_mode(&merge, "main", true, 3, 0).unwrap(), None);
        assert_eq!(
            pull_mode(&merge, "main", true, 0, 2).unwrap(),
            Some(PullMode::FastForward)
        );
        assert_eq!(
            pull_mode(&merge, "main", true, 1, 2).unwrap(),
            Some(PullMode::Merge { no_ff: false })
        );
        assert_eq!(
            pull_mode(&policy(PullStrategy::Merge, true), "main", true, 0, 2).unwrap(),
            Some(PullMode::Merge { no_ff: true })
        );
        // Un branch non attivo: solo fast-forward, anche con `no_ff`.
        assert_eq!(
            pull_mode(&policy(PullStrategy::Merge, true), "x", false, 0, 2).unwrap(),
            Some(PullMode::FastForward)
        );
        assert!(matches!(
            pull_mode(&merge, "x", false, 1, 2),
            Err(EngineError::NotAllowed(_))
        ));
        assert_eq!(
            pull_mode(&policy(PullStrategy::Rebase, false), "main", true, 1, 2).unwrap(),
            Some(PullMode::Rebase { merges: false })
        );
        assert!(matches!(
            pull_mode(&policy(PullStrategy::FfOnly, false), "main", true, 1, 2),
            Err(EngineError::NotAllowed(_))
        ));
    }

    fn entry(path: &str, index: Change, worktree: Change, kind: EntryKind) -> StatusEntry {
        StatusEntry {
            path: RepoPath::from(path),
            index,
            worktree,
            kind,
        }
    }

    #[test]
    fn state_checks() {
        let info = TrackedInfo {
            submodule: None,
            mode_head: 0o100644,
            mode_index: 0o100644,
            mode_worktree: 0o100644,
            oid_head: Oid::from_bytes([1; Oid::LEN]),
            oid_index: Oid::from_bytes([1; Oid::LEN]),
        };
        let mut status = Status {
            head: Head::Branch {
                name: "main".into(),
                target: None,
            },
            upstream: None,
            entries: vec![entry(
                "nuovo",
                Change::Untracked,
                Change::Untracked,
                EntryKind::Untracked,
            )],
            operation: None,
        };
        for mode in [
            PullMode::FastForward,
            PullMode::Merge { no_ff: false },
            PullMode::Rebase { merges: false },
        ] {
            check_pull_state(&status, &mode).unwrap();
        }
        status.entries.push(entry(
            "a",
            Change::Unmodified,
            Change::Modified,
            EntryKind::Ordinary(info),
        ));
        check_pull_state(&status, &PullMode::FastForward).unwrap();
        let err = check_pull_state(&status, &PullMode::Rebase { merges: false }).unwrap_err();
        assert!(err.to_string().contains("`a`"), "{err}");
        status.entries.pop();
        status.operation = Some(project_domain::Operation::Merge);
        assert!(check_pull_state(&status, &PullMode::FastForward).is_err());
    }

    #[test]
    fn merge_messages() {
        assert_eq!(
            merge_message("refs/remotes/origin/main", "main"),
            "Merge remote-tracking branch 'origin/main'"
        );
        assert_eq!(
            merge_message("refs/remotes/origin/x", "x"),
            "Merge remote-tracking branch 'origin/x' into x"
        );
    }
}
