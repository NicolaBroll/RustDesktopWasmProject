//! Testo per la UI: date dei commit.

use project_domain::Signature;

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
