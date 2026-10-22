//! Parser dell'output di `git status --porcelain=v2 -z --branch --untracked-files=all`.
//!
//! # Formato
//!
//! Con `-z` ogni record è **terminato** da un NUL, ultimo compreso; i percorsi non sono mai
//! tra virgolette né con escape, quindi possono contenere spazi, a capo e byte non UTF-8. Prima
//! vengono le intestazioni (con `--branch`), poi una voce per file:
//!
//! ```text
//! # branch.oid <commit> | (initial)
//! # branch.head <branch> | (detached)
//! # branch.upstream <upstream>            solo se c'è un upstream
//! # branch.ab +<ahead> -<behind>          solo se l'upstream esiste ancora
//! 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
//! 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path> \0 <origPath>
//! u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
//! ? <path>
//! ! <path>                                solo con `--ignored`
//! ```
//!
//! Comportamento verificato con git 2.43:
//!
//! - in un repo senza commit `branch.oid` è `(initial)`; con HEAD staccata `branch.head` è
//!   `(detached)` e `branch.oid` è il commit;
//! - `branch.upstream` è il nome **breve** (es. `origin/main`); se l'upstream è configurato ma il
//!   ref non esiste più `branch.ab` manca;
//! - nel record `2` il percorso originale è un campo a sé dopo un NUL; `<X><score>` è `R` o `C`
//!   seguito dalla somiglianza in percentuale (le copie compaiono solo con
//!   `status.renames=copies`);
//! - `<sub>` è `N...` per i file normali e `S<c><m><u>` per i sottomoduli;
//! - i modi sono in ottale a sei cifre, gli id sono completi (zeri se l'oggetto non esiste);
//! - le intestazioni sconosciute (es. `# stash <n>` con `--show-stash`) vanno ignorate, come
//!   chiede la documentazione di git.
//!
//! L'operazione in corso (merge, rebase, ...) non compare nell'output: [`parse_status`] lascia
//! [`Status::operation`] a `None` e la riempie il backend leggendo `.git`.

use super::log::parse_digits;
use super::{
    Change, Conflict, EntryKind, Head, Oid, RepoPath, Status, StatusEntry, SubmoduleStatus,
    TrackedInfo, UnmergedInfo, Upstream,
};
use crate::DomainError;

/// Argomenti obbligatori di `git status` per produrre l'input di [`parse_status`]. Chi chiama
/// può aggiungere `--ignored` (voci [`EntryKind::Ignored`]) o dei percorsi dopo `--`; non vanno
/// aggiunte opzioni che cambiano il formato, come `--short` o `--porcelain=v1`.
///
/// - `--porcelain=v2`: formato stabile, con modi, id e percorso originale delle rinomine;
/// - `-z`: record terminati da NUL e percorsi senza virgolette né escape;
/// - `--branch`: intestazioni con HEAD, upstream e ahead/behind;
/// - `--untracked-files=all`: un record per ogni file non tracciato, anche dentro cartelle
///   nuove (senza, git mostra solo la cartella).
pub const STATUS_ARGS: &[&str] = &["--porcelain=v2", "-z", "--branch", "--untracked-files=all"];

