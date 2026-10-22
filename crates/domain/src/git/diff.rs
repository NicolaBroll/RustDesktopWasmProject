//! Parser del formato unified diff di `git diff`, e il serializzatore inverso.
//!
//! # Formato
//!
//! Con [`DIFF_ARGS`] l'output è una sequenza di sezioni, una per file:
//!
//! ```text
//! diff --git a/<vecchio> b/<nuovo>
//! [old mode <m>, new mode <m> | new file mode <m> | deleted file mode <m>]
//! [similarity index <n>%, rename from <p>, rename to <p> | copy from <p>, copy to <p>]
//! [index <id>..<id>[ <modo>]]
//! [Binary files a/<p> and b/<p> differ
//!  | --- a/<p>|/dev/null, +++ b/<p>|/dev/null, hunk...]
//! ```
//!
//! Comportamento verificato con git 2.43:
//!
//! - le intestazioni hanno sempre quest'ordine; mancano quelle che non servono: un cambio di
//!   solo modo o una rinomina esatta non hanno `index` né hunk, un file nuovo vuoto ha solo
//!   `new file mode` e `index`;
//! - `index` ha il modo in fondo solo se è uguale nei due lati (mai per file nuovi o eliminati);
//!   con `--full-index` gli id sono completi;
//! - un nome che contiene `"`, `\` o caratteri di controllo (a capo, tab, ...) è tra
//!   virgolette con escape in stile C, prefisso compreso (`"a/con\ttab.txt"`), anche con
//!   `core.quotePath=false`; con `core.quotePath=true` anche i byte non ASCII diventano escape
//!   ottali. Il parser decodifica entrambe le forme; [`FileDiff::write_header`] scrive la prima;
//! - dopo `--- <nome>` e `+++ <nome>` git aggiunge un tab se il nome scritto contiene uno
//!   spazio;
//! - un cambio di tipo (es. da file a link simbolico) è una coppia di sezioni sullo stesso
//!   percorso: file eliminato, poi file nuovo;
//! - un sottomodulo (modo `160000`) con `--submodule=short` è un file di testo di una riga,
//!   `Subproject commit <id>` (con `-dirty` se ha modifiche);
//! - l'hunk inizia con `@@ -<inizio>[,<righe>] +<inizio>[,<righe>] @@[ <contesto>]`: il
//!   numero di righe manca quando vale 1; con 0 righe l'inizio è la riga precedente;
//!   `<contesto>` è la riga della funzione che contiene l'hunk, se git la trova;
//! - ogni riga dell'hunk inizia con ` ` (contesto), `-` (tolta) o `+` (aggiunta); una riga
//!   `\ No newline at end of file` dopo una riga dice che quella riga non ha l'a capo finale.
//!   Con `diff.suppressBlankEmpty` una riga di contesto vuota è scritta senza lo spazio: il
//!   parser la accetta. Il parser conta le righe dall'intestazione dell'hunk, quindi una riga
//!   tolta `-- x` (scritta `--- x`) non è scambiata per un'intestazione;
//! - durante un merge con conflitti, nel worktree un file in conflitto è un diff combinato
//!   (`diff --cc <nome>` con hunk `@@@`), e nell'indice una riga `* Unmerged path <nome>` (con
//!   il nome senza virgolette né escape). Il parser li riporta come [`Change::Unmerged`] senza
//!   hunk: il contenuto del diff combinato non è letto.

use super::quote::{unquote, unquote_prefix, write_quoted};
use super::status::{mode, oid, show};
use super::{ByteText, Change, Oid, RepoPath};
use crate::DomainError;
use serde::{Deserialize, Serialize};

/// Argomenti obbligatori di `git diff` per produrre l'input di [`parse_diff`], insieme a
/// `-c core.quotePath=false` prima del sottocomando (il backend lo passa già a ogni comando).
///
/// Chi chiama aggiunge cosa confrontare (`--cached`, revisioni) e i percorsi dopo `--`; può
/// aggiungere `-C`/`--find-copies-harder` o `--unified=<n>`, ma non opzioni che cambiano il
/// formato, come `--stat`, `--word-diff`, `--binary` o `-B`.
///
/// - `--no-color`: niente sequenze di colore con `color.diff=always`;
/// - `--no-ext-diff`: niente programma esterno da `diff.external` o dagli attributi;
/// - `--no-textconv`: il contenuto vero dei file, non quello convertito da un filtro `textconv`;
/// - `--find-renames`: rinomine riconosciute anche con `diff.renames=false`;
/// - `--full-index`: id completi nella riga `index`, per leggerli come [`Oid`];
/// - `--src-prefix=a/` e `--dst-prefix=b/`: prefissi fissi qualunque siano `diff.noprefix` e
///   `diff.mnemonicPrefix` (che li toglierebbe o li cambierebbe in `i/`, `w/`, `c/`...);
/// - `--submodule=short`: un sottomodulo come riga `Subproject commit <id>` anche con
///   `diff.submodule=log`, che scriverebbe l'elenco dei commit al posto del diff;
/// - `--no-relative`: percorsi dalla radice del repo anche con `diff.relative=true`.
pub const DIFF_ARGS: &[&str] = &[
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--find-renames",
    "--full-index",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--submodule=short",
    "--no-relative",
];

