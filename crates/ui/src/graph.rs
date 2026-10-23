//! Disegno del grafo dei commit: colori delle corsie, geometria degli archi ed etichette dei
//! ref. Le corsie le calcola `project_domain::layout_graph`; qui c'è solo come mostrarle, in
//! funzioni pure che si provano senza aprire una finestra.

use egui::{Color32, Pos2, pos2};
use project_domain::{GraphSegment, Head, Oid, RefInfo, RefKind};
use std::collections::HashMap;

/// Larghezza di una corsia, in punti.
pub const LANE_WIDTH: f32 = 14.0;
/// Raggio del pallino di un commit.
pub const NODE_RADIUS: f32 = 4.5;
/// Spessore degli archi.
pub const EDGE_WIDTH: f32 = 2.0;

/// Colori delle corsie: abbastanza saturi da leggersi su sfondo chiaro e scuro, vicini tra
/// loro diversi. Si ripetono dopo l'ultimo.
const PALETTE: [Color32; 8] = [
    Color32::from_rgb(0x3d, 0x9e, 0xf5), // blu
    Color32::from_rgb(0xf5, 0x8b, 0x2d), // arancio
    Color32::from_rgb(0x3c, 0xba, 0x6a), // verde
    Color32::from_rgb(0xe0, 0x4f, 0x8f), // rosa
    Color32::from_rgb(0x9b, 0x6b, 0xdf), // viola
    Color32::from_rgb(0xd9, 0xb8, 0x2b), // giallo
    Color32::from_rgb(0x2b, 0xb8, 0xb8), // turchese
    Color32::from_rgb(0xe0, 0x55, 0x45), // rosso
];

/// Il colore della corsia `lane` (stabile finché la corsia resta aperta).
pub fn lane_color(lane: usize) -> Color32 {
    PALETTE[lane % PALETTE.len()]
}

/// Larghezza della colonna del grafo per `lanes` corsie, con un margine per lato.
pub fn graph_width(lanes: usize) -> f32 {
    lanes.max(1) as f32 * LANE_WIDTH + LANE_WIDTH / 2.0
}

/// Ascissa del centro della colonna `column`, se la colonna del grafo comincia a `left`.
pub fn column_x(left: f32, column: usize) -> f32 {
    left + LANE_WIDTH / 4.0 + (column as f32 + 0.5) * LANE_WIDTH
}

/// Un tratto da disegnare.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edge {
    /// Tratto dritto (verticale) tra due punti.
    Line([Pos2; 2]),
    /// Curva di Bézier cubica: sposta l'arco da una colonna all'altra.
    Curve([Pos2; 4]),
}

/// Il tratto di `segment`: dal centro della sua riga (ordinata `y`) al centro della riga
/// seguente (`y + step`). Se cambia colonna è una curva a S che parte e arriva in verticale.
pub fn segment_edge(segment: &GraphSegment, left: f32, y: f32, step: f32) -> Edge {
    let start = pos2(column_x(left, segment.from), y);
    let end = pos2(column_x(left, segment.to), y + step);
    if segment.from == segment.to {
        Edge::Line([start, end])
    } else {
        let mid = y + step / 2.0;
        Edge::Curve([start, pos2(start.x, mid), pos2(end.x, mid), end])
    }
}

/// Tipo di etichetta, nell'ordine in cui si mostrano sulla riga.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LabelKind {
    /// HEAD staccata su questo commit.
    Head,
    /// Il branch su cui è HEAD.
    CurrentBranch,
    Branch,
    Remote,
    Tag,
    /// Uno stash, sul commit su cui è stato creato (la sua base).
    Stash,
}

/// Un'etichetta da mostrare sulla riga di un commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub kind: LabelKind,
    pub text: String,
}

