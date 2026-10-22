//! Stage e unstage di singole righe (`index.stage_lines`, `index.unstage_lines`): la parte
//! comune ai backend.
//!
//! Il backend rilegge il diff del file ([`LineSelection::diff_spec`]), controlla che sia quello
//! che ha visto chi ha scelto le righe e costruisce la patch con [`line_patch`]; poi la applica
//! all'indice (il `CliBackend` con `git apply` e `APPLY_ARGS`, il `MemoryBackend` con
//! [`apply_patch`]).
//!
//! # Il diff visto
//!
//! I numeri di riga di una selezione valgono solo per il diff da cui sono stati presi: se il
//! file è cambiato nel frattempo (nel worktree o nell'indice) gli stessi numeri possono
//! indicare righe diverse, e la patch sposterebbe nell'indice righe che l'utente non ha mai
//! visto. Per questo la selezione porta il [`file_digest`] del `FileDiff` visto: i comandi
//! `diff.worktree` e `diff.index` lo restituiscono per ogni file (campo `digest`), e il backend,
//! dopo aver riletto il diff, lo ricalcola. Se non coincide: [`EngineError::StaleDiff`] e
//! niente scritto.
//!
//! Il digest copre tutto il `FileDiff` (percorsi, modi, id, hunk con numeri e contenuto delle
//! righe), non solo gli id: nel diff del worktree l'id del lato nuovo lo calcola git dal file,
//! ma nel `MemoryBackend` è finto, e un digest dell'intero diff vale per tutti i backend allo
//! stesso modo. Il parametro è obbligatorio: senza, una selezione fatta su un diff vecchio
//! verrebbe applicata a righe diverse senza che nessuno se ne accorga, e chi sceglie le righe
//! ha comunque appena letto il diff (i numeri vengono da lì).

use super::DiffSpec;
use crate::EngineError;
use project_domain::{
    Change, Diff, DomainError, FileDiff, LineKind, LineRef, PatchDirection, RepoPath, build_patch,
};

/// Le righe da mettere in stage o da togliere dallo stage: parametri di `index.stage_lines` e
/// `index.unstage_lines`, già validati.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSelection {
    /// Il file, con il percorso usato per chiederne il diff (il nuovo; per una rinomina anche il
    /// vecchio).
    pub path: RepoPath,
    /// Le righe scelte, con i numeri del diff visto (vedi [`LineRef`]). Non vuota.
    pub lines: Vec<LineRef>,
    /// Il [`file_digest`] del `FileDiff` da cui vengono le righe.
    pub digest: String,
    /// [`PatchDirection::Stage`]: righe di `diff.worktree`; [`PatchDirection::Unstage`]: righe
    /// di `diff.index`.
    pub direction: PatchDirection,
}

impl LineSelection {
    /// L'id del comando: `index.stage_lines` o `index.unstage_lines`.
    pub fn command_id(&self) -> &'static str {
        match self.direction {
            PatchDirection::Stage => "index.stage_lines",
            PatchDirection::Unstage => "index.unstage_lines",
        }
    }

    /// Il diff da cui vengono le righe, limitato al file.
    pub fn diff_spec(&self) -> DiffSpec {
        let path = Some(self.path.clone());
        match self.direction {
            PatchDirection::Stage => DiffSpec::Worktree { path },
            PatchDirection::Unstage => DiffSpec::Index { path },
        }
    }
}

/// Identifica un [`FileDiff`]: 32 cifre esadecimali, uguali solo per diff uguali in ogni campo
/// (in pratica: è un hash FNV-1a a 128 bit della serializzazione JSON). Lo restituiscono
/// `diff.worktree` e `diff.index` per ogni file (`digest`); un `FileDiff` riletto dal JSON di
/// quei comandi dà lo stesso digest. Non è un hash crittografico: serve a riconoscere un diff
/// cambiato, non a difendersi da chi costruisce apposta due diff con lo stesso digest.
pub fn file_digest(file: &FileDiff) -> String {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let json = serde_json::to_vec(file).expect("i tipi del dominio sono serializzabili");
    let hash = json
        .iter()
        .fold(OFFSET, |h, &b| (h ^ u128::from(b)).wrapping_mul(PRIME));
    format!("{hash:032x}")
}

