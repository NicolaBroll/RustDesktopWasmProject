//! Stringhe "C-quoted" di git: i nomi di file nelle intestazioni dei diff.
//!
//! git mette un nome tra virgolette, con escape in stile C, se contiene `"`, `\`, un carattere
//! di controllo (`0x00`–`0x1f`, `0x7f`) e, con `core.quotePath=true` (predefinito), un byte non
//! ASCII. Gli escape sono `\a \b \t \n \v \f \r \" \\` e, per gli altri byte, tre cifre ottali
//! (`\377`). Lo spazio non richiede le virgolette.

/// Vero se git metterebbe `name` tra virgolette con `core.quotePath=false`.
pub(super) fn needs_quoting(name: &[u8]) -> bool {
    name.iter()
        .any(|&b| b == b'"' || b == b'\\' || b < 0x20 || b == 0x7f)
}

/// Scrive `prefix` seguito da `name` come fa git con `core.quotePath=false`: così com'è se non
/// serve, altrimenti tutto tra virgolette (prefisso compreso, es. `"a/con\ttab"`).
pub(super) fn write_quoted(out: &mut Vec<u8>, prefix: &[u8], name: &[u8]) {
    if !needs_quoting(prefix) && !needs_quoting(name) {
        out.extend_from_slice(prefix);
        out.extend_from_slice(name);
        return;
    }
    out.push(b'"');
    for &b in prefix.iter().chain(name) {
        let named = match b {
            0x07 => Some(b'a'),
            0x08 => Some(b'b'),
            b'\t' => Some(b't'),
            b'\n' => Some(b'n'),
            0x0b => Some(b'v'),
            0x0c => Some(b'f'),
            b'\r' => Some(b'r'),
            b'"' => Some(b'"'),
            b'\\' => Some(b'\\'),
            _ => None,
        };
        match named {
            Some(c) => out.extend_from_slice(&[b'\\', c]),
            None if b < 0x20 || b == 0x7f => {
                out.extend_from_slice(&[
                    b'\\',
                    b'0' + (b >> 6),
                    b'0' + ((b >> 3) & 7),
                    b'0' + (b & 7),
                ]);
            }
            None => out.push(b),
        }
    }
    out.push(b'"');
}

/// Legge una stringa tra virgolette all'inizio di `input` (che deve iniziare con `"`).
/// Restituisce i byte decodificati e quanti byte di `input` ha consumato (virgolette comprese).
pub(super) fn unquote_prefix(input: &[u8]) -> Result<(Vec<u8>, usize), String> {
    let show = || String::from_utf8_lossy(input).into_owned();
    let Some(body) = input.strip_prefix(b"\"") else {
        return Err(format!("`{}` non inizia con `\"`", show()));
    };
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while let Some(&b) = body.get(i) {
        match b {
            b'"' => return Ok((out, i + 2)),
            b'\\' => {
                let Some(&e) = body.get(i + 1) else { break };
                let byte = match e {
                    b'a' => 0x07,
                    b'b' => 0x08,
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'v' => 0x0b,
                    b'f' => 0x0c,
                    b'r' => b'\r',
                    b'"' => b'"',
                    b'\\' => b'\\',
                    b'0'..=b'3' => {
                        let digits = body.get(i + 1..i + 4).unwrap_or_default();
                        let value = match digits {
                            [x, y, z] if [x, y, z].iter().all(|d| (b'0'..=b'7').contains(d)) => {
                                ((x - b'0') << 6) | ((y - b'0') << 3) | (z - b'0')
                            }
                            _ => {
                                return Err(format!(
                                    "escape ottale non valido in `{}`: servono tre cifre da 0 a 7",
                                    show()
                                ));
                            }
                        };
                        out.push(value);
                        i += 4;
                        continue;
                    }
                    other => {
                        return Err(format!(
                            "escape `\\{}` sconosciuto in `{}`",
                            char::from(other),
                            show()
                        ));
                    }
                };
                out.push(byte);
                i += 2;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    Err(format!("virgolette non chiuse in `{}`", show()))
}

/// Legge un nome che occupa tutto `input`: tra virgolette (e allora non deve seguire altro) o
/// così com'è.
pub(super) fn unquote(input: &[u8]) -> Result<Vec<u8>, String> {
    if !input.starts_with(b"\"") {
        return Ok(input.to_vec());
    }
    let (name, used) = unquote_prefix(input)?;
    if used != input.len() {
        return Err(format!(
            "testo dopo le virgolette chiuse in `{}`",
            String::from_utf8_lossy(input)
        ));
    }
    Ok(name)
}
