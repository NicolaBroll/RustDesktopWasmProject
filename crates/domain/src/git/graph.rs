//! Corsie del grafo dei commit (`impl-docs/ARCHITETTURA-git.md` §3).
//!
//! Funzione pura: riceve i commit in ordine topologico, dal più recente (come `git log`), e
//! restituisce una riga per commit con la colonna del commit e i segmenti da disegnare fino
//! alla riga successiva.
//!
//! Algoritmo:
//! 1. si tiene un vettore di corsie attive; ogni corsia "aspetta" un [`Oid`];
//! 2. per ogni commit la colonna è la prima corsia che lo aspetta, oppure la prima corsia libera;
//!    le altre corsie che lo aspettavano convergono qui e si liberano;
//! 3. la corsia del commit passa ad aspettare il primo genitore; ogni altro genitore va sulla
//!    prima corsia che lo aspetta già, oppure sulla prima corsia libera (si apre una diramazione).
//!
//! Le corsie non si spostano: una corsia resta nella stessa colonna finché è aperta, quindi la
//! colonna può servire anche da indice del colore. Le corsie libere vengono riusate e quelle
//! libere in fondo si tolgono, così la larghezza resta limitata.
//!
//! Input non topologico o troncato (es. `git log -n 100`) non causa errori: un genitore che non
//! compare più avanti tiene aperta la sua corsia fino all'ultima riga, i cui segmenti restano
//! "sospesi" verso il basso.

use serde::{Deserialize, Serialize};

use super::{Commit, Oid};

/// Un tratto di arco genitore–figlio tra una riga e la successiva.
///
/// Parte dalla riga in cui si trova, nella colonna `from`, e arriva alla riga successiva nella
/// colonna `to`. Se `from` è la colonna del commit della riga, il tratto esce da quel commit;
/// se `to` è la colonna del commit della riga successiva, il tratto entra in quel commit;
/// altrimenti il tratto continua nella riga successiva da `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GraphSegment {
    /// Colonna di partenza, nella riga del segmento.
    pub from: usize,
    /// Colonna di arrivo, nella riga successiva.
    pub to: usize,
    /// Corsia su cui viaggia l'arco (utile per il colore): coincide con `to` per una
    /// diramazione, con `from` per una convergenza, con entrambe per un tratto verticale.
    pub lane: usize,
}

/// Una riga del grafo: un commit e i tratti verso la riga successiva.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphRow {
    /// Colonna del commit.
    pub column: usize,
    /// Tratti verso la riga successiva, ordinati per `(lane, from)`. Nell'ultima riga sono i
    /// tratti verso genitori che non compaiono nell'input.
    pub segments: Vec<GraphSegment>,
}

/// Disposizione del grafo: una riga per commit, nello stesso ordine dell'input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphLayout {
    pub rows: Vec<GraphRow>,
    /// Numero di colonne necessarie: `1 +` la colonna più grande usata da commit o tratti
    /// (`0` se non ci sono commit).
    pub width: usize,
}

/// Calcola le corsie per una lista di commit in ordine topologico, dal più recente.
pub fn layout_graph(commits: &[Commit]) -> GraphLayout {
    layout_graph_nodes(commits.iter().map(|c| (&c.id, c.parents.as_slice())))
}

/// Come [`layout_graph`], ma riceve solo identificatore e genitori di ogni commit.
pub fn layout_graph_nodes<'a, I>(nodes: I) -> GraphLayout
where
    I: IntoIterator<Item = (&'a Oid, &'a [Oid])>,
{
    // Corsie attive: `Some(oid)` aspetta quel commit, `None` è libera.
    let mut lanes: Vec<Option<Oid>> = Vec::new();
    // Tratti della riga precedente non ancora risolti: (from, lane). La colonna di arrivo
    // dipende da dove cade il commit successivo.
    let mut pending: Vec<(usize, usize)> = Vec::new();
    let mut rows: Vec<GraphRow> = Vec::new();

    for (id, parents) in nodes {
        // Passo 2: colonna del commit e corsie che convergono.
        let waiting: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, w)| w.as_ref() == Some(id))
            .map(|(j, _)| j)
            .collect();
        let column = match waiting.first() {
            Some(&j) => j,
            None => first_free(&mut lanes),
        };

        // Ora si conosce l'arrivo dei tratti della riga precedente.
        if let Some(prev) = rows.last_mut() {
            prev.segments = resolve(&pending, &waiting, column);
        }
        for &j in &waiting {
            lanes[j] = None;
        }

        // Passo 3: genitori, senza duplicati (non dovrebbero esserci, ma un arco va disegnato
        // una volta sola).
        pending.clear();
        // Tratti verticali delle corsie che passano accanto al commit: quelle già aperte prima
        // dei suoi genitori (la sua corsia si è appena liberata).
        for (j, w) in lanes.iter().enumerate() {
            if w.is_some() {
                pending.push((j, j));
            }
        }
        let mut seen: Vec<&Oid> = Vec::with_capacity(parents.len());
        for parent in parents {
            if seen.contains(&parent) {
                continue;
            }
            seen.push(parent);
            let lane = if seen.len() == 1 {
                column
            } else {
                match lanes.iter().position(|w| w.as_ref() == Some(parent)) {
                    Some(j) => j,
                    None => first_free(&mut lanes),
                }
            };
            lanes[lane] = Some(*parent);
            pending.push((column, lane));
        }

        // Le corsie libere in fondo non servono più.
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }

        rows.push(GraphRow {
            column,
            segments: Vec::new(),
        });
    }

    // Ultima riga: i tratti restano sospesi nella loro corsia.
    if let Some(last) = rows.last_mut() {
        last.segments = resolve(&pending, &[], usize::MAX);
    }

    let width = rows
        .iter()
        .flat_map(|r| {
            std::iter::once(r.column).chain(r.segments.iter().flat_map(|s| [s.from, s.to]))
        })
        .max()
        .map_or(0, |m| m + 1);

    GraphLayout { rows, width }
}

/// Prima corsia libera, aggiungendone una in fondo se servono.
fn first_free(lanes: &mut Vec<Option<Oid>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(j) => j,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

/// Trasforma i tratti in sospeso in segmenti: chi viaggia su una corsia che aspettava il
/// commit successivo converge sulla sua colonna, gli altri proseguono dritti.
fn resolve(pending: &[(usize, usize)], waiting: &[usize], column: usize) -> Vec<GraphSegment> {
    let mut segments: Vec<GraphSegment> = pending
        .iter()
        .map(|&(from, lane)| GraphSegment {
            from,
            to: if waiting.contains(&lane) {
                column
            } else {
                lane
            },
            lane,
        })
        .collect();
    segments.sort_by_key(|s| (s.lane, s.from));
    segments
}
