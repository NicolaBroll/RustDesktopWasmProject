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
//!    raggiungibile da un nuovo valore (`merge-base --is-ancestor`) né da un altro ref non
//!    simbolico (`for-each-ref --contains`, backup compresi; un ref simbolico verso il branch
//!    spostato non conta): un ref di backup
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
//! Limiti (HEAD non si verifica dentro la transazione):
//!
//! - Con HEAD su un branch, che HEAD punti ancora a quel branch si controlla al passo 2. Il
//!   valore dei branch invece è controllato sotto lock.
//! - Con HEAD staccata, `option no-deref` + `update HEAD <nuovo> <vecchio>` controlla solo il
//!   valore: se fra il passo 2 e la transazione un altro processo fa `git checkout main` con
//!   `main` sul commit atteso, la transazione riesce e stacca di nuovo HEAD (verificato con
//!   git 2.43; `main` non si sposta, nessun commit è perso).
//!
//! `symref-verify` (git ≥ 2.46) non si usa: si potrebbe solo per il caso del branch, ma una
//! verifica di HEAD nella stessa transazione che aggiorna il branch a cui HEAD punta urta il
//! controllo di git sugli aggiornamenti multipli di HEAD (git aggiunge da sé una voce di solo
//! reflog per HEAD), e la CI non ha un git così recente per provarlo. Al suo posto, dopo la
//! transazione si rilegge HEAD e si contano le voci nuove del reflog di HEAD (al massimo una,
//! la nostra): se HEAD non è quella chiesta o è cambiata nel frattempo, il risultato lo dice
//! in `warnings`. Senza reflog di HEAD resta solo il confronto con HEAD attuale.
//!
//! I controlli di raggiungibilità (`merge-base`, `for-each-ref --contains`) usano
//! `--no-replace-objects`: un replace object potrebbe far sembrare raggiungibile un commit che
//! per il gc non lo è.

use super::CliBackend;
use crate::command::{failure, path_from_output};
use project_domain::Oid;
use project_engine::{EngineError, HeadState, RefMove, RefRestore, RestoreOutcome};
use std::collections::BTreeSet;