/// Legge l'output di `git status` prodotto con [`STATUS_ARGS`].
///
/// Le voci restano nell'ordine di git. [`Status::operation`] è sempre `None`: l'output non la
/// contiene. Un input che non termina con NUL, senza le intestazioni `branch.oid` e
/// `branch.head`, con un tipo di record sconosciuto o con campi non validi dà
/// [`DomainError::InvalidGitOutput`], con il numero del record (da 1) coinvolto.
pub fn parse_status(input: &[u8]) -> Result<Status, DomainError> {
    let body = if input.is_empty() {
        input
    } else {
        input.strip_suffix(b"\0").ok_or_else(|| {
            invalid(
                "l'output non termina con NUL: è troncato o non è stato prodotto con `-z`"
                    .to_owned(),
            )
        })?
    };
    let mut headers = Headers::default();
    let mut entries = Vec::new();
    let mut tokens = body.split(|&b| b == 0);
    let mut n = 0;
    // Con `body` vuoto `split` dà un solo token vuoto: nessun record.
    while let Some(record) = tokens.next().filter(|_| !body.is_empty()) {
        n += 1;
        let err = |reason: String| invalid(format!("record n. {n}: {reason}"));
        let (kind, rest) = match record {
            [kind, b' ', rest @ ..] => (*kind, rest),
            [] => return Err(err("record vuoto".to_owned())),
            _ => {
                return Err(err(format!(
                    "`{}` non è un record valido (atteso `<tipo> <campi>`)",
                    show(record)
                )));
            }
        };
        match kind {
            b'#' => headers.read(rest).map_err(err)?,
            b'1' => entries.push(parse_ordinary(rest).map_err(err)?),
            b'2' => {
                let original = tokens.next().ok_or_else(|| {
                    err("manca il percorso originale della rinomina dopo il NUL".to_owned())
                })?;
                entries.push(parse_renamed(rest, original).map_err(err)?);
            }
            b'u' => entries.push(parse_unmerged(rest).map_err(err)?),
            b'?' => {
                entries.push(simple(rest, Change::Untracked, EntryKind::Untracked).map_err(err)?)
            }
            b'!' => entries.push(simple(rest, Change::Ignored, EntryKind::Ignored).map_err(err)?),
            other => {
                return Err(err(format!(
                    "tipo di record `{}` sconosciuto (attesi `#`, `1`, `2`, `u`, `?`, `!`)",
                    char::from(other).escape_default()
                )));
            }
        }
    }
    let (head, upstream) = headers.finish()?;
    Ok(Status {
        head,
        upstream,
        entries,
        operation: None,
    })
}

/// Intestazioni lette finora.
#[derive(Default)]
struct Headers {
    /// `Some(None)` per `(initial)`.
    oid: Option<Option<Oid>>,
    /// `Some(None)` per `(detached)`.
    head: Option<Option<String>>,
    upstream: Option<String>,
    ab: Option<(u32, u32)>,
}

impl Headers {
    fn read(&mut self, line: &[u8]) -> Result<(), String> {
        let (key, value) = match line.iter().position(|&b| b == b' ') {
            Some(i) => (&line[..i], &line[i + 1..]),
            None => (line, &line[line.len()..]),
        };
        let once = |present: bool| {
            if present {
                Err(format!("intestazione `{}` ripetuta", show(key)))
            } else {
                Ok(())
            }
        };
        match key {
            b"branch.oid" => {
                once(self.oid.is_some())?;
                self.oid = Some(if value == b"(initial)" {
                    None
                } else {
                    Some(oid(value, "branch.oid")?)
                });
            }
            b"branch.head" => {
                once(self.head.is_some())?;
                if value.is_empty() {
                    return Err("`branch.head` senza nome del branch".to_owned());
                }
                self.head = Some((value != b"(detached)").then(|| show(value)));
            }
            b"branch.upstream" => {
                once(self.upstream.is_some())?;
                if value.is_empty() {
                    return Err("`branch.upstream` senza nome".to_owned());
                }
                self.upstream = Some(show(value));
            }
            b"branch.ab" => {
                once(self.ab.is_some())?;
                self.ab = Some(parse_ab(value)?);
            }
            // Intestazioni future o opzionali (es. `stash`): da ignorare.
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> Result<(Head, Option<Upstream>), DomainError> {
        let missing = |key: &str| {
            invalid(format!(
                "manca l'intestazione `# {key}`: l'output non è stato prodotto con `--branch`"
            ))
        };
        let oid = self.oid.ok_or_else(|| missing("branch.oid"))?;
        let head = match self.head.ok_or_else(|| missing("branch.head"))? {
            Some(name) => Head::Branch { name, target: oid },
            None => Head::Detached(oid.ok_or_else(|| {
                invalid("HEAD staccata (`(detached)`) ma `branch.oid` è `(initial)`".to_owned())
            })?),
        };
        let upstream = match (self.upstream, self.ab) {
            (Some(name), ab) => Some(Upstream {
                name,
                ahead: ab.map(|(a, _)| a),
                behind: ab.map(|(_, b)| b),
            }),
            (None, Some(_)) => {
                return Err(invalid(
                    "`branch.ab` presente senza `branch.upstream`".to_owned(),
                ));
            }
            (None, None) => None,
        };
        Ok((head, upstream))
    }
}

/// `+<ahead> -<behind>`.
fn parse_ab(value: &[u8]) -> Result<(u32, u32), String> {
    let bad = || {
        format!(
            "`branch.ab {}` non valido (atteso `+<ahead> -<behind>`)",
            show(value)
        )
    };
    let mut parts = value.split(|&b| b == b' ');
    let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(bad());
    };
    let num = |s: &[u8], sign: u8| {
        s.strip_prefix(&[sign])
            .and_then(parse_digits)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(bad)
    };
    Ok((num(a, b'+')?, num(b, b'-')?))
}

/// Divide `rest` in `n` campi separati da uno spazio; l'ultimo (il percorso) prende tutto il
/// resto, spazi compresi.
fn split_fields<'a>(rest: &'a [u8], n: usize, what: &str) -> Result<Vec<&'a [u8]>, String> {
    let fields: Vec<&[u8]> = rest.splitn(n, |&b| b == b' ').collect();
    if fields.len() != n {
        return Err(format!("{what} con {} campi invece di {n}", fields.len()));
    }
    Ok(fields)
}

