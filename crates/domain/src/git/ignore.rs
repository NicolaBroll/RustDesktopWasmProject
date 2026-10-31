//! Patterns for `.gitignore` and `.git/info/exclude` (`worktree.ignore`,
//! `impl-docs/git/ignore.md`): building the escaped pattern for a path, checking a pattern given by
//! the caller and appending patterns to the bytes of an ignore file. Pure functions on bytes.

use super::RepoPath;
use super::path::{deserialize_bytes, serialize_bytes};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::fmt;

/// One line of an ignore file, as bytes (a pattern built from a non-UTF-8 path is not UTF-8).
///
/// In JSON it is a string if the bytes are valid UTF-8, otherwise a byte array, as
/// [`RepoPath`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct IgnorePattern(Vec<u8>);

impl IgnorePattern {
    /// The pattern with these exact bytes (not checked: see [`check_ignore_pattern`]).
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    /// The exact bytes of the pattern, as they go in the file.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Display for IgnorePattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.0))
    }
}

impl From<&str> for IgnorePattern {
    fn from(s: &str) -> Self {
        Self(s.as_bytes().to_vec())
    }
}

impl Serialize for IgnorePattern {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_bytes(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for IgnorePattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_bytes(deserializer, "a pattern as a string or as a byte array").map(Self)
    }
}

impl JsonSchema for IgnorePattern {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "IgnorePattern".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A gitignore pattern (one line); a byte array if it is not UTF-8",
            "anyOf": [
                { "type": "string" },
                { "type": "array", "items": { "type": "integer", "minimum": 0, "maximum": 255 } },
            ],
        })
    }
}

/// What [`ignore_pattern`] builds for a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IgnoreKind {
    /// Exactly this file (or folder): `/dir/name`.
    Path,
    /// The folder `path` and everything under it: `/dir/`.
    Folder,
    /// Every file with the extension of `path`, anywhere: `*.log`.
    Extension,
}

/// The pattern, for a root ignore file (`.gitignore` at the top or `.git/info/exclude`), that
/// matches what `kind` says for `path`:
///
/// - [`IgnoreKind::Path`]: `/` + the path, anchored to the root;
/// - [`IgnoreKind::Folder`]: `/` + the path + `/` (the path is the folder);
/// - [`IgnoreKind::Extension`]: `*.` + the extension of the file name (the bytes after its last
///   `.`); `None` without an extension (no `.`, a name like `.env` or ending in `.`).
///
/// The special bytes of gitignore (`\ * ? [`, and `!`/`#`, which only matter at the start)
/// are escaped with `\`, and so are trailing spaces (otherwise git drops them). `None` for an
/// empty path or one with a line break (it cannot be one line).
pub fn ignore_pattern(path: &RepoPath, kind: IgnoreKind) -> Option<IgnorePattern> {
    let bytes = path.as_bytes();
    if bytes.is_empty() || bytes.iter().any(|b| matches!(b, b'\n' | b'\r' | 0)) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() + 4);
    match kind {
        IgnoreKind::Path | IgnoreKind::Folder => {
            out.push(b'/');
            escape_into(bytes, &mut out);
            if kind == IgnoreKind::Folder {
                out.push(b'/');
            }
        }
        IgnoreKind::Extension => {
            let name = path.file_name();
            let dot = name.iter().rposition(|&b| b == b'.')?;
            let ext = &name[dot + 1..];
            if dot == 0 || ext.is_empty() {
                return None;
            }
            out.extend_from_slice(b"*.");
            escape_into(ext, &mut out);
        }
    }
    Some(IgnorePattern(out))
}

/// Appends `bytes` to `out` escaping the gitignore specials and the trailing spaces.
fn escape_into(bytes: &[u8], out: &mut Vec<u8>) {
    let trailing = bytes.iter().rev().take_while(|&&b| b == b' ').count();
    for (i, &b) in bytes.iter().enumerate() {
        let trailing_space = b == b' ' && i >= bytes.len() - trailing;
        if matches!(b, b'\\' | b'*' | b'?' | b'[' | b'!' | b'#') || trailing_space {
            out.push(b'\\');
        }
        out.push(b);
    }
}