/// Etichette per commit: branch locali (quello corrente per primo), remoti, tag, e `HEAD` se
/// è staccata. `refs/stash` non ha etichetta: gli stash si aggiungono con [`add_stash_labels`],
/// sulla loro base.
pub fn ref_labels(refs: &[RefInfo], head: &Head) -> HashMap<Oid, Vec<RefLabel>> {
    let mut labels: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
    let current = match head {
        Head::Branch { name, .. } => Some(name.as_str()),
        Head::Detached(_) => None,
    };
    let mut current_found = false;
    for info in refs {
        let text = info.short_name().to_owned();
        let kind = match info.kind {
            RefKind::LocalBranch if current == Some(text.as_str()) => {
                current_found = true;
                LabelKind::CurrentBranch
            }
            RefKind::LocalBranch => LabelKind::Branch,
            RefKind::RemoteBranch => LabelKind::Remote,
            RefKind::Tag => LabelKind::Tag,
            RefKind::Stash => continue,
        };
        labels
            .entry(info.target)
            .or_default()
            .push(RefLabel { kind, text });
    }
    // HEAD arriva anche prima dei ref (da `repo.open`): il branch corrente si mostra subito.
    match head {
        Head::Detached(id) => labels.entry(*id).or_default().push(RefLabel {
            kind: LabelKind::Head,
            text: "HEAD".to_owned(),
        }),
        Head::Branch {
            name,
            target: Some(id),
        } if !current_found => labels.entry(*id).or_default().push(RefLabel {
            kind: LabelKind::CurrentBranch,
            text: name.clone(),
        }),
        Head::Branch { .. } => {}
    }
    for list in labels.values_mut() {
        list.sort_by(|a, b| (a.kind, &a.text).cmp(&(b.kind, &b.text)));
    }
    labels
}

/// Aggiunge in fondo alle etichette di ogni commit gli stash creati lì sopra: `(base, nome)`,
/// dal più recente (come `stash.list`). Più stash sulla stessa base restano nell'ordine
/// dell'elenco.
pub fn add_stash_labels<'a>(
    labels: &mut HashMap<Oid, Vec<RefLabel>>,
    stashes: impl IntoIterator<Item = (Oid, &'a str)>,
) {
    for (base, name) in stashes {
        labels.entry(base).or_default().push(RefLabel {
            kind: LabelKind::Stash,
            text: name.to_owned(),
        });
    }
}

