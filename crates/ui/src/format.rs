//! Testo per la UI: date dei commit, file cambiati.

use project_domain::{Change, ChangedFile, Signature};
use project_i18n::tr;

/// Lettera del tipo di modifica, come la scrive git: `A`, `D`, `M`, `T`, `R`, `C`.
pub fn change_letter(change: Change) -> &'static str {
    match change {
        Change::Added => "A",
        Change::Deleted => "D",
        Change::Modified => "M",
        Change::TypeChanged => "T",
        Change::Renamed => "R",
        Change::Copied => "C",
        Change::Unmerged => "U",
        Change::Untracked => "?",
        Change::Ignored => "!",
        Change::Unmodified => ".",
    }
}

/// Nome del tipo di modifica, per il suggerimento.
pub fn change_name(change: Change) -> String {
    match change {
        Change::Added => tr!("format-change-added"),
        Change::Deleted => tr!("format-change-deleted"),
        Change::Modified => tr!("format-change-modified"),
        Change::TypeChanged => tr!("format-change-type-changed"),
        Change::Renamed => tr!("format-change-renamed"),
        Change::Copied => tr!("format-change-copied"),
        Change::Unmerged => tr!("format-change-unmerged"),
        Change::Untracked => tr!("format-change-untracked"),
        Change::Ignored => tr!("format-change-ignored"),
        Change::Unmodified => tr!("format-change-unmodified"),
    }
}

/// Percorso da mostrare: `vecchio -> nuovo` (ASCII: i font di default non hanno le frecce) per rinomine e copie, altrimenti il percorso.
pub fn file_path(file: &ChangedFile) -> String {
    match &file.original {
        Some(original) => format!("{original} -> {}", file.path),
        None => file.path.to_string(),
    }
}

/// Somiglianza di rinomine e copie, es. `92%`; vuoto per gli altri file.
pub fn file_score(file: &ChangedFile) -> String {
    file.score.map(|s| format!("{s}%")).unwrap_or_default()
}

/// Data e ora di una firma nel fuso orario di chi ha firmato, es. `2023-11-14 22:13`.
pub fn date_time(sig: &Signature) -> String {
    let local = sig.time + i64::from(sig.offset_min) * 60;
    let (days, secs) = (local.div_euclid(86_400), local.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// Come [`date_time`] con il fuso, es. `2023-11-14 22:13 +0200`.
pub fn date_time_zone(sig: &Signature) -> String {
    let sign = if sig.offset_min < 0 { '-' } else { '+' };
    let off = sig.offset_min.unsigned_abs();
    format!("{} {sign}{:02}{:02}", date_time(sig), off / 60, off % 60)
}

/// Giorni dall'epoca Unix → (anno, mese, giorno) nel calendario gregoriano
/// (algoritmo `civil_from_days` di Howard Hinnant).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(time: i64, offset_min: i32) -> Signature {
        Signature {
            name: String::new(),
            email: String::new(),
            time,
            offset_min,
        }
    }

    fn file(change: Change, path: &str, original: Option<&str>, score: Option<u8>) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            change,
            original: original.map(Into::into),
            score,
            old_mode: 0o100644,
            new_mode: 0o100644,
            old_oid: project_domain::Oid::from_bytes([1; 20]),
            new_oid: project_domain::Oid::from_bytes([2; 20]),
        }
    }

    #[test]
    fn changed_file_text() {
        let renamed = file(
            Change::Renamed,
            "src/bin/app.rs",
            Some("src/main.rs"),
            Some(92),
        );
        assert_eq!(change_letter(renamed.change), "R");
        assert_eq!(file_path(&renamed), "src/main.rs -> src/bin/app.rs");
        assert_eq!(file_score(&renamed), "92%");

        let modified = file(Change::Modified, "README.md", None, None);
        assert_eq!(change_letter(modified.change), "M");
        assert_eq!(file_path(&modified), "README.md");
        assert_eq!(file_score(&modified), "");

        // Un percorso non UTF-8 si mostra con U+FFFD.
        let mut odd = file(Change::Added, "", None, None);
        odd.path = project_domain::RepoPath::from_bytes(b"ca\xff".to_vec());
        assert_eq!(file_path(&odd), "ca\u{fffd}");
        let letters: String = [
            Change::Added,
            Change::Deleted,
            Change::Modified,
            Change::TypeChanged,
            Change::Copied,
        ]
        .map(change_letter)
        .concat();
        assert_eq!(letters, "ADMTC");
    }

    #[test]
    fn dates_in_the_signer_zone() {
        assert_eq!(date_time(&sig(0, 0)), "1970-01-01 00:00");
        assert_eq!(date_time(&sig(1_700_000_000, 0)), "2023-11-14 22:13");
        assert_eq!(
            date_time_zone(&sig(1_700_000_000, 120)),
            "2023-11-15 00:13 +0200"
        );
        assert_eq!(
            date_time_zone(&sig(1_700_000_000, -330)),
            "2023-11-14 16:43 -0530"
        );
        // 29 febbraio di un anno bisestile, e prima del 1970.
        assert_eq!(date_time(&sig(951_782_400, 0)), "2000-02-29 00:00");
        assert_eq!(date_time(&sig(-1, 0)), "1969-12-31 23:59");
    }
}
