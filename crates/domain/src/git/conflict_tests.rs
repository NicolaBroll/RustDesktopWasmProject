use super::*;
use crate::DomainError;
use proptest::prelude::*;

// Real conflicted worktree files, generated with git 2.56.0 by `testdata/conflict.sh`
// (`sh conflict.sh testdata`): `git merge other` in a temporary repo with
// `GIT_CONFIG_GLOBAL=/dev/null`, then `git checkout --conflict=diff3|zdiff3 -- merge.txt`.

/// Two blocks, merge style, labels `HEAD` and `other`.
const MERGE: &[u8] = include_bytes!("testdata/conflict-merge.bin");
/// The same file with `--conflict=diff3`: labels `ours`, `base`, `theirs`.
const DIFF3: &[u8] = include_bytes!("testdata/conflict-diff3.bin");
/// The same with `--conflict=zdiff3`.
const ZDIFF3: &[u8] = include_bytes!("testdata/conflict-zdiff3.bin");
/// Lines with CRLF.
const CRLF: &[u8] = include_bytes!("testdata/conflict-crlf.bin");
/// Last line without a newline on both sides (git adds the newline in the block).
const TAIL: &[u8] = include_bytes!("testdata/conflict-tail.bin");
/// Our side removed the lines that theirs changes: `ours` empty.
const EMPTY: &[u8] = include_bytes!("testdata/conflict-empty.bin");
/// `conflict-marker-size=10` from `.gitattributes`.
const SIZE: &[u8] = include_bytes!("testdata/conflict-size.bin");
/// Non-UTF-8 content (latin-1).
const LATIN1: &[u8] = include_bytes!("testdata/conflict-latin1.bin");
/// The script that generates the files.
const SCRIPT: &str = include_str!("testdata/conflict.sh");

fn parse(text: &[u8]) -> ConflictText {
    parse_conflicts(text, 7).expect("valid conflict")
}

fn blocks(parsed: &ConflictText) -> Vec<&ConflictBlock> {
    parsed.blocks().collect()
}

fn common(bytes: &[u8]) -> Segment {
    Segment::Common(bytes.to_vec())
}

/// Recomposes the file: common segments and, for the blocks, the bytes of the text in their `range`.
fn rebuild(text: &[u8], parsed: &ConflictText) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = 0;
    for s in &parsed.segments {
        match s {
            Segment::Common(bytes) => {
                assert!(!bytes.is_empty(), "empty common segment");
                assert_eq!(&text[pos..pos + bytes.len()], bytes.as_slice());
                pos += bytes.len();
                out.extend_from_slice(bytes);
            }
            Segment::Conflict(b) => {
                assert_eq!(b.range.start, pos, "the block follows the previous segment");
                out.extend_from_slice(&text[b.range.clone()]);
                pos = b.range.end;
            }
        }
    }
    out
}

fn invalid(result: Result<impl std::fmt::Debug, DomainError>) -> String {
    match result {
        Err(DomainError::InvalidConflict { reason }) => reason,
        other => panic!("expected InvalidConflict, got {other:?}"),
    }
}

#[test]
fn the_script_generates_the_test_files() {
    for f in [
        "merge", "diff3", "zdiff3", "crlf", "tail", "empty", "size", "latin1",
    ] {
        assert!(SCRIPT.contains(&format!("conflict-{f}.bin")), "{f}");
    }
}

#[test]
fn merge_style() {
    let parsed = parse(MERGE);
    assert_eq!(parsed.block_count(), 2);
    assert_eq!(parsed.segments.len(), 5);
    assert_eq!(parsed.segments[0], common(b"one\n"));
    assert_eq!(
        parsed.segments[2],
        common(b"three\nfour\nfive\nsix\nseven\n")
    );
    assert_eq!(parsed.segments[4], common(b"nine\n"));
    let b = blocks(&parsed);
    assert_eq!(b[0].ours, b"TWO main\n");
    assert_eq!(b[0].theirs, b"TWO other\n");
    assert_eq!(b[0].base, None);
    assert_eq!(b[0].ours_label, b"HEAD");
    assert_eq!(b[0].theirs_label, b"other");
    assert_eq!(b[0].base_label, b"");
    assert_eq!(b[1].ours, b"EIGHT main\nEIGHT extra\n");
    assert_eq!(b[1].theirs, b"EIGHT other\n");
    assert_eq!(rebuild(MERGE, &parsed), MERGE);
}