/// Quante etichette larghe `widths` (spazio tra l'una e l'altra compreso) stanno in
/// `available`. Se non ci stanno tutte serve anche l'indicatore `+N`, largo `more`: le
/// etichette mostrate lasciano posto anche a quello.
pub fn labels_that_fit(widths: &[f32], available: f32, more: f32) -> usize {
    let total: f32 = widths.iter().sum();
    if total <= available {
        return widths.len();
    }
    let mut used = more;
    widths
        .iter()
        .take_while(|&&w| {
            used += w;
            used <= available
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::Upstream;

    fn oid(n: u8) -> Oid {
        Oid::from_bytes([n; 20])
    }

    fn info(name: &str, kind: RefKind, target: u8) -> RefInfo {
        RefInfo {
            name: name.to_owned(),
            kind,
            target: oid(target),
            tag_object: None,
            upstream: None::<Upstream>,
        }
    }

    #[test]
    fn adjacent_lanes_have_different_colors() {
        for lane in 0..PALETTE.len() * 2 {
            assert_ne!(lane_color(lane), lane_color(lane + 1), "corsia {lane}");
        }
        assert_eq!(lane_color(1), lane_color(1 + PALETTE.len()));
    }

    #[test]
    fn columns_are_inside_the_graph() {
        let left = 100.0;
        for lanes in 1..6 {
            let right = left + graph_width(lanes);
            let first = column_x(left, 0);
            let last = column_x(left, lanes - 1);
            assert!(first - NODE_RADIUS > left, "{lanes} corsie");
            assert!(last + NODE_RADIUS < right, "{lanes} corsie");
            assert_eq!(column_x(left, 1) - first, LANE_WIDTH);
        }
        assert_eq!(graph_width(0), graph_width(1));
    }

    #[test]
    fn straight_and_curved_edges() {
        let vertical = GraphSegment {
            from: 1,
            to: 1,
            lane: 1,
        };
        let x1 = column_x(0.0, 1);
        assert_eq!(
            segment_edge(&vertical, 0.0, 10.0, 20.0),
            Edge::Line([pos2(x1, 10.0), pos2(x1, 30.0)])
        );

        // Diramazione dalla colonna 0 alla 2: parte e arriva in verticale, curva a metà.
        let branch = GraphSegment {
            from: 0,
            to: 2,
            lane: 2,
        };
        let Edge::Curve([a, b, c, d]) = segment_edge(&branch, 0.0, 10.0, 20.0) else {
            panic!("attesa una curva");
        };
        let (x0, x2) = (column_x(0.0, 0), column_x(0.0, 2));
        assert_eq!((a, d), (pos2(x0, 10.0), pos2(x2, 30.0)));
        assert_eq!((b.x, c.x), (x0, x2));
        assert_eq!((b.y, c.y), (20.0, 20.0));
    }

    #[test]
    fn labels_per_commit_in_order() {
        let refs = vec![
            info("refs/tags/v1", RefKind::Tag, 1),
            info("refs/remotes/origin/main", RefKind::RemoteBranch, 1),
            info("refs/heads/main", RefKind::LocalBranch, 1),
            info("refs/heads/alfa", RefKind::LocalBranch, 1),
            info("refs/heads/feature", RefKind::LocalBranch, 2),
            info("refs/stash", RefKind::Stash, 3),
        ];
        let head = Head::Branch {
            name: "main".to_owned(),
            target: Some(oid(1)),
        };
        let labels = ref_labels(&refs, &head);
        let texts = |n: u8| -> Vec<(LabelKind, &str)> {
            labels[&oid(n)]
                .iter()
                .map(|l| (l.kind, l.text.as_str()))
                .collect()
        };
        assert_eq!(
            texts(1),
            [
                (LabelKind::CurrentBranch, "main"),
                (LabelKind::Branch, "alfa"),
                (LabelKind::Remote, "origin/main"),
                (LabelKind::Tag, "v1"),
            ]
        );
        assert_eq!(texts(2), [(LabelKind::Branch, "feature")]);
        assert!(
            !labels.contains_key(&oid(3)),
            "niente etichetta per refs/stash"
        );
    }

    #[test]
    fn stash_labels_go_last_on_their_base() {
        let refs = vec![info("refs/heads/main", RefKind::LocalBranch, 1)];
        let head = Head::Branch {
            name: "main".to_owned(),
            target: Some(oid(1)),
        };
        let mut labels = ref_labels(&refs, &head);
        add_stash_labels(
            &mut labels,
            [
                (oid(1), "stash@{0}"),
                (oid(2), "stash@{1}"),
                (oid(1), "stash@{2}"),
            ],
        );
        let texts = |n: u8| -> Vec<(LabelKind, &str)> {
            labels[&oid(n)]
                .iter()
                .map(|l| (l.kind, l.text.as_str()))
                .collect()
        };
        assert_eq!(
            texts(1),
            [
                (LabelKind::CurrentBranch, "main"),
                (LabelKind::Stash, "stash@{0}"),
                (LabelKind::Stash, "stash@{2}"),
            ]
        );
        assert_eq!(texts(2), [(LabelKind::Stash, "stash@{1}")]);
    }

    #[test]
    fn head_labels_without_refs() {
        let labels = ref_labels(&[], &Head::Detached(oid(4)));
        assert_eq!(labels[&oid(4)][0].kind, LabelKind::Head);

        let head = Head::Branch {
            name: "main".to_owned(),
            target: Some(oid(5)),
        };
        let labels = ref_labels(&[], &head);
        assert_eq!(labels[&oid(5)][0].text, "main");

        let unborn = Head::Branch {
            name: "main".to_owned(),
            target: None,
        };
        assert!(ref_labels(&[], &unborn).is_empty());
    }

    #[test]
    fn labels_leave_room_for_the_counter() {
        assert_eq!(labels_that_fit(&[30.0, 30.0], 60.0, 20.0), 2);
        assert_eq!(labels_that_fit(&[30.0, 30.0, 30.0], 80.0, 20.0), 2);
        assert_eq!(labels_that_fit(&[30.0, 30.0, 30.0], 70.0, 20.0), 1);
        assert_eq!(labels_that_fit(&[50.0], 10.0, 20.0), 0);
        assert_eq!(labels_that_fit(&[], 10.0, 20.0), 0);
    }
}
