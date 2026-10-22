//! Diff di righe per [`super::MemoryBackend`]: gli hunk che scriverebbe `git diff` con le
//! impostazioni predefinite (3 righe di contesto, hunk vicini uniti).
//!
//! Usa la sottosequenza comune più lunga, in tempo e memoria `O(n·m)`: basta per i file piccoli
//! di un repo in memoria, non per file veri.

use project_domain::{ByteText, Hunk, Line, LineKind};

/// Righe di contesto intorno a ogni gruppo di modifiche, come il default di git.
const CONTEXT: usize = 3;

/// Una riga del file: il testo senza a capo, e se l'a capo c'era. Due righe sono uguali solo se
/// coincidono entrambi, come per git (`x` in fondo al file senza a capo è diversa da `x\n`).
#[derive(PartialEq, Eq)]
struct Source<'a> {
    text: &'a [u8],
    newline: bool,
}

fn split(content: &[u8]) -> Vec<Source<'_>> {
    let mut lines = Vec::new();
    let mut rest = content;
    while !rest.is_empty() {
        match rest.iter().position(|&b| b == b'\n') {
            Some(i) => {
                lines.push(Source {
                    text: &rest[..i],
                    newline: true,
                });
                rest = &rest[i + 1..];
            }
            None => {
                lines.push(Source {
                    text: rest,
                    newline: false,
                });
                rest = &[];
            }
        }
    }
    lines
}

#[derive(Clone, Copy)]
enum Op {
    /// Riga uguale: indice nel vecchio e nel nuovo.
    Equal(usize, usize),
    /// Riga tolta: indice nel vecchio.
    Delete(usize),
    /// Riga aggiunta: indice nel nuovo.
    Insert(usize),
}

/// Le operazioni che portano da `a` a `b`; in un blocco cambiato le righe tolte vengono prima
/// di quelle aggiunte, come in git.
fn ops(a: &[Source], b: &[Source]) -> Vec<Op> {
    let (n, m) = (a.len(), b.len());
    // lcs[i][j]: lunghezza della sottosequenza comune più lunga di a[i..] e b[j..].
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(Op::Equal(i, j));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(Op::Delete(i));
            i += 1;
        } else {
            out.push(Op::Insert(j));
            j += 1;
        }
    }
    out.extend((i..n).map(Op::Delete));
    out.extend((j..m).map(Op::Insert));
    out
}

fn number(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// Gli hunk del diff da `old` a `new` (contenuti interi dei due file). Vuoto se sono uguali.
pub(super) fn hunks(old: &[u8], new: &[u8]) -> Vec<Hunk> {
    let (a, b) = (split(old), split(new));
    let ops = ops(&a, &b);

    // Gruppi di modifiche (indici in `ops`, estremi compresi): due modifiche separate da al
    // massimo 2 × CONTEXT righe uguali finiscono nello stesso hunk.
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        if matches!(op, Op::Equal(..)) {
            continue;
        }
        match groups.last_mut() {
            Some((_, end)) if i - *end <= 2 * CONTEXT + 1 => *end = i,
            _ => groups.push((i, i)),
        }
    }

    // Righe del vecchio e del nuovo prima di ogni operazione.
    let mut before = Vec::with_capacity(ops.len() + 1);
    let (mut old_seen, mut new_seen) = (0, 0);
    for op in &ops {
        before.push((old_seen, new_seen));
        match op {
            Op::Equal(..) => {
                old_seen += 1;
                new_seen += 1;
            }
            Op::Delete(_) => old_seen += 1,
            Op::Insert(_) => new_seen += 1,
        }
    }

    groups
        .into_iter()
        .map(|(start, end)| {
            let from = start.saturating_sub(CONTEXT);
            let to = (end + CONTEXT).min(ops.len() - 1);
            let (old_before, new_before) = before[from];
            let (mut old_count, mut new_count) = (0, 0);
            let lines: Vec<Line> = ops[from..=to]
                .iter()
                .map(|op| {
                    let (kind, source, old_line, new_line) = match *op {
                        Op::Equal(i, j) => (
                            LineKind::Context,
                            &a[i],
                            Some(number(i + 1)),
                            Some(number(j + 1)),
                        ),
                        Op::Delete(i) => (LineKind::Removed, &a[i], Some(number(i + 1)), None),
                        Op::Insert(j) => (LineKind::Added, &b[j], None, Some(number(j + 1))),
                    };
                    old_count += usize::from(old_line.is_some());
                    new_count += usize::from(new_line.is_some());
                    Line {
                        kind,
                        old_line,
                        new_line,
                        content: ByteText::from_bytes(source.text),
                        no_newline: !source.newline,
                    }
                })
                .collect();
            // Con zero righe l'inizio è la riga dopo cui si inserisce (0 = inizio del file).
            let start_line = |seen: usize, count: usize| number(seen + usize::from(count > 0));
            Hunk {
                old_start: start_line(old_before, old_count),
                old_count: number(old_count),
                new_start: start_line(new_before, new_count),
                new_count: number(new_count),
                section: ByteText::default(),
                lines,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gli hunk scritti come li scrive git.
    fn text(old: &str, new: &str) -> String {
        let mut out = Vec::new();
        for hunk in hunks(old.as_bytes(), new.as_bytes()) {
            hunk.write_to(&mut out);
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn uguali_nessun_hunk() {
        assert_eq!(text("a\nb\n", "a\nb\n"), "");
        assert_eq!(text("", ""), "");
    }

    #[test]
    fn file_nuovo_ed_eliminato() {
        assert_eq!(text("", "a\nb\n"), "@@ -0,0 +1,2 @@\n+a\n+b\n");
        assert_eq!(text("a\n", ""), "@@ -1 +0,0 @@\n-a\n");
    }

    #[test]
    fn modifica_con_contesto() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let new = "1\n2\n3\n4\nCINQUE\n6\n7\n8\n";
        assert_eq!(
            text(old, new),
            "@@ -2,7 +2,7 @@\n 2\n 3\n 4\n-5\n+CINQUE\n 6\n 7\n 8\n"
        );
    }

    #[test]
    fn hunk_lontani_restano_separati_quelli_vicini_si_uniscono() {
        let file = |changed: &[usize]| -> String {
            (1..=20)
                .map(|n| match changed.contains(&n) {
                    true => format!("riga {n} cambiata\n"),
                    false => format!("{n}\n"),
                })
                .collect()
        };
        let old = file(&[]);
        // Tra le due modifiche 7 righe uguali: due hunk.
        let far = text(&old, &file(&[2, 10]));
        assert_eq!(far.matches("@@ -").count(), 2, "{far}");
        assert!(far.starts_with("@@ -1,5 +1,5 @@\n"), "{far}");
        assert!(far.contains("@@ -7,7 +7,7 @@\n"), "{far}");
        // Con 6 righe uguali: un solo hunk.
        let near = text(&old, &file(&[2, 9]));
        assert!(near.starts_with("@@ -1,12 +1,12 @@\n"), "{near}");
        assert_eq!(near.matches("@@ -").count(), 1, "{near}");
    }

    #[test]
    fn ultima_riga_senza_a_capo() {
        assert_eq!(
            text("a\nb", "a\nb\n"),
            "@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n"
        );
        let h = hunks(b"x", b"y");
        assert!(h[0].lines.iter().all(|l| l.no_newline));
    }

    #[test]
    fn inserimento_in_mezzo() {
        assert_eq!(
            text("a\nb\n", "a\nnuova\nb\n"),
            "@@ -1,2 +1,3 @@\n a\n+nuova\n b\n"
        );
    }
}