#[test]
fn diff3_and_zdiff3_style() {
    for text in [DIFF3, ZDIFF3] {
        let parsed = parse(text);
        let b = blocks(&parsed);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].ours, b"TWO main\n");
        assert_eq!(b[0].base.as_deref(), Some(&b"two\n"[..]));
        assert_eq!(b[0].theirs, b"TWO other\n");
        assert_eq!(
            (
                b[0].ours_label.as_slice(),
                b[0].base_label.as_slice(),
                b[0].theirs_label.as_slice()
            ),
            (&b"ours"[..], &b"base"[..], &b"theirs"[..])
        );
        assert_eq!(b[1].base.as_deref(), Some(&b"eight\n"[..]));
        assert_eq!(rebuild(text, &parsed), text);
    }
}

#[test]
fn range_delimits_the_block_exactly() {
    for text in [MERGE, DIFF3, CRLF, TAIL, EMPTY, LATIN1] {
        let parsed = parse(text);
        for b in parsed.blocks() {
            let bytes = &text[b.range.clone()];
            assert!(bytes.starts_with(b"<<<<<<< "), "{bytes:?}");
            assert!(bytes.ends_with(b"\n"));
            let last = bytes[..bytes.len() - 1]
                .rsplit(|&c| c == b'\n')
                .next()
                .unwrap_or_default();
            assert!(last.starts_with(b">>>>>>> "), "{last:?}");
            // The block starts at the start of a line.
            assert!(b.range.start == 0 || text[b.range.start - 1] == b'\n');
        }
    }
    let parsed = parse(MERGE);
    assert_eq!(blocks(&parsed)[0].range, 4..58);
    assert_eq!(
        &MERGE[4..58],
        b"<<<<<<< HEAD\nTWO main\n=======\nTWO other\n>>>>>>> other\n"
    );
}

#[test]
fn crlf_preserved() {
    let parsed = parse(CRLF);
    let b = blocks(&parsed);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].ours, b"TWO main\r\n");
    assert_eq!(b[0].theirs, b"TWO other\r\n");
    // The labels do not have the `\r`.
    assert_eq!(b[0].ours_label, b"HEAD");
    assert_eq!(b[0].theirs_label, b"other");
    assert_eq!(parsed.segments[0], common(b"one\r\n"));
    assert_eq!(
        resolve_all(CRLF, &parsed, Choice::Theirs).unwrap(),
        b"one\r\nTWO other\r\nthree\r\n"
    );
}

#[test]
fn last_line_without_newline() {
    // Real: git adds the newline to lines without one inside the block.
    let parsed = parse(TAIL);
    let b = blocks(&parsed);
    assert_eq!(b[0].ours, b"two main\n");
    assert_eq!(b[0].range.end, TAIL.len());
    // Common text at the end without a newline.
    let text = b"<<<<<<< a\nx\n=======\ny\n>>>>>>> b\nend";
    let parsed = parse(text);
    assert_eq!(parsed.segments.last(), Some(&common(b"end")));
    assert_eq!(resolve_all(text, &parsed, Choice::Ours).unwrap(), b"x\nend");
    // Closing marker as the last line without a newline.
    let text = b"a\n<<<<<<< a\nx\n=======\ny\n>>>>>>> b";
    let parsed = parse(text);
    let b = blocks(&parsed);
    assert_eq!(b[0].range, 2..text.len());
    assert_eq!(b[0].theirs_label, b"b");
    assert_eq!(parsed.segments.len(), 2);
    assert_eq!(
        resolve_all(text, &parsed, Choice::Theirs).unwrap(),
        b"a\ny\n"
    );
}

#[test]
fn block_empty_on_one_side() {
    let parsed = parse(EMPTY);
    let b = blocks(&parsed);
    assert_eq!(b[0].ours, b"");
    assert_eq!(b[0].theirs, b"X\nY\n");
    assert_eq!(
        resolve_all(EMPTY, &parsed, Choice::Ours).unwrap(),
        b"first\nafter\n"
    );
    // Both sides empty, without labels.
    let text = b"<<<<<<<\n=======\n>>>>>>>\n";
    let parsed = parse(text);
    let b = blocks(&parsed);
    assert_eq!((b[0].ours.len(), b[0].theirs.len()), (0, 0));
    assert_eq!(b[0].ours_label, b"");
    assert_eq!(b[0].theirs_label, b"");
    assert_eq!(
        resolve_all(text, &parsed, Choice::OursThenTheirs).unwrap(),
        b""
    );
}

