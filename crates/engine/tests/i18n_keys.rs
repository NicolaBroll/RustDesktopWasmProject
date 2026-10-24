//! Every error message of the engine and the backends exists in both languages, with the right
//! parameters. Messages always go through the `msg!` macro with the literal key
//! (`msg!("engine-no-repo")`, `msg!("engine-stash-gone", command = c, id = id)`): here they are
//! read from the sources of `crates/engine/src` and `crates/git/src` and compared with the
//! `.ftl` files of `crates/i18n/locales/{it,en}/`.

use fluent_syntax::ast::{Entry, Expression, InlineExpression, Pattern, PatternElement};
use project_engine::Lang;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A `msg!` call found in the sources.
struct Call {
    place: String,
    key: Result<String, String>,
    args: BTreeSet<String>,
}

/// The `msg!(` calls in the text (comment lines are skipped).
fn calls(name: &str, source: &str) -> Vec<Call> {
    let mut found = Vec::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let code = line.trim_start();
        if !code.starts_with("//") {
            for (start, _) in line.match_indices("msg!(") {
                let at = offset + start;
                let lineno = source[..at].matches('\n').count() + 1;
                let rest = source[at + 5..].trim_start();
                let place = format!("{name}:{lineno}");
                let Some(after) = rest.strip_prefix('"') else {
                    let text = rest.chars().take(30).collect();
                    found.push(Call {
                        place,
                        key: Err(text),
                        args: BTreeSet::new(),
                    });
                    continue;
                };
                let (key, tail) = after.split_once('"').expect("closed key");
                found.push(Call {
                    place,
                    key: Ok(key.to_owned()),
                    args: arg_names(tail),
                });
            }
        }
        offset += line.len();
    }
    found
}

/// The parameter names after the key: `, a = x, b = f(c, d))` → `{a, b}`.
fn arg_names(tail: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut depth = 0usize;
    let mut piece = String::new();
    let mut chars = tail.chars();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => in_string = false,
                _ => {}
            }
            piece.push(c);
            continue;
        }
        match c {
            '"' => in_string = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => break,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                if let Some((name, _)) = piece.split_once('=') {
                    names.insert(name.trim().to_owned());
                }
                piece.clear();
                continue;
            }
            _ => {}
        }
        piece.push(c);
    }
    if let Some((name, _)) = piece.split_once('=') {
        names.insert(name.trim().to_owned());
    }
    names.remove("");
    names
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The `$name` variables of a pattern, also in selectors and variants.
fn variables(pattern: &Pattern<String>, out: &mut BTreeSet<String>) {
    for element in &pattern.elements {
        if let PatternElement::Placeable { expression } = element {
            expression_variables(expression, out);
        }
    }
}

fn expression_variables(expression: &Expression<String>, out: &mut BTreeSet<String>) {
    match expression {
        Expression::Inline(inline) => inline_variables(inline, out),
        Expression::Select { selector, variants } => {
            inline_variables(selector, out);
            for variant in variants {
                variables(&variant.value, out);
            }
        }
    }
}

fn inline_variables(inline: &InlineExpression<String>, out: &mut BTreeSet<String>) {
    match inline {
        InlineExpression::VariableReference { id } => {
            out.insert(id.name.clone());
        }
        InlineExpression::Placeable { expression } => expression_variables(expression, out),
        _ => {}
    }
}

/// The `engine*` keys of a language with their variables.
fn messages(lang: Lang) -> BTreeMap<String, BTreeSet<String>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../i18n/locales")
        .join(lang.code());
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.starts_with("engine") || !name.ends_with(".ftl") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let resource = fluent_syntax::parser::parse(text)
            .unwrap_or_else(|(_, e)| panic!("{}: {e:?}", path.display()));
        for entry in resource.body {
            if let Entry::Message(m) = entry {
                let mut vars = BTreeSet::new();
                if let Some(value) = &m.value {
                    variables(value, &mut vars);
                }
                out.insert(m.id.name, vars);
            }
        }
    }
    out
}

#[test]
fn every_engine_message_exists_with_its_parameters() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in ["src", "../git/src"] {
        sources(&root.join(dir), &mut files);
    }
    let langs: Vec<_> = Lang::ALL.iter().map(|&l| (l, messages(l))).collect();
    let mut errors = Vec::new();
    let mut used = BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let name = file.strip_prefix(root).unwrap().display().to_string();
        for call in calls(&name, &text) {
            let key = match call.key {
                Ok(key) => key,
                Err(text) => {
                    errors.push(format!("{}: msg! without a literal: {text}", call.place));
                    continue;
                }
            };
            for (lang, messages) in &langs {
                match messages.get(&key) {
                    None => errors.push(format!(
                        "{}: `{key}` missing in {}",
                        call.place,
                        lang.code()
                    )),
                    Some(vars) => {
                        let missing: Vec<_> = vars.difference(&call.args).collect();
                        if !missing.is_empty() {
                            errors.push(format!(
                                "{}: `{key}` in {} uses {missing:?}, not passed",
                                call.place,
                                lang.code()
                            ));
                        }
                    }
                }
            }
            // A parameter passed but unused in every language is probably a wrong
            // name.
            let all: BTreeSet<&String> = langs
                .iter()
                .filter_map(|(_, m)| m.get(&key))
                .flatten()
                .collect();
            for arg in &call.args {
                if !all.contains(arg) {
                    errors.push(format!(
                        "{}: `{key}` does not use the parameter `{arg}`",
                        call.place
                    ));
                }
            }
            used.insert(key);
        }
    }
    used.insert(project_engine::INDEX_NOT_RESTORED.to_owned());
    // The keys nobody uses any more.
    for (lang, messages) in &langs {
        for key in messages.keys() {
            if !used.contains(key) {
                errors.push(format!("`{key}` ({}) is not used", lang.code()));
            }
        }
    }
    assert!(used.len() > 100, "too few keys found: {}", used.len());
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

#[test]
fn the_scanner_reads_keys_and_parameters() {
    let found = calls(
        "x.rs",
        "a(msg!(\"x-a\"), msg!(\n    \"x-b\",\n n = f(1, 2), s = \"a,b\"), msg!(k))\n// msg!(\"x-c\")\n",
    );
    assert_eq!(found.len(), 3);
    assert_eq!(found[0].key, Ok("x-a".to_owned()));
    assert!(found[0].args.is_empty());
    assert_eq!(found[1].key, Ok("x-b".to_owned()));
    assert_eq!(
        found[1].args,
        ["n", "s"].into_iter().map(str::to_owned).collect()
    );
    assert!(found[2].key.is_err());
}