/// Il testo che git scrive dopo una riga senza a capo finale.
pub const NO_NEWLINE_MARKER: &[u8] = b"\\ No newline at end of file";

const GITLINK: u32 = 0o160000;

/// Un diff: un elemento per file, nell'ordine di git.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Diff {
    pub files: Vec<FileDiff>,
}

impl Diff {
    /// Riscrive il diff nel formato di git (vedi [`FileDiff::to_patch`]). Per un diff letto da
    /// `git diff` con [`DIFF_ARGS`] e senza conflitti il risultato è identico all'originale.
    pub fn to_patch(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for file in &self.files {
            file.write_patch(&mut out);
        }
        out
    }
}

/// Le modifiche a un file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    /// Percorso prima della modifica (senza `a/`). Per un file nuovo è uguale a `new_path`,
    /// come nella riga `diff --git` di git.
    pub old_path: RepoPath,
    /// Percorso dopo la modifica (senza `b/`). Per un file eliminato è uguale a `old_path`.
    pub new_path: RepoPath,
    /// Solo [`Change::Added`], [`Change::Deleted`], [`Change::Modified`] (contenuto, modo o
    /// entrambi), [`Change::Renamed`], [`Change::Copied`] o [`Change::Unmerged`] (file in
    /// conflitto: senza hunk, modi né id).
    pub change: Change,
    /// Somiglianza in percentuale (0–100) per rinomine e copie.
    pub similarity: Option<u8>,
    /// Modo prima della modifica, in ottale (es. `0o100644`, `0o100755`, `0o120000` per un
    /// link, `0o160000` per un sottomodulo). `None` per un file nuovo, o se git non lo scrive
    /// (rinomina esatta senza cambio di modo).
    pub old_mode: Option<u32>,
    /// Modo dopo la modifica. `None` per un file eliminato, o se git non lo scrive.
    pub new_mode: Option<u32>,
    /// Id del contenuto prima della modifica (tutti zeri per un file nuovo); `None` se git non
    /// scrive la riga `index` (contenuto invariato: cambio di solo modo, rinomina esatta).
    pub old_oid: Option<Oid>,
    /// Id del contenuto dopo la modifica (tutti zeri per un file eliminato); `None` come sopra.
    pub new_oid: Option<Oid>,
    /// Vero se git considera il file binario (`Binary files ... differ`): niente hunk.
    pub binary: bool,
    /// Gli hunk, in ordine. Vuoto per binari, file vuoti, cambi di solo modo, rinomine esatte e
    /// file in conflitto.
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// Il percorso da mostrare: quello nuovo (per un file eliminato coincide con il vecchio).
    pub fn path(&self) -> &RepoPath {
        &self.new_path
    }

    /// Vero se il percorso è un sottomodulo (modo `160000`) prima o dopo la modifica.
    pub fn is_submodule(&self) -> bool {
        self.old_mode == Some(GITLINK) || self.new_mode == Some(GITLINK)
    }

    /// Numero di righe aggiunte e tolte, per un riepilogo come `+3 −1`.
    pub fn line_counts(&self) -> (usize, usize) {
        let lines = self.hunks.iter().flat_map(|h| &h.lines);
        lines.fold((0, 0), |(add, del), l| match l.kind {
            LineKind::Added => (add + 1, del),
            LineKind::Removed => (add, del + 1),
            LineKind::Context => (add, del),
        })
    }

    /// Le intestazioni del file come le scrive git (da `diff --git` a `+++`, oppure fino a
    /// `Binary files ... differ`), con i nomi tra virgolette solo dove servono. Seguite dagli
    /// hunk (vedi [`Hunk::write_to`]) formano una patch per `git apply`.
    ///
    /// Le righe `---`/`+++` ci sono solo se il file ha hunk, come in git. Per un file in
    /// conflitto scrive solo `* Unmerged path <nome>`, che `git apply` non accetta.
    pub fn write_header(&self, out: &mut Vec<u8>) {
        if self.change == Change::Unmerged {
            out.extend_from_slice(b"* Unmerged path ");
            out.extend_from_slice(self.new_path.as_bytes());
            out.push(b'\n');
            return;
        }
        let (old, new) = (self.old_path.as_bytes(), self.new_path.as_bytes());
        out.extend_from_slice(b"diff --git ");
        write_quoted(out, b"a/", old);
        out.push(b' ');
        write_quoted(out, b"b/", new);
        out.push(b'\n');

        let added = self.change == Change::Added;
        let deleted = self.change == Change::Deleted;
        if added {
            line(
                out,
                &format!("new file mode {:06o}", self.new_mode.unwrap_or(0o100644)),
            );
        } else if deleted {
            line(
                out,
                &format!(
                    "deleted file mode {:06o}",
                    self.old_mode.unwrap_or(0o100644)
                ),
            );
        } else if let (Some(o), Some(n)) = (self.old_mode, self.new_mode)
            && o != n
        {
            line(out, &format!("old mode {o:06o}"));
            line(out, &format!("new mode {n:06o}"));
        }

        let verb: &[u8] = match self.change {
            Change::Renamed => b"rename",
            Change::Copied => b"copy",
            _ => b"",
        };
        if !verb.is_empty() {
            if let Some(score) = self.similarity {
                line(out, &format!("similarity index {score}%"));
            }
            for (dir, name) in [(&b" from "[..], old), (b" to ", new)] {
                out.extend_from_slice(verb);
                out.extend_from_slice(dir);
                write_quoted(out, b"", name);
                out.push(b'\n');
            }
        }

        if let (Some(o), Some(n)) = (self.old_oid, self.new_oid) {
            let same_mode = match (self.old_mode, self.new_mode) {
                (Some(a), Some(b)) if a == b && !added && !deleted => Some(a),
                _ => None,
            };
            match same_mode {
                Some(m) => line(out, &format!("index {o}..{n} {m:06o}")),
                None => line(out, &format!("index {o}..{n}")),
            }
        }

        let label = |prefix: &[u8], name: &[u8], missing: bool| {
            let mut l = Vec::new();
            if missing {
                l.extend_from_slice(b"/dev/null");
            } else {
                write_quoted(&mut l, prefix, name);
            }
            l
        };
        let minus = label(b"a/", old, added);
        let plus = label(b"b/", new, deleted);
        if self.binary {
            out.extend_from_slice(b"Binary files ");
            out.extend_from_slice(&minus);
            out.extend_from_slice(b" and ");
            out.extend_from_slice(&plus);
            out.extend_from_slice(b" differ\n");
        } else if !self.hunks.is_empty() {
            for (sign, l) in [(&b"--- "[..], minus), (b"+++ ", plus)] {
                out.extend_from_slice(sign);
                out.extend_from_slice(&l);
                if l.contains(&b' ') {
                    out.push(b'\t');
                }
                out.push(b'\n');
            }
        }
    }

    /// Intestazioni e hunk del file, nel formato di git.
    pub fn to_patch(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_patch(&mut out);
        out
    }

    fn write_patch(&self, out: &mut Vec<u8>) {
        self.write_header(out);
        if self.change != Change::Unmerged && !self.binary {
            for hunk in &self.hunks {
                hunk.write_to(out);
            }
        }
    }
}

