//! Stato della lista dei commit: pagine di `repo.log` caricate durante lo scorrimento e
//! selezione da mouse e tastiera, corsie del grafo. Solo dati, senza egui: si prova con test
//! unitari.

use project_domain::{Commit, GraphLayout, Oid, layout_graph};

/// Commit chiesti a ogni pagina di `repo.log`.
pub const PAGE: usize = 500;

/// Quante righe prima della fine dei commit caricati si chiede la pagina seguente.
const PREFETCH: usize = PAGE / 2;

/// Una richiesta di pagina: `skip` e `limit` di `repo.log`, più l'epoca della lista a cui
/// appartiene (vedi [`CommitList::reset`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    pub skip: usize,
    pub limit: usize,
    pub epoch: u64,
}

/// I commit caricati finora e il commit selezionato.
#[derive(Debug, Default)]
pub struct CommitList {
    commits: Vec<Commit>,
    /// Corsie del grafo di tutti i commit caricati, una riga per commit. Si ricalcolano quando
    /// arriva una pagina, non a ogni frame: una corsia può dipendere da commit di pagine
    /// diverse.
    graph: GraphLayout,
    has_more: bool,
    /// La pagina chiesta e non ancora arrivata.
    loading: Option<PageRequest>,
    /// Cresce a ogni [`CommitList::reset`]: le pagine chieste prima si scartano.
    epoch: u64,
    selected: Option<usize>,
    /// Riga da portare in vista al prossimo frame (dopo un movimento da tastiera).
    scroll_to: Option<usize>,
}

impl CommitList {
    pub fn commits(&self) -> &[Commit] {
        &self.commits
    }

    /// Le corsie del grafo: `graph().rows[i]` è la riga di `commits()[i]`.
    pub fn graph(&self) -> &GraphLayout {
        &self.graph
    }

    /// Vero se dopo l'ultimo commit caricato ce ne sono altri.
    pub fn has_more(&self) -> bool {
        self.has_more
    }

    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected_commit(&self) -> Option<&Commit> {
        self.commits.get(self.selected?)
    }

    /// Ricomincia dalla prima pagina (repo aperto o aggiornato) e la restituisce da chiedere.
    /// I commit già caricati restano visibili finché non arriva la nuova prima pagina.
    pub fn reset(&mut self) -> PageRequest {
        self.epoch += 1;
        let request = PageRequest {
            skip: 0,
            limit: PAGE,
            epoch: self.epoch,
        };
        self.loading = Some(request);
        request
    }

    /// La pagina seguente da chiedere se la riga `visible_end` (esclusa) è vicina alla fine dei
    /// commit caricati; `None` se non serve o se una pagina è già in arrivo.
    pub fn next_page(&mut self, visible_end: usize) -> Option<PageRequest> {
        if !self.has_more || self.loading.is_some() || visible_end + PREFETCH < self.commits.len() {
            return None;
        }
        let request = PageRequest {
            skip: self.commits.len(),
            limit: PAGE,
            epoch: self.epoch,
        };
        self.loading = Some(request);
        Some(request)
    }

    /// Una pagina arrivata. Se è la prima sostituisce la lista e mantiene selezionato lo
    /// stesso commit (o il primo); le altre si aggiungono in fondo. Le pagine di un'epoca
    /// passata o che non attaccano alla fine della lista si scartano: restituisce `false`.
    pub fn receive(&mut self, request: PageRequest, commits: Vec<Commit>, has_more: bool) -> bool {
        if self.loading != Some(request) {
            return false;
        }
        self.loading = None;
        if request.skip == 0 {
            let selected = self.selected_commit().map(|c| c.id);
            self.commits = commits;
            self.selected = selected
                .and_then(|id| self.position(id))
                .or((!self.commits.is_empty()).then_some(0));
        } else {
            self.commits.extend(commits);
        }
        self.graph = layout_graph(&self.commits);
        self.has_more = has_more;
        true
    }

    /// La pagina chiesta non è arrivata (errore): si può chiedere di nuovo.
    pub fn failed(&mut self, request: PageRequest) {
        if self.loading == Some(request) {
            self.loading = None;
        }
    }

