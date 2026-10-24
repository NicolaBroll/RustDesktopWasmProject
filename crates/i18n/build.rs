//! Generates the list of `.ftl` files (`locales/<language>/<area>.ftl`), included with
//! `include_str!` so the translations live in the binary (wasm too). A new area is just a new
//! file: no list to edit by hand, no conflicts between people working on different areas.

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut out = String::new();
    for lang in ["it", "en"] {
        let dir = root.join(lang);
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut areas: Vec<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .filter_map(|entry| {
                let path = entry.expect("directory entry").path();
                (path.extension()? == "ftl")
                    .then(|| path.file_stem()?.to_str().map(str::to_owned))?
            })
            .collect();
        areas.sort();
        writeln!(
            out,
            "pub(crate) const {}: &[(&str, &str)] = &[",
            lang.to_uppercase()
        )
        .unwrap();
        for area in areas {
            let file = dir.join(format!("{area}.ftl"));
            println!("cargo:rerun-if-changed={}", file.display());
            writeln!(
                out,
                "    ({area:?}, include_str!({:?})),",
                file.display().to_string()
            )
            .unwrap();
        }
        writeln!(out, "];").unwrap();
    }
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("areas.rs");
    std::fs::write(dest, out).unwrap();
}
