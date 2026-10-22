//! Spostamento controllato dei ref per `edit.undo` e `edit.redo`
//! (`impl-docs/ARCHITETTURA-git.md` §7).
//!
//! Il diario delle operazioni ([`Journal`](crate::Journal)) ricorda, per ogni comando che ha
//! spostato ref, HEAD e i ref prima e dopo. Annullare vuol dire riportarli ai valori "prima",
//! rifare ai valori "dopo", e **solo** se nel frattempo nessuno li ha toccati: una
//! [`RefRestore`] dice ai backend che valori si aspettano ora (`expected`) e quali scrivere
//! (`target`). I controlli comuni ai backend sono qui, così `MemoryBackend` e `CliBackend`
//! rifiutano gli stessi casi con gli stessi errori.

use crate::EngineError;
use project_domain::{Oid, Operation};
use serde::{Deserialize, Serialize};

/// Dove punta HEAD in un momento, per il diario: su un branch (nome completo, es.
/// `refs/heads/main`, anche se il branch non ha ancora commit) o staccata su un commit.
///
/// JSON: `{"branch": "refs/heads/main"}` oppure `{"detached": "<oid>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadState {
    Branch(String),
    Detached(Oid),
}

impl std::fmt::Display for HeadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeadState::Branch(name) => write!(f, "sul branch `{name}`"),
            HeadState::Detached(oid) => write!(f, "staccata su {}", short(oid)),
        }
    }
}

/// Un ref da riportare da `expected` (il valore che deve avere ora; `None` = non deve esistere)
/// a `target` (`None` = da cancellare).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefMove {
    /// Nome completo, sotto `refs/` (es. `refs/heads/main`).
    pub name: String,
    pub expected: Option<Oid>,
    pub target: Option<Oid>,
}

/// Cosa chiede `edit.undo`/`edit.redo` al backend ([`GitBackend::restore_refs`]): HEAD e i
/// ref devono valere ora `expected`, e vanno portati a `target` tutti insieme o nessuno.
///
/// [`GitBackend::restore_refs`]: crate::GitBackend::restore_refs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRestore {
    /// `edit.undo` o `edit.redo`: negli errori e nel nome dei ref di backup.
    pub command: &'static str,
    /// HEAD come deve essere ora.
    pub head_expected: HeadState,
    /// HEAD come deve diventare. Oggi solo lo stesso branch di `head_expected` (HEAD non si
    /// tocca) o un altro commit con HEAD staccata; un cambio di branch arriverà con
    /// `branch.checkout`.
    pub head_target: HeadState,
    pub refs: Vec<RefMove>,
    /// Messaggio del reflog (`update-ref -m`), es. `project: undo Commit a0744b4 «...»`.
    pub reflog: String,
}