/// Il file scelto e la patch da applicare all'indice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinePatch {
    /// Il file del diff riletto, quello visto da chi ha scelto le righe.
    pub file: FileDiff,
    /// La patch di [`build_patch`], da applicare all'indice.
    pub patch: FileDiff,
}

impl LinePatch {
    /// Il percorso che la patch cambia nell'indice: il lato indice del diff (il vecchio per lo
    /// stage, il nuovo per l'unstage).
    pub fn target(&self) -> &RepoPath {
        &self.patch.new_path
    }

    /// I percorsi toccati, per il backup e il ricontrollo dell'indice: quello richiesto e i due
    /// del file (diversi per una rinomina).
    pub fn touched(&self, selection: &LineSelection) -> Vec<RepoPath> {
        let mut paths = vec![selection.path.clone()];
        for p in [
            &self.file.old_path,
            &self.file.new_path,
            &self.patch.new_path,
        ] {
            if !paths.contains(p) {
                paths.push(p.clone());
            }
        }
        paths
    }
}

/// Sceglie in `diff` (il diff di [`LineSelection::diff_spec`] appena riletto) il file della
/// selezione, controlla che sia quello visto e costruisce la patch.
///
/// - Il file è quello con il percorso nuovo uguale a `path`; se non c'è, quello con il percorso
///   vecchio (una rinomina chiesta con il vecchio nome). Due file sullo stesso percorso (cambio
///   di tipo, es. un file diventato link: git lo scrive come eliminazione più file nuovo):
///   [`EngineError::NotAllowed`], si mette in stage il file intero.
/// - Nessun file (il percorso non ha più modifiche) o digest diverso:
///   [`EngineError::StaleDiff`].
/// - File non adatto a una patch per righe (binario, sottomodulo, link, conflitto, copia,
///   rinomina nello stage): [`EngineError::NotAllowed`].
/// - Righe che non sono righe cambiate del diff, o selezione vuota:
///   [`EngineError::InvalidParams`].
pub fn line_patch(diff: &Diff, selection: &LineSelection) -> Result<LinePatch, EngineError> {
    let id = selection.command_id();
    let path = &selection.path;
    let by_new: Vec<&FileDiff> = diff.files.iter().filter(|f| f.new_path == *path).collect();
    let candidates = if by_new.is_empty() {
        diff.files.iter().filter(|f| f.old_path == *path).collect()
    } else {
        by_new
    };
    let file = match candidates[..] {
        [file] => file,
        [] => {
            let side = match selection.direction {
                PatchDirection::Stage => "non in stage",
                PatchDirection::Unstage => "in stage",
            };
            return Err(EngineError::StaleDiff {
                path: path.to_string(),
                reason: format!("il file non ha più modifiche {side}"),
            });
        }
        _ => {
            return Err(EngineError::NotAllowed(format!(
                "`{path}` ha più modifiche sullo stesso percorso (cambio di tipo, ad esempio da \
                 file a link): le righe non si possono scegliere, si mette in stage o si toglie \
                 dallo stage il file intero"
            )));
        }
    };
    if file_digest(file) != selection.digest {
        return Err(EngineError::StaleDiff {
            path: path.to_string(),
            reason: "il diff è cambiato da quando è stato letto".to_owned(),
        });
    }
    let patch = build_patch(file, &selection.lines, selection.direction).map_err(|e| match e {
        DomainError::UnsupportedPatch { .. } => EngineError::NotAllowed(e.to_string()),
        e @ DomainError::InvalidLineSelection { .. } => EngineError::InvalidParams {
            id,
            reason: e.to_string(),
        },
        e => EngineError::Domain(e),
    })?;
    let patch = patch.ok_or_else(|| EngineError::InvalidParams {
        id,
        reason: "nessuna riga scelta: `lines` è vuoto".to_owned(),
    })?;
    Ok(LinePatch {
        file: file.clone(),
        patch,
    })
}