/// Un blocco di righe cambiate, con il contesto intorno.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    /// Prima riga nel file vecchio (da 1); con `old_count == 0` è la riga dopo cui si
    /// inserisce (0 per l'inizio del file).
    pub old_start: u32,
    /// Righe del file vecchio nell'hunk (contesto e tolte).
    pub old_count: u32,
    /// Prima riga nel file nuovo, come `old_start`.
    pub new_start: u32,
    /// Righe del file nuovo nell'hunk (contesto e aggiunte).
    pub new_count: u32,
    /// Il testo dopo il secondo `@@` (senza lo spazio iniziale), di solito la riga della
    /// funzione che contiene l'hunk; vuoto se git non ne scrive.
    pub section: ByteText,
    pub lines: Vec<Line>,
}

impl Hunk {
    /// L'intestazione `@@ -a,b +c,d @@ contesto`, senza a capo; il numero di righe è omesso
    /// quando vale 1, come fa git.
    pub fn header(&self) -> Vec<u8> {
        let range = |start: u32, count: u32| match count {
            1 => start.to_string(),
            _ => format!("{start},{count}"),
        };
        let mut out = format!(
            "@@ -{} +{} @@",
            range(self.old_start, self.old_count),
            range(self.new_start, self.new_count)
        )
        .into_bytes();
        if !self.section.is_empty() {
            out.push(b' ');
            out.extend_from_slice(self.section.as_bytes());
        }
        out
    }