/// Why a pattern cannot be appended as one line ([`check_ignore_pattern`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternProblem {
    Empty,
    /// It has a NUL, CR or LF.
    LineBreak,
    /// Only spaces and tabs.
    Blank,
    /// It starts with `#`: git would read a comment (escape it as `\#`).
    Comment,
}

/// Checks that `pattern` is one meaningful line of an ignore file.
pub fn check_ignore_pattern(pattern: &[u8]) -> Result<(), PatternProblem> {
    if pattern.is_empty() {
        return Err(PatternProblem::Empty);
    }
    if pattern.iter().any(|b| matches!(b, b'\n' | b'\r' | 0)) {
        return Err(PatternProblem::LineBreak);
    }
    if pattern.iter().all(|b| matches!(b, b' ' | b'\t')) {
        return Err(PatternProblem::Blank);
    }
    if pattern[0] == b'#' {
        return Err(PatternProblem::Comment);
    }
    Ok(())
}

/// The result of [`append_ignore_patterns`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreAppend {
    /// The whole new content of the file (equal to the old one if `added` is empty).
    pub content: Vec<u8>,
    /// The patterns appended, in the given order.
    pub added: Vec<IgnorePattern>,
    /// The patterns already present as a line (or repeated in the request): not appended.
    pub already: Vec<IgnorePattern>,
}