#[test]
fn different_marker_size() {
    // With the right length the markers of 10 are a block.
    let parsed = parse_conflicts(SIZE, 10).unwrap();
    let b = blocks(&parsed);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].ours, b"TWO main\n");
    assert_eq!(b[0].theirs_label, b"other");
    // With 7 they are lines longer than the marker: text.
    let parsed = parse(SIZE);
    assert_eq!(parsed.segments, vec![common(SIZE)]);
    // With 10, the markers of 7 in normal files are text.
    assert_eq!(
        parse_conflicts(MERGE, 10).unwrap().segments,
        vec![common(MERGE)]
    );
    assert!(invalid(parse_conflicts(MERGE, 0)).contains("at least 1"));
}

#[test]
fn longer_or_attached_markers_are_text() {
    let text = b"<<<<<<<< no\n<<<<<<<x\n=======\n>>>>>>>>\n";
    assert_eq!(parse(text).segments, vec![common(text)]);
    // Inside a block a `========` line is content.
    let text = b"<<<<<<< a\n========\n=======\n>>>>>>>>\n>>>>>>> b\n";
    let parsed = parse(text);
    let b = blocks(&parsed);
    assert_eq!(b[0].ours, b"========\n");
    assert_eq!(b[0].theirs, b">>>>>>>>\n");
}

#[test]
fn isolated_separator_is_text() {
    // Setext heading and middle markers without an open `<<<<<<<`.
    let text = b"Title\n=======\n|||||||\n>>>>>>> x\ntext\n";
    assert_eq!(parse(text).segments, vec![common(text)]);
    // Also after a closed block.
    let text = b"<<<<<<< a\nx\n=======\ny\n>>>>>>> b\nTitle\n=======\n";
    let parsed = parse(text);
    assert_eq!(parsed.block_count(), 1);
    assert_eq!(parsed.segments[1], common(b"Title\n=======\n"));
}

#[test]
fn unclosed_block() {
    let reason = invalid(parse_conflicts(b"a\n<<<<<<< HEAD\nx\n=======\ny\n", 7));
    assert!(reason.contains("line 2"), "{reason}");
    assert!(reason.contains("is not closed"), "{reason}");
    // The file truncated after the first real block.
    let parsed = parse(MERGE);
    let end = blocks(&parsed)[1].range.end - 1 - b">>>>>>> other".len();
    assert!(parse_conflicts(&MERGE[..end], 7).is_err());
}

#[test]
fn parts_in_the_wrong_order() {
    for (text, line) in [
        (&b"<<<<<<< a\nx\n>>>>>>> b\n"[..], "line 3"),
        (b"<<<<<<< a\n<<<<<<< b\n", "line 2"),
        (b"<<<<<<< a\n=======\n||||||| base\n>>>>>>> b\n", "line 3"),
        (b"<<<<<<< a\n||||||| base\n||||||| base\n", "line 3"),
        (b"<<<<<<< a\n=======\n=======\n>>>>>>> b\n", "line 3"),
        (b"<<<<<<< a\n||||||| o\n>>>>>>> b\n", "line 3"),
    ] {
        let reason = invalid(parse_conflicts(text, 7));
        assert!(reason.contains(line), "{reason}");
        assert!(reason.contains("line 1"), "{reason}");
    }
}

#[test]
fn non_utf8_without_loss() {
    let parsed = parse(LATIN1);
    let b = blocks(&parsed);
    assert_eq!(b[0].ours, b"caf\xe9 main\n");
    assert_eq!(b[0].theirs, b"caf\xe9 other\n");
    assert_eq!(
        resolve_all(LATIN1, &parsed, Choice::TheirsThenOurs).unwrap(),
        b"caf\xe9 other\ncaf\xe9 main\ntwo\n"
    );
    // Non-UTF-8 label.
    let text = b"<<<<<<< r\xe9f\n=======\n>>>>>>> \xff\n";
    let b = parse(text);
    let b = blocks(&b);
    assert_eq!(b[0].ours_label, b"r\xe9f");
    assert_eq!(b[0].theirs_label, b"\xff");
}