    /// Scrive intestazione e righe nel formato di git, con `\ No newline at end of file` dopo
    /// le righe senza a capo. I conteggi dell'intestazione sono quelli dei campi, non
    /// ricalcolati dalle righe (per una patch con righe tolte usare `git apply --recount`).
    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.header());
        out.push(b'\n');
        for l in &self.lines {
            l.write_to(out);
        }
    }
}

/// Il tipo di una riga di un hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LineKind {
    /// ` `: uguale nei due file.
    Context,
    /// `+`: solo nel file nuovo.
    Added,
    /// `-`: solo nel file vecchio.
    Removed,
}

impl LineKind {
    /// Il carattere con cui git inizia la riga.
    pub fn prefix(self) -> u8 {
        match self {
            LineKind::Context => b' ',
            LineKind::Added => b'+',
            LineKind::Removed => b'-',
        }
    }
}

/// Una riga di un hunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub kind: LineKind,
    /// Numero della riga nel file vecchio (da 1); `None` per le righe aggiunte.
    pub old_line: Option<u32>,
    /// Numero della riga nel file nuovo (da 1); `None` per le righe tolte.
    pub new_line: Option<u32>,
    /// Il contenuto senza il prefisso e senza l'a capo finale (un eventuale `\r` resta).
    pub content: ByteText,
    /// Vero se nel file la riga non ha l'a capo finale (seguita da
    /// `\ No newline at end of file`): può esserlo solo l'ultima riga di un lato.
    pub no_newline: bool,
}

impl Line {
    /// Scrive la riga nel formato di git: prefisso, contenuto, a capo e, se serve, la riga
    /// `\ No newline at end of file`.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.push(self.kind.prefix());
        out.extend_from_slice(self.content.as_bytes());
        out.push(b'\n');
        if self.no_newline {
            out.extend_from_slice(NO_NEWLINE_MARKER);
            out.push(b'\n');
        }
    }
}

fn line(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(text.as_bytes());
    out.push(b'\n');
}

/// Legge l'output di `git diff` prodotto con [`DIFF_ARGS`].
///
/// Un input vuoto dà un diff senza file. Un input che non termina con un a capo, una riga
/// inattesa, un'intestazione non valida, un hunk con meno righe di quelle dichiarate o un nome
/// senza i prefissi `a/`/`b/` danno [`DomainError::InvalidGitOutput`], con il numero della riga
/// (da 1) coinvolta.
pub fn parse_diff(input: &[u8]) -> Result<Diff, DomainError> {
    if input.is_empty() {
        return Ok(Diff::default());
    }
    let Some(body) = input.strip_suffix(b"\n") else {
        return Err(invalid(
            "l'output non termina con un a capo: è troncato".to_owned(),
        ));
    };
    let lines: Vec<&[u8]> = body.split(|&b| b == b'\n').collect();
    let mut parser = Parser { lines, pos: 0 };
    let mut files = Vec::new();
    while let Some(first) = parser.peek() {
        let n = parser.pos + 1;
        let file = if let Some(rest) = first.strip_prefix(b"diff --git ") {
            parser.pos += 1;
            parser.git_file(rest, n)
        } else if let Some(rest) = first
            .strip_prefix(b"diff --cc ")
            .or_else(|| first.strip_prefix(b"diff --combined "))
        {
            parser.pos += 1;
            parser.combined(rest, n)
        } else if let Some(rest) = first.strip_prefix(b"* Unmerged path ") {
            parser.pos += 1;
            Ok(parser.unmerged(rest))
        } else {
            Err(format!(
                "riga n. {n}: `{}` inattesa, atteso l'inizio di un file (`diff --git`)",
                show(first)
            ))
        };
        files.push(file.map_err(invalid)?);
    }
    Ok(Diff { files })
}

struct Parser<'a> {
    lines: Vec<&'a [u8]>,
    pos: usize,
}

/// Intestazioni lette per un file, prima di stabilire i percorsi.
#[derive(Default)]
struct Headers {
    change: Option<Change>,
    similarity: Option<u8>,
    old_mode: Option<u32>,
    new_mode: Option<u32>,
    oids: Option<(Oid, Oid)>,
    index_mode: Option<u32>,
    from: Option<RepoPath>,
    to: Option<RepoPath>,
    binary: bool,
    labels: Option<(Option<RepoPath>, Option<RepoPath>)>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a [u8]> {
        self.lines.get(self.pos).copied()
    }

