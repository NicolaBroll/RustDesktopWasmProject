//! `edit.undo` / `edit.redo` sul [`CliBackend`]: HEAD e ref riportati con una transazione
//! `git update-ref --stdin` (§7).
//!
//! In quest'ordine, e niente cambia se un passo fallisce:
//!
//! 1. [`RefRestore::check`] con l'operazione in corso letta da `.git` (merge, rebase, ...);
//! 2. HEAD (`symbolic-ref`, o il commit se staccata) e i ref (`for-each-ref`, nome esatto)
//!    confrontati con i valori attesi: se qualcosa è cambiato da fuori,
//!    [`EngineError::UndoConflict`];
//! 3. per ogni commit che verrebbe abbandonato ([`RefRestore::abandoned`]) e che non resta
//!    raggiungibile da un nuovo valore (`merge-base --is-ancestor`) né da un altro ref
//!    (`for-each-ref --contains`, backup compresi): un ref di backup
//!    `refs/project/backup/<undo|redo>-<secondi>-<nanosecondi>` creato solo se non esiste;
//! 4. una sola transazione `update-ref -m <reflog> -z --stdin` con `start`, poi per ogni ref
//!    `update <ref> <nuovo> <vecchio>` / `create <ref> <nuovo>` / `delete <ref> <vecchio>` (git
//!    controlla il valore vecchio sotto il lock del ref) e, con HEAD staccata,
//!    `option no-deref` + `update HEAD <nuovo> <vecchio>`, infine `commit`: o tutto o niente.
//!    Se fallisce e i ref non sono più quelli attesi, [`EngineError::UndoConflict`];
//!    altrimenti l'errore di git (es. un hook `reference-transaction` che rifiuta). Gli
//!    eventuali ref di backup del passo 3 restano: non fanno danni.
//!
//! Indice e worktree non si toccano mai (`update-ref` non li legge). Quando HEAD punta al
//! branch spostato git scrive la voce anche nel reflog di HEAD. Cancellare un ref (undo del
//! primo commit) cancella anche il suo reflog, come `git update-ref -d`; resta la voce nel
//! reflog di HEAD.
//!
//! Limite: che HEAD punti ancora allo stesso branch si controlla al passo 2, non dentro la
//! transazione (`update-ref` non sa verificare un ref simbolico senza `symref-verify`, che c'è
//! solo da git 2.46): resta l'istante fra il controllo e la transazione, solo con un altro
//! processo git che cambia branch proprio allora. Il valore dei branch invece è controllato
//! sotto lock.

use super::CliBackend;
use crate::command::failure;
use project_domain::Oid;
use project_engine::{EngineError, HeadState, RefMove, RefRestore};
use std::collections::BTreeSet;

impl CliBackend {
    /// [`GitBackend::restore_refs`](project_engine::GitBackend::restore_refs).
    pub(super) fn restore(&mut self, request: &RefRestore) -> Result<Vec<String>, EngineError> {
        request.check(self.operation())?;
        self.check_expected(request)?;
        let backups = self.protect(request)?;

        let args = ["update-ref", "-m", &request.reflog, "-z", "--stdin"];
        let out = self
            .git
            .output_with_input(Some(&self.workdir), &args, &transaction(request))?;
        if !out.status.success() {
            // Qualcuno ha spostato un ref fra il controllo e la transazione?
            self.check_expected(request)?;
            return Err(failure(&args, &out));
        }
        Ok(backups)
    }

    /// HEAD come la registra il diario: il branch a cui punta (nome completo, anche senza
    /// commit) o il commit se staccata.
    fn head_state(&self) -> Result<HeadState, EngineError> {
        match self.head_ref_name()? {
            Some(name) => Ok(HeadState::Branch(name)),
            None => self
                .resolve("HEAD")?
                .map(HeadState::Detached)
                .ok_or_else(|| EngineError::UnknownRev("HEAD".to_owned())),
        }
    }

    /// Il valore del ref `name` (nome completo, validato da [`RefRestore::check`]), `None` se
    /// non esiste. `for-each-ref` con il nome come modello trova anche i ref sotto `name/`:
    /// si tiene solo quello con il nome esatto.
    fn ref_value(&self, name: &str) -> Result<Option<Oid>, EngineError> {
        let args = ["for-each-ref", "--format=%(refname)%00%(objectname)", name];
        let out = self.run(&args)?;
        for line in out.split(|&b| b == b'\n') {
            let mut fields = line.split(|&b| b == 0);
            if fields.next() == Some(name.as_bytes())
                && let Some(hex) = fields.next()
            {
                return Ok(Some(Oid::from_hex(hex)?));
            }
        }
        Ok(None)
    }