impl RefRestore {
    /// Il tipo di backup nel nome dei ref (`refs/project/backup/undo-...`): `undo` o `redo`.
    pub fn backup_kind(&self) -> &'static str {
        self.command.strip_prefix("edit.").unwrap_or("undo")
    }

    /// I controlli prima di toccare qualcosa, uguali per tutti i backend (`operation` è
    /// l'operazione in corso nel repo, come in [`Status::operation`]); niente è cambiato quando
    /// danno errore.
    ///
    /// [`Status::operation`]: project_domain::Status::operation
    ///
    /// - Merge, rebase, cherry-pick, revert o bisect in corso → [`EngineError::NotAllowed`]:
    ///   spostare un branch a metà di un'operazione la cambierebbe in modo poco visibile (es.
    ///   un merge che si concluderebbe su un altro commit).
    /// - Nomi di ref non validi (non sotto `refs/`, con spazi, caratteri di controllo o
    ///   sequenze vietate da `git check-ref-format`) → [`EngineError::NotAllowed`]: non
    ///   arrivano mai a git.
    /// - HEAD che dovrebbe cambiare branch, o passare fra branch e staccata →
    ///   [`EngineError::NotAllowed`]: non ancora supportato (nessun comando del diario lo fa).
    pub fn check(&self, operation: Option<Operation>) -> Result<(), EngineError> {
        if let Some(op) = operation {
            let name = match op {
                Operation::Merge => "un merge",
                Operation::Rebase => "un rebase",
                Operation::CherryPick => "un cherry-pick",
                Operation::Revert => "un revert",
                Operation::Bisect => "un bisect",
            };
            return Err(EngineError::NotAllowed(format!(
                "è in corso {name}: `{}` sposterebbe i ref a metà dell'operazione, niente \
                 modificato. Concludere o annullare l'operazione e riprovare",
                self.command
            )));
        }
        for r in &self.refs {
            if !valid_ref_name(&r.name) {
                return Err(EngineError::NotAllowed(format!(
                    "nome di ref non valido nel diario: `{}`, niente modificato",
                    r.name.escape_debug()
                )));
            }
        }
        match (&self.head_expected, &self.head_target) {
            (HeadState::Branch(a), HeadState::Branch(b)) if a == b => {}
            (HeadState::Detached(_), HeadState::Detached(_)) => {}
            _ => {
                return Err(EngineError::NotAllowed(format!(
                    "`{}` dovrebbe spostare HEAD {} a {}: non ancora supportato, niente \
                     modificato",
                    self.command, self.head_expected, self.head_target
                )));
            }
        }
        Ok(())
    }

    /// Le differenze fra lo stato atteso e quello attuale (`head`, e `current` che dà il
    /// valore attuale di un ref per nome, `None` se non esiste), una per riga, per
    /// [`EngineError::UndoConflict`]. Vuoto se tutto è come atteso.
    pub fn conflicts(
        &self,
        head: &HeadState,
        mut current: impl FnMut(&str) -> Result<Option<Oid>, EngineError>,
    ) -> Result<Vec<String>, EngineError> {
        let mut out = Vec::new();
        if *head != self.head_expected {
            out.push(format!(
                "HEAD: doveva essere {}, ora è {head}",
                self.head_expected
            ));
        }
        for r in &self.refs {
            let now = current(&r.name)?;
            if now != r.expected {
                out.push(format!(
                    "`{}`: doveva essere {}, ora è {}",
                    r.name,
                    describe(r.expected),
                    describe(now)
                ));
            }
        }
        Ok(out)
    }

    /// I commit che dopo lo spostamento non sarebbero più puntati da HEAD né dai ref della
    /// richiesta: i valori attesi (dei ref e di HEAD staccata) che non sono fra i nuovi. Il
    /// backend salva in un ref di backup quelli che non restano raggiungibili da altri ref né
    /// dai nuovi valori (§7), così il gc non li cancella e `edit.redo` resta possibile.
    pub fn abandoned(&self) -> Vec<Oid> {
        let targets = self.targets();
        let mut out: Vec<Oid> = self
            .refs
            .iter()
            .filter_map(|r| r.expected)
            .chain(match self.head_expected {
                HeadState::Detached(oid) => Some(oid),
                HeadState::Branch(_) => None,
            })
            .filter(|oid| !targets.contains(oid))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// I nuovi valori: dei ref e di HEAD staccata.
    pub fn targets(&self) -> Vec<Oid> {
        self.refs
            .iter()
            .filter_map(|r| r.target)
            .chain(match self.head_target {
                HeadState::Detached(oid) => Some(oid),
                HeadState::Branch(_) => None,
            })
            .collect()
    }

    /// Il messaggio del commit di backup / del reflog del ref di backup per `oid`.
    pub fn backup_reflog(&self, oid: &Oid) -> String {
        format!(
            "project: backup di {oid} prima di {} ({})",
            self.command, self.reflog
        )
    }
}

/// Vero se `name` è un nome completo di ref che git accetterebbe (`git check-ref-format`) e che
/// si può passare a `update-ref --stdin -z` senza ambiguità: sotto `refs/`, senza spazi né
/// caratteri di controllo, senza `~ ^ : ? * [ \`, `..`, `@{`, componenti vuoti o che iniziano
/// con `.` o finiscono con `.lock`, e che non finisce con `/` o `.`.
pub fn valid_ref_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("refs/") else {
        return false;
    };
    if rest.is_empty()
        || name.ends_with('/')
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("@{")
        || name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
    {
        return false;
    }
    rest.split('/')
        .all(|c| !c.is_empty() && !c.starts_with('.') && !c.ends_with(".lock"))
}