    /// Vero se la riga corrente inizia un nuovo file (o non ci sono più righe).
    fn at_file_start(&self) -> bool {
        self.peek()
            .is_none_or(|l| l.starts_with(b"diff ") || l.starts_with(b"* Unmerged path "))
    }

    /// Una sezione `diff --git`; `n` è il numero della sua prima riga.
    fn git_file(&mut self, rest: &[u8], n: usize) -> Result<FileDiff, String> {
        let at = |n: usize, reason: String| format!("riga n. {n}: {reason}");
        let git_paths = git_line_paths(rest).map_err(|e| at(n, e))?;
        let mut h = Headers::default();
        let set_change = |h: &mut Headers, c: Change, n: usize| match h.change {
            Some(old) if old != c => Err(at(n, "intestazioni di tipi di modifica diversi".into())),
            _ => {
                h.change = Some(c);
                Ok(())
            }
        };
        while let Some(l) = self.peek() {
            let n = self.pos + 1;
            let err = |reason: String| at(n, reason);
            if l.starts_with(b"diff ") || l.starts_with(b"* Unmerged path ") {
                break;
            }
            self.pos += 1;
            if let Some(m) = l.strip_prefix(b"old mode ") {
                h.old_mode = Some(mode(m).map_err(err)?);
            } else if let Some(m) = l.strip_prefix(b"new mode ") {
                h.new_mode = Some(mode(m).map_err(err)?);
            } else if let Some(m) = l.strip_prefix(b"deleted file mode ") {
                set_change(&mut h, Change::Deleted, n)?;
                h.old_mode = Some(mode(m).map_err(err)?);
            } else if let Some(m) = l.strip_prefix(b"new file mode ") {
                set_change(&mut h, Change::Added, n)?;
                h.new_mode = Some(mode(m).map_err(err)?);
            } else if let Some(s) = l.strip_prefix(b"similarity index ") {
                h.similarity = Some(percent(s).map_err(err)?);
            } else if let Some(p) = l.strip_prefix(b"rename from ") {
                set_change(&mut h, Change::Renamed, n)?;
                h.from = Some(header_path(p).map_err(err)?);
            } else if let Some(p) = l.strip_prefix(b"rename to ") {
                set_change(&mut h, Change::Renamed, n)?;
                h.to = Some(header_path(p).map_err(err)?);
            } else if let Some(p) = l.strip_prefix(b"copy from ") {
                set_change(&mut h, Change::Copied, n)?;
                h.from = Some(header_path(p).map_err(err)?);
            } else if let Some(p) = l.strip_prefix(b"copy to ") {
                set_change(&mut h, Change::Copied, n)?;
                h.to = Some(header_path(p).map_err(err)?);
            } else if let Some(i) = l.strip_prefix(b"index ") {
                let (o, new, m) = index_line(i).map_err(err)?;
                h.oids = Some((o, new));
                h.index_mode = m;
            } else if l.starts_with(b"Binary files ") && l.ends_with(b" differ") {
                h.binary = true;
                break;
            } else if let Some(minus) = l.strip_prefix(b"--- ") {
                let minus = label(minus, b"a/").map_err(err)?;
                let n = self.pos + 1;
                let plus = match self.peek().and_then(|p| p.strip_prefix(b"+++ ")) {
                    Some(p) => label(p, b"b/").map_err(|e| at(n, e))?,
                    None => return Err(at(n, "manca la riga `+++` dopo `---`".into())),
                };
                self.pos += 1;
                h.labels = Some((minus, plus));
                break;
            } else if l.starts_with(b"GIT binary patch") {
                return Err(err(
                    "patch binaria (`--binary`) non supportata: usa gli argomenti di DIFF_ARGS"
                        .into(),
                ));
            } else if l.starts_with(b"dissimilarity index ") {
                return Err(err(
                    "riscrittura completa (`-B`) non supportata: usa gli argomenti di DIFF_ARGS"
                        .into(),
                ));
            } else if l.starts_with(b"@@") {
                return Err(err("hunk senza le righe `---` e `+++` prima".into()));
            } else {
                return Err(err(format!("intestazione `{}` sconosciuta", show(l))));
            }
        }

        let change = h.change.unwrap_or(Change::Modified);
        let (old_path, new_path) = match (h.from, h.to, &h.labels) {
            (Some(from), Some(to), _) => (from, to),
            (None, None, Some((minus, plus))) => match (minus, plus) {
                (Some(a), Some(b)) => (a.clone(), b.clone()),
                (Some(a), None) => (a.clone(), a.clone()),
                (None, Some(b)) => (b.clone(), b.clone()),
                (None, None) => {
                    return Err(at(n, "`---` e `+++` sono entrambi `/dev/null`".into()));
                }
            },
            (None, None, None) => git_paths.ok_or_else(|| {
                at(
                    n,
                    format!(
                        "impossibile ricavare i percorsi da `diff --git {}`",
                        show(rest)
                    ),
                )
            })?,
            _ => {
                return Err(at(
                    n,
                    "rinomina o copia senza entrambe le righe `from` e `to`".into(),
                ));
            }
        };
        let (mut old_mode, mut new_mode) = (h.old_mode, h.new_mode);
        if let Some(m) = h.index_mode {
            old_mode = old_mode.or(Some(m));
            new_mode = new_mode.or(Some(m));
        }
        if matches!(change, Change::Added) && h.labels.as_ref().is_some_and(|l| l.0.is_some()) {
            return Err(at(n, "file nuovo con `---` diverso da `/dev/null`".into()));
        }
        if matches!(change, Change::Deleted) && h.labels.as_ref().is_some_and(|l| l.1.is_some()) {
            return Err(at(
                n,
                "file eliminato con `+++` diverso da `/dev/null`".into(),
            ));
        }

        let mut hunks = Vec::new();
        if h.labels.is_some() {
            while let Some(l) = self.peek() {
                if !l.starts_with(b"@@") {
                    break;
                }
                hunks.push(self.hunk(l)?);
            }
            if hunks.is_empty() {
                return Err(at(self.pos + 1, "nessun hunk dopo la riga `+++`".into()));
            }
            if !self.at_file_start() {
                let l = self.peek().unwrap_or_default();
                return Err(at(
                    self.pos + 1,
                    format!("`{}` inattesa dopo l'ultimo hunk", show(l)),
                ));
            }
        }

        Ok(FileDiff {
            old_path,
            new_path,
            change,
            similarity: h.similarity,
            old_mode,
            new_mode,
            old_oid: h.oids.map(|o| o.0),
            new_oid: h.oids.map(|o| o.1),
            binary: h.binary,
            hunks,
        })
    }