fn parse_ordinary(rest: &[u8]) -> Result<StatusEntry, String> {
    let f = split_fields(rest, 8, "voce `1`")?;
    let (index, worktree) = parse_xy(f[0])?;
    Ok(StatusEntry {
        path: path(f[7])?,
        index,
        worktree,
        kind: EntryKind::Ordinary(tracked_info(&f[1..7])?),
    })
}

fn parse_renamed(rest: &[u8], original: &[u8]) -> Result<StatusEntry, String> {
    let f = split_fields(rest, 9, "voce `2`")?;
    let (index, worktree) = parse_xy(f[0])?;
    let info = tracked_info(&f[1..7])?;
    let (copy, score) = match f[7] {
        [b'R', score @ ..] => (false, score),
        [b'C', score @ ..] => (true, score),
        other => {
            return Err(format!(
                "`{}` non è un punteggio di rinomina valido (atteso `R<n>` o `C<n>`)",
                show(other)
            ));
        }
    };
    let score = parse_digits(score)
        .and_then(|v| u8::try_from(v).ok())
        .filter(|&v| v <= 100)
        .ok_or_else(|| {
            format!(
                "`{}`: la somiglianza deve essere un numero da 0 a 100",
                show(f[7])
            )
        })?;
    let path = path(f[8])?;
    let original = self::path(original).map_err(|e| format!("percorso originale: {e}"))?;
    let kind = if copy {
        EntryKind::Copied {
            info,
            original,
            score,
        }
    } else {
        EntryKind::Renamed {
            info,
            original,
            score,
        }
    };
    Ok(StatusEntry {
        path,
        index,
        worktree,
        kind,
    })
}

fn parse_unmerged(rest: &[u8]) -> Result<StatusEntry, String> {
    let f = split_fields(rest, 10, "voce `u`")?;
    let (index, worktree) = parse_xy(f[0])?;
    let conflict = match f[0] {
        b"DD" => Conflict::BothDeleted,
        b"AU" => Conflict::AddedByUs,
        b"UD" => Conflict::DeletedByThem,
        b"UA" => Conflict::AddedByThem,
        b"DU" => Conflict::DeletedByUs,
        b"AA" => Conflict::BothAdded,
        b"UU" => Conflict::BothModified,
        other => {
            return Err(format!(
                "`{}` non è uno stato di conflitto (attesi DD, AU, UD, UA, DU, AA, UU)",
                show(other)
            ));
        }
    };
    Ok(StatusEntry {
        path: path(f[9])?,
        index,
        worktree,
        kind: EntryKind::Unmerged(UnmergedInfo {
            conflict,
            submodule: parse_sub(f[1])?,
            mode_base: mode(f[2])?,
            mode_ours: mode(f[3])?,
            mode_theirs: mode(f[4])?,
            mode_worktree: mode(f[5])?,
            oid_base: oid(f[6], "id della base")?,
            oid_ours: oid(f[7], "id di «nostro»")?,
            oid_theirs: oid(f[8], "id di «loro»")?,
        }),
    })
}