#[test]
fn text_without_conflicts() {
    assert_eq!(parse(b"").segments, vec![]);
    assert_eq!(parse(b"a\nb").segments, vec![common(b"a\nb")]);
    assert_eq!(
        resolve_all(b"a\nb", &parse(b"a\nb"), Choice::Ours).unwrap(),
        b"a\nb"
    );
}

#[test]
fn resolved_for_each_choice() {
    let parsed = parse(DIFF3);
    let b = blocks(&parsed)[1];
    let cases = [
        (Choice::Ours, &b"EIGHT main\nEIGHT extra\n"[..]),
        (Choice::Theirs, b"EIGHT other\n"),
        (Choice::Base, b"eight\n"),
        (
            Choice::OursThenTheirs,
            b"EIGHT main\nEIGHT extra\nEIGHT other\n",
        ),
        (
            Choice::TheirsThenOurs,
            b"EIGHT other\nEIGHT main\nEIGHT extra\n",
        ),
    ];
    for (choice, expected) in cases {
        assert_eq!(b.resolved(choice).unwrap(), expected, "{choice:?}");
    }
    // Without a base, `Base` is an error.
    let parsed = parse(MERGE);
    let reason = invalid(blocks(&parsed)[0].resolved(Choice::Base));
    assert!(reason.contains("base"), "{reason}");
}

#[test]
fn resolve_block_for_each_choice() {
    let parsed = parse(DIFF3);
    let head = b"one\n".as_slice();
    let mid = b"three\nfour\nfive\nsix\nseven\n".as_slice();
    let second = &DIFF3[blocks(&parsed)[1].range.clone()];
    let cases = [
        (Choice::Ours, &b"TWO main\n"[..]),
        (Choice::Theirs, b"TWO other\n"),
        (Choice::Base, b"two\n"),
        (Choice::OursThenTheirs, b"TWO main\nTWO other\n"),
        (Choice::TheirsThenOurs, b"TWO other\nTWO main\n"),
    ];
    for (choice, resolved) in cases {
        let out = resolve_block(DIFF3, &parsed, 0, choice).unwrap();
        // Only the first block changes: the second keeps its markers.
        let expected = [head, resolved, mid, second, b"nine\n"].concat();
        assert_eq!(out, expected, "{choice:?}");
        // The result still has one block, equal to the second.
        let again = parse(&out);
        assert_eq!(again.block_count(), 1);
        assert_eq!(blocks(&again)[0].ours, blocks(&parsed)[1].ours);
    }
    // The second block, leaving the first.
    let out = resolve_block(MERGE, &parse(MERGE), 1, Choice::Theirs).unwrap();
    let first = &MERGE[blocks(&parse(MERGE))[0].range.clone()];
    assert_eq!(
        out,
        [b"one\n", first, mid, b"EIGHT other\n", b"nine\n"].concat()
    );
    // Nonexistent index.
    let reason = invalid(resolve_block(MERGE, &parse(MERGE), 2, Choice::Ours));
    assert!(reason.contains("2 conflict blocks"), "{reason}");
    // `Base` without a base.
    assert!(resolve_block(MERGE, &parse(MERGE), 0, Choice::Base).is_err());
    // Text different from the one read.
    assert!(resolve_block(b"corto", &parse(MERGE), 1, Choice::Ours).is_err());
}

#[test]
fn resolve_all_for_each_choice() {
    let parsed = parse(DIFF3);
    let cases = [
        (Choice::Ours, "TWO main\n", "EIGHT main\nEIGHT extra\n"),
        (Choice::Theirs, "TWO other\n", "EIGHT other\n"),
        (Choice::Base, "two\n", "eight\n"),
        (
            Choice::OursThenTheirs,
            "TWO main\nTWO other\n",
            "EIGHT main\nEIGHT extra\nEIGHT other\n",
        ),
        (
            Choice::TheirsThenOurs,
            "TWO other\nTWO main\n",
            "EIGHT other\nEIGHT main\nEIGHT extra\n",
        ),
    ];
    for (choice, first, second) in cases {
        let expected = format!("one\n{first}three\nfour\nfive\nsix\nseven\n{second}nine\n");
        assert_eq!(
            resolve_all(DIFF3, &parsed, choice).unwrap(),
            expected.as_bytes(),
            "{choice:?}"
        );
    }
    // Merge style: there is no `Base`.
    assert!(resolve_all(MERGE, &parse(MERGE), Choice::Base).is_err());
    assert_eq!(
        resolve_all(MERGE, &parse(MERGE), Choice::Ours).unwrap(),
        b"one\nTWO main\nthree\nfour\nfive\nsix\nseven\nEIGHT main\nEIGHT extra\nnine\n"
    );
}