/// Appends `patterns` (already checked) to the content of an ignore file (`None`: the file does
/// not exist). A pattern that is already a line of the file (compared without the line's final
/// `\r` and, on the first line, a UTF-8 BOM) is not appended again, unless a negation (`!...`)
/// follows it, nor is a repetition in `patterns`. The lines end like the
/// file's first line (`\r\n` if it ends so, `\n` otherwise or for a new file); if the file
/// does not end with a newline, one is added first.
pub fn append_ignore_patterns(existing: Option<&[u8]>, patterns: &[IgnorePattern]) -> IgnoreAppend {
    let old = existing.unwrap_or_default();
    let mut lines: Vec<&[u8]> = old
        .split(|&b| b == b'\n')
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
        .collect();
    // A UTF-8 BOM is not part of the first pattern.
    if let Some(first) = lines.first_mut() {
        *first = first.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(first);
    }
    let eol: &[u8] = match old.iter().position(|&b| b == b'\n') {
        Some(i) if i > 0 && old[i - 1] == b'\r' => b"\r\n",
        _ => b"\n",
    };
    let mut content = old.to_vec();
    let mut added = Vec::new();
    let mut already = Vec::new();
    for pattern in patterns {
        // Already there only if no negation follows its last occurrence: a later `!...` line
        // may re-include what it matches, and a later line wins, so it is appended again.
        let last = lines.iter().rposition(|l| *l == pattern.as_bytes());
        if last.is_some_and(|i| !lines[i + 1..].iter().any(|l| l.starts_with(b"!"))) {
            already.push(pattern.clone());
            continue;
        }
        if !content.is_empty() && !content.ends_with(b"\n") {
            content.extend_from_slice(eol);
        }
        content.extend_from_slice(pattern.as_bytes());
        content.extend_from_slice(eol);
        lines.push(pattern.as_bytes());
        added.push(pattern.clone());
    }
    IgnoreAppend {
        content,
        added,
        already,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pat(path: &[u8], kind: IgnoreKind) -> Option<Vec<u8>> {
        ignore_pattern(&RepoPath::from_bytes(path), kind).map(|p| p.0)
    }

    #[test]
    fn patterns_for_paths() {
        use IgnoreKind::*;
        assert_eq!(pat(b"build/out.log", Path).unwrap(), b"/build/out.log");
        assert_eq!(pat(b"build", Folder).unwrap(), b"/build/");
        assert_eq!(pat(b"a/b/out.log", Extension).unwrap(), b"*.log");
        assert_eq!(pat(b"a/x.tar.gz", Extension).unwrap(), b"*.gz");
        assert_eq!(
            pat(b"#a !b *c? [d] e\\f  ", Path).unwrap(),
            b"/\\#a \\!b \\*c\\? \\[d] e\\\\f\\ \\ "
        );
        assert_eq!(pat(b"x.l*g", Extension).unwrap(), b"*.l\\*g");
        assert_eq!(pat(b"d/\xff.bin", Path).unwrap(), b"/d/\xff.bin");
        assert_eq!(pat(b"-x", Path).unwrap(), b"/-x");
        for (path, kind) in [
            (&b""[..], Path),
            (b"a\nb", Path),
            (b"Makefile", Extension),
            (b"d/.env", Extension),
            (b"name.", Extension),
        ] {
            assert_eq!(pat(path, kind), None, "{path:?}");
        }
    }

    #[test]
    fn pattern_checks() {
        assert_eq!(check_ignore_pattern(b"*.log"), Ok(()));
        assert_eq!(check_ignore_pattern(b"\\#x"), Ok(()));
        assert_eq!(check_ignore_pattern(b"!keep"), Ok(()));
        assert_eq!(check_ignore_pattern(b""), Err(PatternProblem::Empty));
        assert_eq!(check_ignore_pattern(b" \t"), Err(PatternProblem::Blank));
        assert_eq!(check_ignore_pattern(b"#x"), Err(PatternProblem::Comment));
        for bad in [&b"a\nb"[..], b"a\r", b"a\0"] {
            assert_eq!(check_ignore_pattern(bad), Err(PatternProblem::LineBreak));
        }
    }

    #[test]
    fn appending() {
        let p = |s: &str| IgnorePattern::from(s);
        let new = append_ignore_patterns(None, &[p("a"), p("b"), p("a")]);
        assert_eq!(new.content, b"a\nb\n");
        assert_eq!(new.added, [p("a"), p("b")]);
        assert_eq!(new.already, [p("a")]);

        let out = append_ignore_patterns(Some(b"x\ny"), &[p("y"), p("z")]);
        assert_eq!(out.content, b"x\ny\nz\n");
        assert_eq!(out.added, [p("z")]);
        assert_eq!(out.already, [p("y")]);

        let crlf = append_ignore_patterns(Some(b"x\r\ny"), &[p("x"), p("z")]);
        assert_eq!(crlf.content, b"x\r\ny\r\nz\r\n");
        assert_eq!(crlf.already, [p("x")]);

        let none = append_ignore_patterns(Some(b"a\n"), &[p("a")]);
        assert_eq!(none.content, b"a\n");
        assert!(none.added.is_empty());

        let empty = append_ignore_patterns(Some(b""), &[p("a")]);
        assert_eq!(empty.content, b"a\n");

        let bom = append_ignore_patterns(Some(b"\xEF\xBB\xBFa\n"), &[p("a")]);
        assert!(bom.added.is_empty());
        assert_eq!(bom.content, b"\xEF\xBB\xBFa\n");

        // A later negation may re-include it: appended again, so it wins.
        let negated = append_ignore_patterns(Some(b"/build\n!/build\n"), &[p("/build")]);
        assert_eq!(negated.added, [p("/build")]);
        assert_eq!(negated.content, b"/build\n!/build\n/build\n");
    }

    #[test]
    fn json_of_a_pattern() {
        let p = IgnorePattern::from_bytes(b"/\xff".to_vec());
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v, serde_json::json!([47, 255]));
        assert_eq!(serde_json::from_value::<IgnorePattern>(v).unwrap(), p);
        assert_eq!(
            serde_json::to_value(IgnoreKind::Extension).unwrap(),
            "extension"
        );
    }
}