    fn position(&self, id: Oid) -> Option<usize> {
        self.commits.iter().position(|c| c.id == id)
    }

    /// Seleziona la riga `index` (clic del mouse); fuori dalla lista non fa niente.
    pub fn select(&mut self, index: usize) {
        if index < self.commits.len() {
            self.selected = Some(index);
        }
    }

    /// Seleziona il commit `id` e lo porta in vista; `false` se non è tra quelli caricati.
    pub fn select_id(&mut self, id: Oid) -> bool {
        match self.position(id) {
            Some(index) => {
                self.selected = Some(index);
                self.scroll_to = Some(index);
                true
            }
            None => false,
        }
    }

    /// Sposta la selezione di `delta` righe (negativo verso l'alto), restando nella lista, e
    /// la porta in vista. Senza selezione parte dalla prima riga.
    pub fn move_selection(&mut self, delta: isize) {
        let Some(last) = self.commits.len().checked_sub(1) else {
            return;
        };
        let index = match self.selected {
            Some(current) => current.saturating_add_signed(delta).min(last),
            None => 0,
        };
        self.selected = Some(index);
        self.scroll_to = Some(index);
    }

    /// Seleziona la prima riga (`true`) o l'ultima caricata (`false`).
    pub fn select_edge(&mut self, first: bool) {
        if let Some(last) = self.commits.len().checked_sub(1) {
            let index = if first { 0 } else { last };
            self.selected = Some(index);
            self.scroll_to = Some(index);
        }
    }

    /// La riga da portare in vista, senza consumarla.
    pub fn peek_scroll_to(&self) -> Option<usize> {
        self.scroll_to
    }