/// Una riga di un file con il suo a capo (l'ultima può non averlo).
fn split_lines(content: &[u8]) -> Vec<&[u8]> {
    content.split_inclusive(|&b| b == b'\n').collect()
}

/// Applica una patch di [`build_patch`] a `base` (`None` = file assente dall'indice, o
/// presente solo come segnaposto `intent-to-add`), come `git apply --cached --recount`:
/// ogni hunk si applica alla sua posizione (`old_start`) e le righe di contesto e tolte devono
/// coincidere byte per byte, a capo compreso. Risultato `None`: il file esce dall'indice.
/// Serve al `MemoryBackend`.
pub fn apply_patch(base: Option<&[u8]>, patch: &FileDiff) -> Result<Option<Vec<u8>>, String> {
    let base = match (base, patch.change) {
        (Some(_), Change::Added) => return Err("file nuovo, ma esiste già".into()),
        (None, Change::Added) => &[][..],
        (None, _) => return Err("il file non esiste nell'indice".into()),
        (Some(b), _) => b,
    };
    let src = split_lines(base);
    let mut out: Vec<u8> = Vec::with_capacity(base.len());
    let mut pos = 0usize;
    for h in &patch.hunks {
        let start = h.old_start as usize;
        let first = if h.old_count == 0 {
            start
        } else {
            start.checked_sub(1).ok_or("hunk alla riga 0")?
        };
        if first < pos || first > src.len() {
            return Err(format!("hunk fuori posto alla riga {start}"));
        }
        for l in &src[pos..first] {
            out.extend_from_slice(l);
        }
        pos = first;
        for l in &h.lines {
            let mut bytes = l.content.as_bytes().to_vec();
            if !l.no_newline {
                bytes.push(b'\n');
            }
            if l.kind != LineKind::Added {
                if src.get(pos).copied() != Some(&bytes[..]) {
                    return Err(format!("contenuto diverso alla riga {}", pos + 1));
                }
                pos += 1;
            }
            if l.kind != LineKind::Removed {
                if out.last().is_some_and(|&b| b != b'\n') {
                    return Err("riga senza a capo seguita da un'altra riga".into());
                }
                out.extend_from_slice(&bytes);
            }
        }
    }
    for l in &src[pos..] {
        if out.last().is_some_and(|&b| b != b'\n') {
            return Err("riga senza a capo seguita da un'altra riga".into());
        }
        out.extend_from_slice(l);
    }
    if patch.change == Change::Deleted {
        if !out.is_empty() {
            return Err("l'eliminazione lascerebbe del contenuto".into());
        }
        return Ok(None);
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::parse_diff;

    fn diff(text: &str) -> Diff {
        parse_diff(text.as_bytes()).unwrap()
    }

    const MODIFIED: &str = "diff --git a/f b/f\n\
        index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n\
        --- a/f\n\
        +++ b/f\n\
        @@ -1,3 +1,3 @@\n\
        \x20a\n\
        -b\n\
        +B\n\
        \x20c\n";

    fn selection(diff: &Diff, lines: Vec<LineRef>, direction: PatchDirection) -> LineSelection {
        LineSelection {
            path: RepoPath::from("f"),
            lines,
            digest: file_digest(&diff.files[0]),
            direction,
        }
    }

    #[test]
    fn digest_identifies_the_whole_file_diff() {
        let a = diff(MODIFIED);
        let d = file_digest(&a.files[0]);
        assert_eq!(d.len(), 32);
        assert!(d.bytes().all(|b| b.is_ascii_hexdigit()));
        // Riletto dal JSON dà lo stesso digest.
        let json = serde_json::to_value(&a.files[0]).unwrap();
        let back: FileDiff = serde_json::from_value(json).unwrap();
        assert_eq!(file_digest(&back), d);
        // Basta un byte diverso in una riga.
        let b = diff(&MODIFIED.replace("+B", "+C"));
        assert_ne!(file_digest(&b.files[0]), d);
        // O un numero di riga.
        let c = diff(&MODIFIED.replace("@@ -1,3 +1,3 @@", "@@ -2,3 +2,3 @@"));
        assert_ne!(file_digest(&c.files[0]), d);
    }

    #[test]
    fn patch_from_the_seen_diff() {
        let d = diff(MODIFIED);
        let sel = selection(&d, vec![LineRef::New(2)], PatchDirection::Stage);
        let p = line_patch(&d, &sel).unwrap();
        assert_eq!(p.target(), &RepoPath::from("f"));
        assert_eq!(
            apply_patch(Some(b"a\nb\nc\n"), &p.patch).unwrap().unwrap(),
            b"a\nb\nB\nc\n"
        );
        assert_eq!(p.touched(&sel), [RepoPath::from("f")]);
    }

    #[test]
    fn errors() {
        let d = diff(MODIFIED);
        // Digest diverso.
        let mut sel = selection(&d, vec![LineRef::New(2)], PatchDirection::Stage);
        sel.digest = "0".repeat(32);
        let err = line_patch(&d, &sel).unwrap_err();
        assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
        // Nessun file.
        let err = line_patch(&Diff::default(), &sel).unwrap_err();
        assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
        // Riga di contesto o inesistente.
        for lines in [vec![LineRef::New(1)], vec![LineRef::Old(9)], vec![]] {
            let sel = selection(&d, lines, PatchDirection::Stage);
            let err = line_patch(&d, &sel).unwrap_err();
            assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
        }
        // Binario.
        let bin = diff(
            "diff --git a/f b/f\n\
             index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n\
             Binary files a/f and b/f differ\n",
        );
        let sel = selection(&bin, vec![LineRef::New(1)], PatchDirection::Stage);
        let err = line_patch(&bin, &sel).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        // Due file sullo stesso percorso (cambio di tipo).
        let two = diff(
            "diff --git a/f b/f\n\
             deleted file mode 100644\n\
             index 1111111111111111111111111111111111111111..0000000000000000000000000000000000000000\n\
             --- a/f\n\
             +++ /dev/null\n\
             @@ -1 +0,0 @@\n\
             -a\n\
             diff --git a/f b/f\n\
             new file mode 120000\n\
             index 0000000000000000000000000000000000000000..2222222222222222222222222222222222222222\n\
             --- /dev/null\n\
             +++ b/f\n\
             @@ -0,0 +1 @@\n\
             +x\n\
             \\ No newline at end of file\n",
        );
        let sel = selection(&two, vec![LineRef::Old(1)], PatchDirection::Stage);
        let err = line_patch(&two, &sel).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    }

    #[test]
    fn rename_by_old_path() {
        let d = diff(
            "diff --git a/vecchio b/f\n\
             similarity index 80%\n\
             rename from vecchio\n\
             rename to f\n\
             index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n\
             --- a/vecchio\n\
             +++ b/f\n\
             @@ -1,2 +1,2 @@\n\
             \x20a\n\
             -b\n\
             +B\n",
        );
        let mut sel = selection(&d, vec![LineRef::New(2)], PatchDirection::Unstage);
        sel.path = RepoPath::from("vecchio");
        let p = line_patch(&d, &sel).unwrap();
        assert_eq!(p.target(), &RepoPath::from("f"));
        assert_eq!(
            p.touched(&sel),
            [RepoPath::from("vecchio"), RepoPath::from("f")]
        );
        assert_eq!(
            apply_patch(Some(b"a\nB\n"), &p.patch).unwrap().unwrap(),
            b"a\n"
        );
    }

    #[test]
    fn apply_checks_the_preimage() {
        let d = diff(MODIFIED);
        let sel = selection(&d, vec![LineRef::Old(2)], PatchDirection::Stage);
        let p = line_patch(&d, &sel).unwrap().patch;
        assert_eq!(
            apply_patch(Some(b"a\nb\nc\n"), &p).unwrap().unwrap(),
            b"a\nc\n"
        );
        assert!(apply_patch(Some(b"a\nx\nc\n"), &p).is_err());
        assert!(apply_patch(Some(b"a\nb\r\nc\n"), &p).is_err());
        assert!(apply_patch(None, &p).is_err());
    }
}