    /// Un hunk che inizia alla riga corrente (`header`).
    fn hunk(&mut self, header: &[u8]) -> Result<Hunk, String> {
        let n = self.pos + 1;
        let at = |n: usize, reason: String| format!("riga n. {n}: {reason}");
        let (old_start, old_count, new_start, new_count, section) =
            hunk_header(header).map_err(|e| at(n, e))?;
        self.pos += 1;
        let (mut old_left, mut new_left) = (old_count, new_count);
        let (mut old_no, mut new_no) = (old_start, new_start);
        let mut lines: Vec<Line> = Vec::new();
        loop {
            let pending = old_left > 0 || new_left > 0;
            let n = self.pos + 1;
            let Some(l) = self.peek() else {
                if pending {
                    return Err(at(
                        n,
                        format!(
                            "hunk troncato: mancano {old_left} righe del file vecchio e \
                             {new_left} del nuovo"
                        ),
                    ));
                }
                break;
            };
            if l.starts_with(b"\\") {
                let Some(last) = lines.last_mut() else {
                    return Err(at(
                        n,
                        "`\\` all'inizio di un hunk, senza una riga prima".into(),
                    ));
                };
                last.no_newline = true;
                self.pos += 1;
                continue;
            }
            if !pending {
                break;
            }
            let (kind, content) = match l.split_first() {
                Some((b' ', c)) => (LineKind::Context, c),
                // `diff.suppressBlankEmpty`: riga di contesto vuota senza lo spazio.
                None => (LineKind::Context, &[][..]),
                Some((b'-', c)) => (LineKind::Removed, c),
                Some((b'+', c)) => (LineKind::Added, c),
                Some(_) => {
                    return Err(at(
                        n,
                        format!(
                            "`{}` non è una riga di hunk (deve iniziare con ` `, `-`, `+` o \
                             `\\`); mancano {old_left} righe del file vecchio e {new_left} del \
                             nuovo",
                            show(l)
                        ),
                    ));
                }
            };
            let uses_old = kind != LineKind::Added;
            let uses_new = kind != LineKind::Removed;
            if (uses_old && old_left == 0) || (uses_new && new_left == 0) {
                return Err(at(
                    n,
                    format!(
                        "`{}`: l'hunk ha più righe di quelle dichiarate in `{}`",
                        show(l),
                        show(header)
                    ),
                ));
            }
            let mut line = Line {
                kind,
                old_line: None,
                new_line: None,
                content: ByteText::from(content),
                no_newline: false,
            };
            if uses_old {
                line.old_line = Some(old_no);
                old_left -= 1;
                old_no = old_no.saturating_add(1);
            }
            if uses_new {
                line.new_line = Some(new_no);
                new_left -= 1;
                new_no = new_no.saturating_add(1);
            }
            lines.push(line);
            self.pos += 1;
        }
        Ok(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            section: ByteText::from(section),
            lines,
        })
    }

    /// `diff --cc <nome>`: file in conflitto; il contenuto del diff combinato si salta fino al
    /// file seguente (le sue righe iniziano con ` `, `+`, `-` o `\`, mai con `diff `).
    fn combined(&mut self, rest: &[u8], n: usize) -> Result<FileDiff, String> {
        let path = unquote(rest).map_err(|e| format!("riga n. {n}: {e}"))?;
        if path.is_empty() {
            return Err(format!("riga n. {n}: percorso vuoto in `diff --cc`"));
        }
        while !self.at_file_start() {
            self.pos += 1;
        }
        Ok(unmerged_file(RepoPath::from(path)))
    }

    /// `* Unmerged path <nome>`: il nome è scritto così com'è, quindi un a capo nel nome lo
    /// continua sulle righe seguenti, fino all'inizio del file successivo.
    fn unmerged(&mut self, rest: &[u8]) -> FileDiff {
        let mut path = rest.to_vec();
        while !self.at_file_start() {
            path.push(b'\n');
            path.extend_from_slice(self.peek().unwrap_or_default());
            self.pos += 1;
        }
        unmerged_file(RepoPath::from(path))
    }
}