/// Le prime 7 cifre di un id, come `git log --oneline`.
pub(crate) fn short(oid: &Oid) -> String {
    let hex = oid.to_string();
    hex[..7.min(hex.len())].to_owned()
}

fn describe(value: Option<Oid>) -> String {
    match value {
        Some(oid) => short(&oid),
        None => "assente".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryBackend;

    fn oid(n: u64) -> Oid {
        MemoryBackend::fake_oid(n)
    }

    fn restore() -> RefRestore {
        RefRestore {
            command: "edit.undo",
            head_expected: HeadState::Branch("refs/heads/main".to_owned()),
            head_target: HeadState::Branch("refs/heads/main".to_owned()),
            refs: vec![RefMove {
                name: "refs/heads/main".to_owned(),
                expected: Some(oid(2)),
                target: Some(oid(1)),
            }],
            reflog: "project: undo x".to_owned(),
        }
    }

    #[test]
    fn ref_names() {
        for good in [
            "refs/heads/main",
            "refs/heads/feature/x-1",
            "refs/tags/v1.0",
        ] {
            assert!(valid_ref_name(good), "{good}");
        }
        for bad in [
            "HEAD",
            "main",
            "refs/",
            "refs/heads/",
            "refs/heads/a b",
            "refs/heads/a\nb",
            "refs/heads/a\0b",
            "refs/heads/a..b",
            "refs/heads/a@{1}",
            "refs/heads/.x",
            "refs/heads/x.lock",
            "refs/heads/x.",
            "refs/heads//x",
            "refs/heads/a*",
            "refs/heads/a:b",
        ] {
            assert!(!valid_ref_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn checks() {
        restore().check(None).unwrap();
        for op in [Operation::Merge, Operation::Rebase, Operation::Bisect] {
            let err = restore().check(Some(op)).unwrap_err();
            assert!(matches!(err, EngineError::NotAllowed(_)), "{op:?}");
        }
        let mut bad = restore();
        bad.refs[0].name = "refs/heads/a b".to_owned();
        assert!(matches!(bad.check(None), Err(EngineError::NotAllowed(_))));
        let mut switch = restore();
        switch.head_target = HeadState::Branch("refs/heads/altro".to_owned());
        assert!(matches!(
            switch.check(None),
            Err(EngineError::NotAllowed(_))
        ));
        let mut detach = restore();
        detach.head_target = HeadState::Detached(oid(1));
        assert!(detach.check(None).is_err());
    }

    #[test]
    fn conflicts_and_abandoned() {
        let r = restore();
        let head = HeadState::Branch("refs/heads/main".to_owned());
        assert!(r.conflicts(&head, |_| Ok(Some(oid(2)))).unwrap().is_empty());
        let moved = r.conflicts(&head, |_| Ok(Some(oid(3)))).unwrap();
        assert_eq!(moved.len(), 1);
        assert!(moved[0].contains("refs/heads/main"), "{moved:?}");
        let gone = r.conflicts(&head, |_| Ok(None)).unwrap();
        assert!(gone[0].contains("assente"), "{gone:?}");
        let other = HeadState::Detached(oid(2));
        let both = r.conflicts(&other, |_| Ok(Some(oid(2)))).unwrap();
        assert!(both[0].starts_with("HEAD"), "{both:?}");

        assert_eq!(r.abandoned(), [oid(2)]);
        assert_eq!(r.targets(), [oid(1)]);
        let detached = RefRestore {
            head_expected: HeadState::Detached(oid(5)),
            head_target: HeadState::Detached(oid(4)),
            refs: Vec::new(),
            ..restore()
        };
        assert_eq!(detached.abandoned(), [oid(5)]);
        assert_eq!(detached.backup_kind(), "undo");
        let redo = RefRestore {
            command: "edit.redo",
            ..restore()
        };
        assert_eq!(redo.backup_kind(), "redo");
    }

    #[test]
    fn head_state_json() {
        let branch = HeadState::Branch("refs/heads/main".to_owned());
        assert_eq!(
            serde_json::to_value(&branch).unwrap(),
            serde_json::json!({ "branch": "refs/heads/main" })
        );
        let detached = HeadState::Detached(oid(1));
        assert_eq!(
            serde_json::to_value(&detached).unwrap(),
            serde_json::json!({ "detached": oid(1).to_string() })
        );
    }
}
