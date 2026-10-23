//! Ogni chiave usata nel codice della UI esiste in tutte le lingue. Le chiavi passano sempre
//! dalla macro `tr!` con un letterale (`tr!("merge-close")`): qui si leggono dai sorgenti.

use project_i18n::Lang;
use std::path::Path;

/// Le chiavi di `tr!("…"` nel testo, con la riga; un `tr!(` senza letterale è un errore.
fn keys(source: &str) -> Vec<(usize, Result<String, String>)> {
    let mut found = Vec::new();
    for (start, _) in source.match_indices("tr!(") {
        let line = source[..start].matches('\n').count() + 1;
        let rest = source[start + 4..].trim_start();
        let key = rest
            .strip_prefix('"')
            .and_then(|r| r.split_once('"'))
            .map(|(key, _)| key.to_owned())
            .ok_or_else(|| rest.chars().take(30).collect());
        found.push((line, key));
    }
    found
}

fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn every_key_used_by_the_ui_exists() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    let mut errors = Vec::new();
    let mut used = 0;
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let name = file.strip_prefix(&src).unwrap().display().to_string();
        for (line, key) in keys(&text) {
            match key {
                Ok(key) => {
                    used += 1;
                    for lang in Lang::ALL {
                        if !project_i18n::get(lang).has(&key) {
                            errors.push(format!("{name}:{line}: «{key}» manca in {}", lang.code()));
                        }
                    }
                }
                Err(text) => errors.push(format!("{name}:{line}: tr! senza letterale: {text}")),
            }
        }
    }
    assert!(used > 0, "nessuna chiave trovata");
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

#[test]
fn the_scanner_reads_keys() {
    let keys = keys("a(tr!(\"x-a\"), tr!(\n    \"x-b\",\n n = 1), tr!(k))");
    let keys: Vec<_> = keys.into_iter().map(|(_, k)| k).collect();
    assert_eq!(keys[0], Ok("x-a".to_owned()));
    assert_eq!(keys[1], Ok("x-b".to_owned()));
    assert!(keys[2].is_err());
}