pub(super) fn unmerged_file(path: RepoPath) -> FileDiff {
    FileDiff {
        old_path: path.clone(),
        new_path: path,
        change: Change::Unmerged,
        similarity: None,
        old_mode: None,
        new_mode: None,
        old_oid: None,
        new_oid: None,
        binary: false,
        hunks: Vec::new(),
    }
}

/// I due percorsi della riga `diff --git a/<vecchio> b/<nuovo>`, senza prefissi. `None` se
/// sono ambigui (due nomi senza virgolette, diversi, con ` b/` ripetuto): allora servono le
/// righe `rename`/`copy` o `---`/`+++`.
fn git_line_paths(rest: &[u8]) -> Result<Option<(RepoPath, RepoPath)>, String> {
    let strip = |name: Vec<u8>, prefix: &[u8]| -> Result<RepoPath, String> {
        match name.strip_prefix(prefix) {
            Some(p) if !p.is_empty() => Ok(RepoPath::from(p)),
            _ => Err(missing_prefix(&name, prefix)),
        }
    };
    if rest.starts_with(b"\"") {
        let (a, used) = unquote_prefix(rest)?;
        let Some(b) = rest[used..].strip_prefix(b" ") else {
            return Err(format!("manca lo spazio fra i nomi in `{}`", show(rest)));
        };
        return Ok(Some((strip(a, b"a/")?, strip(unquote(b)?, b"b/")?)));
    }
    if rest.ends_with(b"\"") {
        // Primo nome senza virgolette, secondo con: si cerca il ` "` da cui il resto è
        // esattamente una stringa tra virgolette.
        for (i, w) in rest.windows(2).enumerate() {
            if w == b" \""
                && let Ok(b) = unquote(&rest[i + 1..])
            {
                return Ok(Some((strip(rest[..i].to_vec(), b"a/")?, strip(b, b"b/")?)));
            }
        }
        return Err(format!(
            "nomi tra virgolette non validi in `{}`",
            show(rest)
        ));
    }
    // Stesso nome nei due lati: `a/<n> b/<n>`.
    if rest.len() >= 5 && (rest.len() - 5).is_multiple_of(2) {
        let n = (rest.len() - 5) / 2;
        let (a, b) = rest.split_at(2 + n);
        if a.starts_with(b"a/") && b.starts_with(b" b/") && a[2..] == b[3..] {
            let p = RepoPath::from(&a[2..]);
            return Ok(Some((p.clone(), p)));
        }
    }
    let seps: Vec<usize> = rest
        .windows(3)
        .enumerate()
        .filter(|(_, w)| *w == b" b/")
        .map(|(i, _)| i)
        .collect();
    match seps[..] {
        [i] => Ok(Some((
            strip(rest[..i].to_vec(), b"a/")?,
            strip(rest[i + 1..].to_vec(), b"b/")?,
        ))),
        _ => Ok(None),
    }
}

