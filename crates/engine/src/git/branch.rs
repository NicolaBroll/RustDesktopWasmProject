//! Branch locali (`branch.create`, `branch.checkout`, `branch.delete`, `branch.rename`, S24):
//! controlli comuni ai backend (`impl-docs/git/branch.md`).
//!
//! I nomi passati ai comandi sono **brevi** (`feature/x`, non `refs/heads/feature/x`); il nome
//! completo lo costruisce [`branch_ref`]. Un nome nuovo (creazione, rinomina) deve essere un
//! nome di branch valido per git ([`check_new_branch_name`]: niente `-` iniziale, niente `HEAD`,
//! le regole di `git check-ref-format`); un branch esistente si può indicare anche se il nome
//! non sarebbe più accettato oggi (es. `-x`, creato con `update-ref`), purché sia un nome di ref
//! valido ([`check_branch_name`]): così si può ancora rinominarlo o eliminarlo.

use super::valid_ref_name;
use crate::EngineError;
use project_domain::Operation;
use serde::{Deserialize, Serialize};

/// Prefisso dei branch locali.
pub const HEADS_PREFIX: &str = "refs/heads/";

/// Il nome completo del branch locale `name` (`refs/heads/<name>`).
pub fn branch_ref(name: &str) -> String {
    format!("{HEADS_PREFIX}{name}")
}

/// Il nome breve di un branch locale: `refs/heads/x` → `x`; un altro nome resta com'è.
pub fn branch_short(full: &str) -> &str {
    full.strip_prefix(HEADS_PREFIX).unwrap_or(full)
}

/// Il nome di un branch esistente (`branch.checkout`, `branch.delete`, il `name` di
/// `branch.rename`): un nome breve che dà un nome di ref valido ([`valid_ref_name`]). Un nome
/// che inizia con `-` è accettato (può esistere, creato con `update-ref`): ai backend arriva
/// sempre dopo `--end-of-options` o come `refs/heads/<nome>`. Altrimenti
/// [`EngineError::InvalidParams`].
pub fn check_branch_name(id: &'static str, name: &str) -> Result<(), EngineError> {
    let reason = if name.is_empty() {
        Some("il nome del branch è vuoto")
    } else if name.starts_with("refs/") {
        Some("usare il nome breve del branch (senza `refs/heads/`)")
    } else if !valid_ref_name(&branch_ref(name)) {
        Some("non è un nome di branch valido")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(EngineError::InvalidParams {
            id,
            reason: format!("{reason}: `{}`", name.escape_debug()),
        }),
        None => Ok(()),
    }
}

/// Il nome di un branch **nuovo** (`branch.create`, il `new_name` di `branch.rename`): come
/// [`check_branch_name`] e in più le regole di `git check-ref-format --branch`, che git
/// applica ai nomi nuovi: non inizia con `-`, non è `HEAD` né `@`. Il `CliBackend` lo fa
/// verificare anche da git prima di scrivere. Altrimenti [`EngineError::InvalidParams`].
pub fn check_new_branch_name(id: &'static str, name: &str) -> Result<(), EngineError> {
    check_branch_name(id, name)?;
    let reason = if name.starts_with('-') {
        Some("un nome di branch non può iniziare con `-`")
    } else if name == "HEAD" || name == "@" {
        Some("nome riservato da git")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(EngineError::InvalidParams {
            id,
            reason: format!("{reason}: `{}`", name.escape_debug()),
        }),
        None => Ok(()),
    }
}

/// [`EngineError::NotAllowed`] se è in corso un'operazione (merge, rebase, cherry-pick, revert,
/// bisect, `git am`): `command` `what` (es. "cambierebbe branch") a metà dell'operazione.
pub fn check_no_operation(
    command: &str,
    what: &str,
    operation: Option<Operation>,
) -> Result<(), EngineError> {
    let Some(op) = operation else {
        return Ok(());
    };
    let name = match op {
        Operation::Merge => "un merge",
        Operation::Rebase => "un rebase",
        Operation::CherryPick => "un cherry-pick",
        Operation::Revert => "un revert",
        Operation::Bisect => "un bisect",
        Operation::Am => "un `git am`",
    };
    Err(EngineError::NotAllowed(format!(
        "è in corso {name}: `{command}` {what} a metà dell'operazione, niente modificato. \
         Concludere o annullare l'operazione e riprovare"
    )))
}

/// Esito di [`GitBackend::delete_branch`](crate::GitBackend::delete_branch).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchDeleted {
    /// Il ref di backup sul commit del branch (`refs/project/backup/delete-...`), creato prima
    /// di eliminarlo.
    pub backup: String,
    /// La configurazione `branch.<nome>.*` del repo tolta con il branch (upstream,
    /// descrizione, ...), nell'ordine del file: `edit.undo` la riscrive.
    pub config: Vec<ConfigEntry>,
    /// Avvisi su qualcosa che non è riuscito **dopo** l'eliminazione (es. la configurazione
    /// `branch.<nome>.*` non tolta, o un valore non UTF-8 non salvato). Di solito vuoto.
    pub warnings: Vec<String>,
}

/// Una voce della configurazione del repo, es. `{"key": "branch.x.remote", "value":
/// "origin"}`. `value` `null` per una chiave senza valore (`[branch "x"] flag`, vale `true`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEntry {
    pub key: String,
    pub value: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for good in ["main", "feature/x-1", "-x", "a.b", "über"] {
            check_branch_name("t", good).unwrap();
        }
        for bad in [
            "",
            "refs/heads/main",
            "a b",
            "a..b",
            "a@{1}",
            ".x",
            "x.lock",
            "x/",
            "a//b",
            "a:b",
            "a\nb",
        ] {
            assert!(
                matches!(
                    check_branch_name("t", bad),
                    Err(EngineError::InvalidParams { .. })
                ),
                "{bad:?}"
            );
        }
        check_new_branch_name("t", "feature/x").unwrap();
        for bad in ["-x", "--force", "HEAD", "@", "a b", ""] {
            assert!(check_new_branch_name("t", bad).is_err(), "{bad:?}");
        }
        assert_eq!(branch_ref("x"), "refs/heads/x");
        assert_eq!(branch_short("refs/heads/a/b"), "a/b");
        assert_eq!(branch_short("refs/tags/v1"), "refs/tags/v1");
    }

    #[test]
    fn operations() {
        check_no_operation("branch.checkout", "cambierebbe branch", None).unwrap();
        let err = check_no_operation(
            "branch.checkout",
            "cambierebbe branch",
            Some(Operation::Rebase),
        )
        .unwrap_err();
        assert!(err.to_string().contains("un rebase"), "{err}");
    }
}