#[test]
fn choice_in_json() {
    let json = serde_json::to_string(&Choice::OursThenTheirs).unwrap();
    assert_eq!(json, "\"ours_then_theirs\"");
    let back: Choice = serde_json::from_str("\"theirs\"").unwrap();
    assert_eq!(back, Choice::Theirs);
}

/// Any line: text, sometimes with a marker character at the start, with `\n`,
/// `\r\n` or (if last) without a newline.
fn line() -> impl Strategy<Value = Vec<u8>> {
    (
        prop::sample::select(vec![
            &b""[..],
            b"<",
            b"=",
            b">",
            b"|",
            b"=======",
            b"<<<<<<<<",
        ]),
        prop::collection::vec(prop::sample::select(b"ab \xe8".to_vec()), 0..4),
        prop::sample::select(vec![&b"\n"[..], b"\r\n"]),
    )
        .prop_map(|(pre, body, eol)| [pre, &body, eol].concat())
}

fn lines() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(line(), 0..4).prop_map(|r| r.concat())
}

/// A well-formed block: (text, ours, base, theirs).
fn block() -> impl Strategy<Value = (Vec<u8>, Vec<u8>, Option<Vec<u8>>, Vec<u8>)> {
    // Lines inside a block do not start with a valid marker: no `=======`.
    let inside = || {
        lines().prop_filter("no markers", |r| {
            !r.split(|&c| c == b'\n')
                .any(|l| l.starts_with(b"=======") && !l.starts_with(b"========"))
        })
    };
    (inside(), prop::option::of(inside()), inside()).prop_map(|(ours, base, theirs)| {
        let mut text = b"<<<<<<< nostra\n".to_vec();
        text.extend_from_slice(&ours);
        if let Some(base) = &base {
            text.extend_from_slice(b"||||||| base\r\n");
            text.extend_from_slice(base);
        }
        text.extend_from_slice(b"=======\n");
        text.extend_from_slice(&theirs);
        text.extend_from_slice(b">>>>>>> loro\n");
        (text, ours, base, theirs)
    })
}

proptest! {
    #[test]
    fn segments_recompose_the_file(
        parts in prop::collection::vec((lines(), block()), 0..4),
        tail in lines(),
        senza_a_capo in any::<bool>(),
    ) {
        let mut text = Vec::new();
        let mut expected = Vec::new();
        for (common_text, (block, ours, base, theirs)) in &parts {
            text.extend_from_slice(common_text);
            text.extend_from_slice(block);
            expected.push((ours.clone(), base.clone(), theirs.clone()));
        }
        text.extend_from_slice(&tail);
        if senza_a_capo && text.ends_with(b"\n") {
            text.pop();
        }
        let parsed = parse_conflicts(&text, 7).unwrap();
        prop_assert_eq!(rebuild(&text, &parsed), text.clone());
        let found: Vec<_> = parsed
            .blocks()
            .map(|b| (b.ours.clone(), b.base.clone(), b.theirs.clone()))
            .collect();
        prop_assert_eq!(found.len(), expected.len());
        for (f, e) in found.iter().zip(&expected) {
            prop_assert_eq!(f, e);
        }
        // Resolving everything with `Ours` equals resolving the blocks one at a time, from the last.
        let mut one_by_one = text.clone();
        for i in (0..parsed.block_count()).rev() {
            one_by_one = resolve_block(&one_by_one, &parsed, i, Choice::Ours).unwrap();
        }
        prop_assert_eq!(resolve_all(&text, &parsed, Choice::Ours).unwrap(), one_by_one);
    }
}