    /// La riga da portare in vista, una volta sola: dopo averla disegnata.
    pub fn take_scroll_to(&mut self) -> Option<usize> {
        self.scroll_to.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::Signature;

    fn commit(n: usize) -> Commit {
        let mut bytes = [0u8; 20];
        bytes[..8].copy_from_slice(&(n as u64).to_be_bytes());
        let sig = Signature {
            name: "Ada".to_owned(),
            email: "ada@example.com".to_owned(),
            time: 0,
            offset_min: 0,
        };
        Commit {
            id: Oid::from_bytes(bytes),
            parents: Vec::new(),
            author: sig.clone(),
            committer: sig,
            summary: format!("commit {n}"),
            body: String::new(),
        }
    }

    /// Una pagina di una storia di `total` commit, come la darebbe `repo.log`.
    fn page(request: PageRequest, total: usize) -> (Vec<Commit>, bool) {
        let end = (request.skip + request.limit).min(total);
        let commits = (request.skip.min(end)..end).map(commit).collect();
        (commits, end < total)
    }

    fn load(list: &mut CommitList, request: PageRequest, total: usize) {
        let (commits, more) = page(request, total);
        assert!(list.receive(request, commits, more));
    }

    #[test]
    fn scrolling_loads_every_page_once() {
        let total = 50_000;
        let mut list = CommitList::default();
        let first = list.reset();
        assert_eq!(list.next_page(0), None, "la prima pagina è già in arrivo");
        load(&mut list, first, total);
        assert_eq!(list.selected(), Some(0));

        // Righe visibili lontane dalla fine: niente pagine.
        assert_eq!(list.next_page(10), None);

        let mut requests = 1;
        let mut visible_end = 0;
        while list.has_more() {
            visible_end += 40;
            if let Some(request) = list.next_page(visible_end) {
                assert_eq!(request.skip, list.commits().len());
                assert_eq!(list.next_page(visible_end), None, "una pagina alla volta");
                load(&mut list, request, total);
                requests += 1;
            }
        }
        assert_eq!(list.commits().len(), total);
        assert_eq!(requests, total.div_ceil(PAGE));
        let ids: Vec<Oid> = list.commits().iter().map(|c| c.id).collect();
        assert_eq!(ids, (0..total).map(|n| commit(n).id).collect::<Vec<_>>());
        assert_eq!(list.next_page(total), None);
    }

    #[test]
    fn graph_spans_every_loaded_page() {
        // Una storia lineare con un branch che si apre nella prima pagina e si chiude con un
        // merge nella seconda: la corsia del branch deve attraversare il confine tra le pagine.
        let total = PAGE + 100;
        let merge = PAGE + 10;
        let mut history: Vec<Commit> = (0..total)
            .map(|n| {
                let mut c = commit(n);
                // Commit n ha come genitore n + 1 (la lista va dal più recente).
                if n + 1 < total {
                    c.parents.push(commit(n + 1).id);
                }
                if n == 5 {
                    // Il merge in cima porta dentro il branch che parte da `merge`.
                    c.parents.push(commit(total + 1).id);
                }
                c
            })
            .collect();
        let mut side = commit(total + 1);
        side.parents.push(commit(merge).id);
        history.insert(6, side);

        let mut list = CommitList::default();
        let first = list.reset();
        assert!(list.receive(first, history[..PAGE].to_vec(), true));
        assert_eq!(list.graph().rows.len(), PAGE);
        let second = list.next_page(PAGE).unwrap();
        assert!(list.receive(second, history[PAGE..].to_vec(), false));

        assert_eq!(list.graph(), &layout_graph(&history));
        assert_eq!(list.graph().rows.len(), list.commits().len());
        assert_eq!(list.graph().width, 2);
        // Il branch converge nella corsia 0 sul commit `merge` (riga `merge + 1`, spostata dal
        // commit del branch), nella seconda pagina: il tratto sta nella riga prima.
        let before = &list.graph().rows[merge];
        assert!(before.segments.iter().any(|s| s.from == 1 && s.to == 0));
    }

    #[test]
    fn pages_of_an_old_epoch_are_dropped() {
        let mut list = CommitList::default();
        let first = list.reset();
        load(&mut list, first, 2_000);
        let second = list.next_page(PAGE).unwrap();
        // Aggiorna mentre la seconda pagina è in arrivo.
        let fresh = list.reset();
        let (commits, more) = page(second, 2_000);
        assert!(!list.receive(second, commits, more));
        load(&mut list, fresh, 2_000);
        assert_eq!(list.commits().len(), PAGE);
    }

    #[test]
    fn refresh_keeps_the_selected_commit() {
        let mut list = CommitList::default();
        let first = list.reset();
        load(&mut list, first, 100);
        list.select(5);
        let id = list.selected_commit().unwrap().id;

        // Il nuovo log ha due commit in più in cima: la selezione segue il commit.
        let request = list.reset();
        let mut commits: Vec<Commit> = (0..100).map(commit).collect();
        commits.insert(0, commit(1000));
        commits.insert(0, commit(1001));
        assert!(list.receive(request, commits, false));
        assert_eq!(list.selected(), Some(7));
        assert_eq!(list.selected_commit().unwrap().id, id);
    }

    #[test]
    fn a_failed_page_can_be_asked_again() {
        let mut list = CommitList::default();
        let first = list.reset();
        load(&mut list, first, 1_000);
        let request = list.next_page(PAGE).unwrap();
        list.failed(request);
        assert_eq!(list.next_page(PAGE), Some(request));
    }

    #[test]
    fn keyboard_moves_stay_inside_the_list() {
        let mut list = CommitList::default();
        list.move_selection(1);
        assert_eq!(list.selected(), None, "lista vuota");

        let first = list.reset();
        load(&mut list, first, 30);
        list.move_selection(3);
        assert_eq!(list.selected(), Some(3));
        assert_eq!(list.take_scroll_to(), Some(3));
        assert_eq!(list.take_scroll_to(), None);
        list.move_selection(-10);
        assert_eq!(list.selected(), Some(0));
        list.move_selection(100);
        assert_eq!(list.selected(), Some(29));
        list.select_edge(true);
        assert_eq!(list.selected(), Some(0));
        list.select_edge(false);
        assert_eq!(list.selected(), Some(29));

        list.select(500);
        assert_eq!(list.selected(), Some(29), "clic fuori dalla lista");
        assert!(list.select_id(commit(4).id));
        assert_eq!((list.selected(), list.take_scroll_to()), (Some(4), Some(4)));
        assert!(!list.select_id(commit(999).id));
    }
}