    /// [`EngineError::UndoConflict`] se HEAD o un ref non hanno il valore atteso.
    fn check_expected(&self, request: &RefRestore) -> Result<(), EngineError> {
        let head = self.head_state()?;
        let conflicts = request.conflicts(&head, |name| self.ref_value(name))?;
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(EngineError::UndoConflict {
                command: request.command,
                refs: conflicts,
            })
        }
    }

    /// Salva in un ref di backup ogni commit abbandonato che non resterebbe raggiungibile, e
    /// restituisce i nomi dei ref creati. Un errore ferma tutto prima di spostare i ref.
    fn protect(&self, request: &RefRestore) -> Result<Vec<String>, EngineError> {
        let targets = request.targets();
        let moved: BTreeSet<&str> = request.refs.iter().map(|r| r.name.as_str()).collect();
        let mut backups = Vec::new();
        for oid in request.abandoned() {
            let mut kept = false;
            for target in &targets {
                if self.is_ancestor(&oid, target)? {
                    kept = true;
                    break;
                }
            }
            if kept
                || self
                    .refs_containing_any(&oid)?
                    .iter()
                    .any(|name| !moved.contains(name.as_str()))
            {
                continue;
            }
            let name = self.create_backup_ref(
                request.backup_kind(),
                &oid.to_string(),
                &request.backup_reflog(&oid),
            )?;
            backups.push(name);
        }
        Ok(backups)
    }

    /// Vero se il commit `a` è raggiungibile da `b` (o è `b`).
    fn is_ancestor(&self, a: &Oid, b: &Oid) -> Result<bool, EngineError> {
        let (a, b) = (a.to_string(), b.to_string());
        let args = ["merge-base", "--is-ancestor", "--end-of-options", &a, &b];
        let out = self.git.output(Some(&self.workdir), &args)?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) if out.stderr.is_empty() => Ok(false),
            _ => Err(failure(&args, &out)),
        }
    }

    /// Tutti i ref (branch, remoti, tag, stash, backup, ...) da cui il commit `id` è
    /// raggiungibile, nomi completi.
    fn refs_containing_any(&self, id: &Oid) -> Result<Vec<String>, EngineError> {
        let id = id.to_string();
        let args = ["for-each-ref", "--format=%(refname)", "--contains", &id];
        let out = self.run(&args)?;
        Ok(out
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| String::from_utf8_lossy(l).into_owned())
            .collect())
    }
}

/// L'input di `update-ref -z --stdin` per `request`: una transazione esplicita
/// (`start` ... `commit`) con il valore vecchio di ogni ref.
fn transaction(request: &RefRestore) -> Vec<u8> {
    let mut input = Vec::new();
    let mut push = |fields: &[&str]| {
        for f in fields {
            input.extend_from_slice(f.as_bytes());
            input.push(0);
        }
    };
    push(&["start"]);
    if let (HeadState::Detached(expected), HeadState::Detached(target)) =
        (&request.head_expected, &request.head_target)
    {
        let (expected, target) = (expected.to_string(), target.to_string());
        push(&["option no-deref"]);
        push(&["update HEAD", &target, &expected]);
    }
    for RefMove {
        name,
        expected,
        target,
    } in &request.refs
    {
        let (expected, target) = (
            expected.map(|o| o.to_string()),
            target.map(|o| o.to_string()),
        );
        match (expected.as_deref(), target.as_deref()) {
            (Some(old), Some(new)) => push(&[&format!("update {name}"), new, old]),
            (None, Some(new)) => push(&[&format!("create {name}"), new]),
            (Some(old), None) => push(&[&format!("delete {name}"), old]),
            // Non deve esistere e resta così: solo il controllo.
            (None, None) => push(&[&format!("verify {name}"), ""]),
        }
    }
    push(&["commit"]);
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_input() {
        let a = Oid::from_bytes([0xaa; Oid::LEN]);
        let b = Oid::from_bytes([0xbb; Oid::LEN]);
        let request = RefRestore {
            command: "edit.undo",
            head_expected: HeadState::Detached(b),
            head_target: HeadState::Detached(a),
            refs: vec![
                RefMove {
                    name: "refs/heads/x".to_owned(),
                    expected: Some(b),
                    target: Some(a),
                },
                RefMove {
                    name: "refs/heads/nuovo".to_owned(),
                    expected: None,
                    target: Some(a),
                },
                RefMove {
                    name: "refs/heads/via".to_owned(),
                    expected: Some(b),
                    target: None,
                },
            ],
            reflog: "project: undo".to_owned(),
        };
        let text = String::from_utf8(transaction(&request))
            .unwrap()
            .replace('\0', "|");
        let (a, b) = (a.to_string(), b.to_string());
        assert_eq!(
            text,
            format!(
                "start|option no-deref|update HEAD|{a}|{b}|update refs/heads/x|{a}|{b}|\
                 create refs/heads/nuovo|{a}|delete refs/heads/via|{b}|commit|"
            )
        );
    }
}