impl CliBackend {
    /// [`GitBackend::restore_refs`](project_engine::GitBackend::restore_refs).
    pub(super) fn restore(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError> {
        request.check(self.operation())?;
        // Un branch aperto in un altro worktree non si cancella (es. redo di `branch.delete`).
        for r in &request.refs {
            if r.target.is_none()
                && r.name.starts_with("refs/heads/")
                && let Some(path) = self.checked_out_elsewhere(&r.name)?
            {
                return Err(EngineError::NotAllowed(format!(
                    "`{}` cancellerebbe `{}`, il branch attivo del worktree `{}`: niente \
                     modificato",
                    request.command,
                    r.name,
                    path.display()
                )));
            }
        }
        // Le voci del reflog di HEAD prima del controllo: dopo la transazione ce ne deve essere
        // al massimo una in più (la nostra), altrimenti HEAD è cambiata nel frattempo.
        let head_log_before = self.head_log_len()?;
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
        let warnings = self.check_head_after(request, head_log_before);
        Ok(RestoreOutcome { backups, warnings })
    }

    /// Le voci del reflog di HEAD: le righe del file `logs/HEAD` della worktree (il percorso
    /// lo dà `rev-parse --git-path logs/HEAD`). Non `rev-list --walk-reflogs --count HEAD`,
    /// che con HEAD su un branch senza commit dà 0 anche se il file ha righe. Con il formato
    /// reftable il file non esiste: 0, e resta solo il confronto con HEAD attuale.
    fn head_log_len(&self) -> Result<usize, EngineError> {
        let out = self.run(&["rev-parse", "--git-path", "logs/HEAD"])?;
        let path = self.workdir.join(path_from_output(&out));
        Ok(match std::fs::read(&path) {
            Ok(content) => content
                .split(|&b| b == b'\n')
                .filter(|l| !l.is_empty())
                .count(),
            Err(_) => 0,
        })
    }

    /// Dopo una transazione riuscita: HEAD deve essere quella chiesta, e nel reflog di HEAD ci
    /// deve essere al massimo una voce nuova. Altrimenti un altro processo ha cambiato HEAD
    /// proprio durante l'operazione: con HEAD staccata, ad esempio, un `git checkout main`
    /// fatto fra il controllo e la transazione (con `main` sul commit atteso) viene staccato di
    /// nuovo da `option no-deref` + `update HEAD` (`update-ref` non può verificare che HEAD
    /// non sia simbolica). Nessun commit è perso, ma va detto: un avviso per ogni problema.
    fn check_head_after(&self, request: &RefRestore, log_before: usize) -> Vec<String> {
        let mut warnings = Vec::new();
        match self.head_state() {
            Ok(head) if head == request.head_target => {}
            Ok(head) => warnings.push(format!(
                "dopo `{}` HEAD è {head} invece di {}: un altro processo l'ha cambiata \
                 durante l'operazione; controllare HEAD prima di continuare (nessun commit è \
                 perso)",
                request.command, request.head_target
            )),
            Err(e) => warnings.push(format!(
                "dopo `{}` HEAD non si rilegge ({e}): controllare il repository",
                request.command
            )),
        }
        if let Ok(after) = self.head_log_len()
            && after > log_before + 1
        {
            warnings.push(format!(
                "durante `{}` un altro processo ha cambiato HEAD ({} voci nuove nel reflog di \
                 HEAD invece di una): se era tornata su un branch (`git checkout`), ora è {}; \
                 rifare il checkout se serve (nessun commit è perso, vedi `git reflog HEAD`)",
                request.command,
                after - log_before,
                request.head_target
            ));
        }
        warnings
    }

    /// HEAD come la registra il diario: il branch a cui punta (nome completo, anche senza
    /// commit) o il commit se staccata.
    pub(super) fn head_state(&self) -> Result<HeadState, EngineError> {
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
    pub(super) fn ref_value(&self, name: &str) -> Result<Option<Oid>, EngineError> {
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
    ///
    /// Non contano come protezione i ref spostati dalla richiesta né i ref simbolici (es.
    /// `refs/heads/master -> refs/heads/main`): `for-each-ref` li mostra con il valore del
    /// ref a cui puntano *prima* della transazione, e dopo non terrebbero niente. Il ref a cui
    /// puntano compare comunque da solo, se non si sposta.
    fn protect(&self, request: &RefRestore) -> Result<Vec<String>, EngineError> {
        let targets = request.targets();
        let moved: BTreeSet<&str> = request.refs.iter().map(|r| r.name.as_str()).collect();
        let mut candidates = Vec::new();
        for oid in request.abandoned() {
            let mut kept = false;
            for target in &targets {
                if self.is_ancestor(&oid, target)? {
                    kept = true;
                    break;
                }
            }
            if !kept {
                candidates.push(oid);
            }
        }
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let holders = |oids: &[Oid]| -> Result<bool, EngineError> {
            Ok(self
                .refs_containing_any(oids)?
                .iter()
                .any(|name| !moved.contains(name.as_str())))
        };
        // Un solo `for-each-ref` per tutti: di solito nessun altro ref li contiene.
        let unprotected: Vec<Oid> = if !holders(&candidates)? {
            candidates
        } else if candidates.len() == 1 {
            Vec::new()
        } else {
            let mut out = Vec::new();
            for oid in candidates {
                if !holders(std::slice::from_ref(&oid))? {
                    out.push(oid);
                }
            }
            out
        };
        let mut backups = Vec::new();
        for oid in unprotected {
            let name = self.create_backup_ref(
                request.backup_kind(),
                &oid.to_string(),
                &request.backup_reflog(&oid),
            )?;
            backups.push(name);
        }
        Ok(backups)
    }

    /// Vero se il commit `a` è raggiungibile da `b` (o è `b`). Senza i replace object
    /// (`--no-replace-objects`): conta la storia vera, quella che vede il gc.
    fn is_ancestor(&self, a: &Oid, b: &Oid) -> Result<bool, EngineError> {
        let (a, b) = (a.to_string(), b.to_string());
        let args = [
            "--no-replace-objects",
            "merge-base",
            "--is-ancestor",
            "--end-of-options",
            &a,
            &b,
        ];
        let out = self.git.output(Some(&self.workdir), &args)?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) if out.stderr.is_empty() => Ok(false),
            _ => Err(failure(&args, &out)),
        }
    }

    /// I ref non simbolici (branch, remoti, tag, stash, backup, ...) da cui almeno uno dei
    /// commit `ids` è raggiungibile, nomi completi. Senza i replace object, come
    /// [`Self::is_ancestor`].
    fn refs_containing_any(&self, ids: &[Oid]) -> Result<Vec<String>, EngineError> {
        let ids: Vec<String> = ids.iter().map(Oid::to_string).collect();
        let mut args = vec![
            "--no-replace-objects",
            "for-each-ref",
            "--format=%(refname)%00%(symref)",
        ];
        for id in &ids {
            args.extend(["--contains", id]);
        }
        let out = self.run(&args)?;
        Ok(out
            .split(|&b| b == b'\n')
            .filter_map(|line| {
                let mut fields = line.split(|&b| b == 0);
                let name = fields.next().filter(|n| !n.is_empty())?;
                let symref = fields.next().unwrap_or_default();
                symref
                    .is_empty()
                    .then(|| String::from_utf8_lossy(name).into_owned())
            })
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