/// Il nome di `---`/`+++`: `None` per `/dev/null`, altrimenti senza `prefix`. Il tab che git
/// aggiunge dopo un nome con spazi si toglie.
fn label(raw: &[u8], prefix: &[u8]) -> Result<Option<RepoPath>, String> {
    let name = if raw.starts_with(b"\"") {
        let (name, used) = unquote_prefix(raw)?;
        match &raw[used..] {
            [] | [b'\t'] => name,
            _ => return Err(format!("testo dopo il nome in `{}`", show(raw))),
        }
    } else {
        let name = raw.strip_suffix(b"\t").unwrap_or(raw);
        if name == b"/dev/null" {
            return Ok(None);
        }
        name.to_vec()
    };
    match name.strip_prefix(prefix) {
        Some(p) if !p.is_empty() => Ok(Some(RepoPath::from(p))),
        _ => Err(missing_prefix(&name, prefix)),
    }
}

fn missing_prefix(name: &[u8], prefix: &[u8]) -> String {
    format!(
        "il nome `{}` non inizia con `{}`: serve `--src-prefix=a/ --dst-prefix=b/` (DIFF_ARGS)",
        show(name),
        show(prefix)
    )
}

/// Un percorso di `rename from`/`rename to`/`copy from`/`copy to`, senza prefisso.
fn header_path(raw: &[u8]) -> Result<RepoPath, String> {
    let p = unquote(raw)?;
    if p.is_empty() {
        return Err("percorso vuoto".to_owned());
    }
    Ok(RepoPath::from(p))
}

/// `<n>%` con `n` fra 0 e 100.
fn percent(s: &[u8]) -> Result<u8, String> {
    s.strip_suffix(b"%")
        .and_then(super::log::parse_digits)
        .and_then(|v| u8::try_from(v).ok())
        .filter(|v| *v <= 100)
        .ok_or_else(|| format!("`{}` non è una percentuale valida", show(s)))
}

/// `<id>..<id>[ <modo>]`.
fn index_line(s: &[u8]) -> Result<(Oid, Oid, Option<u32>), String> {
    let (ids, m) = match s.iter().position(|&b| b == b' ') {
        Some(i) => (&s[..i], Some(mode(&s[i + 1..])?)),
        None => (s, None),
    };
    let Some(dots) = ids.windows(2).position(|w| w == b"..") else {
        return Err(format!("riga `index {}` senza `..`", show(s)));
    };
    let hint = |e: String| format!("{e}; con `--full-index` (DIFF_ARGS) gli id sono completi");
    let old = oid(&ids[..dots], "id vecchio").map_err(hint)?;
    let new = oid(&ids[dots + 2..], "id nuovo").map_err(hint)?;
    Ok((old, new, m))
}

/// `@@ -a[,b] +c[,d] @@[ contesto]`.
fn hunk_header(h: &[u8]) -> Result<(u32, u32, u32, u32, &[u8]), String> {
    let bad = || {
        format!(
            "`{}` non è un'intestazione di hunk `@@ -a,b +c,d @@`",
            show(h)
        )
    };
    let rest = h.strip_prefix(b"@@ -").ok_or_else(bad)?;
    let sp = rest.iter().position(|&b| b == b' ').ok_or_else(bad)?;
    let (old_start, old_count) = range(&rest[..sp]).ok_or_else(bad)?;
    let rest = rest[sp + 1..].strip_prefix(b"+").ok_or_else(bad)?;
    let sp = rest.iter().position(|&b| b == b' ').ok_or_else(bad)?;
    let (new_start, new_count) = range(&rest[..sp]).ok_or_else(bad)?;
    let rest = rest[sp + 1..].strip_prefix(b"@@").ok_or_else(bad)?;
    let section = match rest {
        [] => rest,
        [b' ', s @ ..] => s,
        _ => return Err(bad()),
    };
    Ok((old_start, old_count, new_start, new_count, section))
}

/// `<inizio>[,<righe>]`; senza righe vale 1.
fn range(s: &[u8]) -> Option<(u32, u32)> {
    let num = |d: &[u8]| super::log::parse_digits(d).and_then(|v| u32::try_from(v).ok());
    match s.iter().position(|&b| b == b',') {
        Some(i) => Some((num(&s[..i])?, num(&s[i + 1..])?)),
        None => Some((num(s)?, 1)),
    }
}

fn invalid(reason: String) -> DomainError {
    DomainError::InvalidGitOutput {
        command: "diff",
        reason,
    }
}
