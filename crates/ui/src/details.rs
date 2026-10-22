//! Stato del pannello dettagli: quale commit chiedere a `commit.show` e quale risposta
//! mostrare. Solo dati, senza egui: si prova con test unitari.
//!
//! Regole:
//! - si chiede un commit solo quando cambia la selezione, non a ogni frame;
//! - al massimo una richiesta in corso: se la selezione cambia mentre aspetta (frecce tenute
//!   premute), alla risposta si chiede solo l'ultimo commit selezionato, non tutti quelli
//!   attraversati;
//! - una risposta per un commit che non è più quello selezionato si scarta.

use project_domain::{CommitDetails, Oid};

/// Cosa sa il pannello del commit selezionato.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown<'a> {
    /// Nessun commit selezionato.
    Nothing,
    /// Richiesta in corso (o in attesa che finisca quella di un commit precedente).
    Loading(Oid),
    Loaded(&'a CommitDetails),
    Failed(Oid, &'a str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    Loaded(Box<CommitDetails>),
    Failed(String),
}

#[derive(Debug, Default)]
pub struct DetailsState {
    /// Il commit selezionato nella lista.
    target: Option<Oid>,
    /// Il commit chiesto e non ancora arrivato.
    in_flight: Option<Oid>,
    /// L'ultima risposta arrivata per `target`, con il suo id.
    answer: Option<(Oid, Answer)>,
}

impl DetailsState {
    /// Annota il commit selezionato e restituisce l'id da chiedere a `commit.show`, se serve:
    /// la risposta non c'è già e non c'è un'altra richiesta in corso. Si chiama a ogni frame.
    pub fn want(&mut self, selected: Option<Oid>) -> Option<Oid> {
        self.target = selected;
        let id = selected?;
        let answered = self.answer.as_ref().is_some_and(|(a, _)| *a == id);
        if answered || self.in_flight.is_some() {
            return None;
        }
        self.in_flight = Some(id);
        Some(id)
    }

    /// La risposta di `commit.show` per `id`. Si tiene solo se `id` è ancora il commit
    /// selezionato: restituisce `false` se è stata scartata.
    pub fn receive(&mut self, id: Oid, result: Result<CommitDetails, String>) -> bool {
        if self.in_flight == Some(id) {
            self.in_flight = None;
        }
        if self.target != Some(id) {
            return false;
        }
        let answer = match result {
            Ok(details) => Answer::Loaded(Box::new(details)),
            Err(e) => Answer::Failed(e),
        };
        self.answer = Some((id, answer));
        true
    }

    /// Dimentica l'errore del commit selezionato: al prossimo [`Self::want`] si richiede.
    pub fn retry(&mut self) {
        if matches!(self.answer, Some((_, Answer::Failed(_)))) {
            self.answer = None;
        }
    }

    /// Cosa mostrare per il commit selezionato.
    pub fn shown(&self) -> Shown<'_> {
        let Some(id) = self.target else {
            return Shown::Nothing;
        };
        match &self.answer {
            Some((a, Answer::Loaded(details))) if *a == id => Shown::Loaded(details),
            Some((a, Answer::Failed(e))) if *a == id => Shown::Failed(id, e),
            _ => Shown::Loading(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Commit, Signature};

    fn oid(n: u8) -> Oid {
        Oid::from_bytes([n; 20])
    }

    fn details(n: u8) -> CommitDetails {
        let sig = Signature {
            name: "Ada".to_owned(),
            email: "ada@example.com".to_owned(),
            time: 0,
            offset_min: 0,
        };
        CommitDetails {
            commit: Commit {
                id: oid(n),
                parents: Vec::new(),
                author: sig.clone(),
                committer: sig,
                summary: format!("commit {n}"),
                body: String::new(),
            },
            compared_to: None,
            files: Vec::new(),
        }
    }

    #[test]
    fn asks_once_per_selection() {
        let mut s = DetailsState::default();
        assert_eq!(s.want(None), None);
        assert_eq!(s.shown(), Shown::Nothing);
        assert_eq!(s.want(Some(oid(1))), Some(oid(1)));
        // I frame seguenti non richiedono di nuovo.
        assert_eq!(s.want(Some(oid(1))), None);
        assert_eq!(s.shown(), Shown::Loading(oid(1)));
        assert!(s.receive(oid(1), Ok(details(1))));
        assert_eq!(s.want(Some(oid(1))), None);
        assert_eq!(s.shown(), Shown::Loaded(&details(1)));
    }

    #[test]
    fn a_late_answer_for_an_old_selection_is_dropped() {
        let mut s = DetailsState::default();
        assert_eq!(s.want(Some(oid(1))), Some(oid(1)));
        // La selezione passa a 2 e poi a 3 mentre 1 è in corso: nessuna nuova richiesta.
        assert_eq!(s.want(Some(oid(2))), None);
        assert_eq!(s.want(Some(oid(3))), None);
        assert!(!s.receive(oid(1), Ok(details(1))));
        assert_eq!(s.shown(), Shown::Loading(oid(3)));
        // Finita la richiesta di 1 si chiede solo l'ultimo selezionato.
        assert_eq!(s.want(Some(oid(3))), Some(oid(3)));
        assert!(s.receive(oid(3), Ok(details(3))));
        assert_eq!(s.shown(), Shown::Loaded(&details(3)));
    }

    #[test]
    fn going_back_to_the_loaded_commit_needs_no_request() {
        let mut s = DetailsState::default();
        s.want(Some(oid(1)));
        s.receive(oid(1), Ok(details(1)));
        assert_eq!(s.want(Some(oid(2))), Some(oid(2)));
        // Torna a 1 prima che arrivi 2: 1 è già lì.
        assert_eq!(s.want(Some(oid(1))), None);
        assert_eq!(s.shown(), Shown::Loaded(&details(1)));
        assert!(!s.receive(oid(2), Ok(details(2))));
        assert_eq!(s.want(Some(oid(1))), None);
    }

    #[test]
    fn errors_are_shown_and_can_be_retried() {
        let mut s = DetailsState::default();
        s.want(Some(oid(1)));
        assert!(s.receive(oid(1), Err("commit sconosciuto".to_owned())));
        assert_eq!(s.shown(), Shown::Failed(oid(1), "commit sconosciuto"));
        // Niente richieste ripetute a ogni frame dopo un errore.
        assert_eq!(s.want(Some(oid(1))), None);
        s.retry();
        assert_eq!(s.shown(), Shown::Loading(oid(1)));
        assert_eq!(s.want(Some(oid(1))), Some(oid(1)));
    }

    #[test]
    fn deselecting_drops_the_answer_in_flight() {
        let mut s = DetailsState::default();
        s.want(Some(oid(1)));
        s.want(None);
        assert!(!s.receive(oid(1), Ok(details(1))));
        assert_eq!(s.shown(), Shown::Nothing);
    }
}