fn simple(rest: &[u8], change: Change, kind: EntryKind) -> Result<StatusEntry, String> {
    Ok(StatusEntry {
        path: path(rest)?,
        index: change,
        worktree: change,
        kind,
    })
}

/// `<sub> <mH> <mI> <mW> <hH> <hI>`.
fn tracked_info(f: &[&[u8]]) -> Result<TrackedInfo, String> {
    Ok(TrackedInfo {
        submodule: parse_sub(f[0])?,
        mode_head: mode(f[1])?,
        mode_index: mode(f[2])?,
        mode_worktree: mode(f[3])?,
        oid_head: oid(f[4], "id in HEAD")?,
        oid_index: oid(f[5], "id nell'indice")?,
    })
}

fn parse_xy(xy: &[u8]) -> Result<(Change, Change), String> {
    let bad = || {
        format!(
            "`{}` non è un codice XY valido (due lettere tra `.MTADRCU`)",
            show(xy)
        )
    };
    let change = |c: u8| {
        Some(match c {
            b'.' => Change::Unmodified,
            b'M' => Change::Modified,
            b'T' => Change::TypeChanged,
            b'A' => Change::Added,
            b'D' => Change::Deleted,
            b'R' => Change::Renamed,
            b'C' => Change::Copied,
            b'U' => Change::Unmerged,
            _ => return None,
        })
    };
    match xy {
        [x, y] => Ok((change(*x).ok_or_else(bad)?, change(*y).ok_or_else(bad)?)),
        _ => Err(bad()),
    }
}

/// `N...` oppure `S<c><m><u>`.
fn parse_sub(sub: &[u8]) -> Result<Option<SubmoduleStatus>, String> {
    let flag = |c: u8, yes: u8| match c {
        b'.' => Some(false),
        c if c == yes => Some(true),
        _ => None,
    };
    let parsed = match sub {
        b"N..." => return Ok(None),
        [b'S', c, m, u] => (|| {
            Some(SubmoduleStatus {
                commit_changed: flag(*c, b'C')?,
                modified: flag(*m, b'M')?,
                untracked: flag(*u, b'U')?,
            })
        })(),
        _ => None,
    };
    parsed.map(Some).ok_or_else(|| {
        format!(
            "`{}` non è uno stato di sottomodulo valido (atteso `N...` o `S<C|.><M|.><U|.>`)",
            show(sub)
        )
    })
}

/// Modo in ottale, es. `100644`.
pub(super) fn mode(m: &[u8]) -> Result<u32, String> {
    let bad = || format!("`{}` non è un modo ottale valido", show(m));
    if m.is_empty() || m.len() > 7 {
        return Err(bad());
    }
    m.iter().try_fold(0u32, |acc, &b| match b {
        b'0'..=b'7' => Ok(acc * 8 + u32::from(b - b'0')),
        _ => Err(bad()),
    })
}

pub(super) fn oid(hex: &[u8], what: &str) -> Result<Oid, String> {
    Oid::from_hex(hex).map_err(|e| match e {
        DomainError::InvalidOid { input, reason } => {
            format!("{what}: `{input}` non è un id valido ({reason})")
        }
        other => format!("{what}: {other}"),
    })
}

fn path(p: &[u8]) -> Result<RepoPath, String> {
    if p.is_empty() {
        return Err("percorso vuoto".to_owned());
    }
    Ok(RepoPath::from(p))
}

pub(super) fn show(s: &[u8]) -> String {
    String::from_utf8_lossy(s).into_owned()
}

fn invalid(reason: String) -> DomainError {
    DomainError::InvalidGitOutput {
        command: "status",
        reason,
    }
}
