//! The line patches of the domain builder (`project_domain::build_patch`) actually applied
//! to the index with `git apply` and the `APPLY_ARGS` arguments, on real repos.
//!
//! Each patch is first tried with `--check`, then applied; the result is read from the index
//! with `git show :<path>` and compared byte for byte. The worktree must not change.

mod common;

use common::{GIT_ENV, Repo};
use project_domain::{
    APPLY_ARGS, Change, FileDiff, LineKind, LineRef, PatchDirection, RepoPath, build_patch,
};
use project_engine::{DiffSpec, GitBackend};
use std::ffi::OsStr;
use std::io::Write;
use std::process::{Command, Stdio};

#[cfg(unix)]
fn os(path: &[u8]) -> &OsStr {
    use std::os::unix::ffi::OsStrExt;
    OsStr::from_bytes(path)
}

#[cfg(not(unix))]
fn os(path: &[u8]) -> &OsStr {
    OsStr::new(std::str::from_utf8(path).unwrap())
}

fn git(repo: &Repo) -> Command {
    let mut cmd = Command::new("git");
    cmd.envs(GIT_ENV.iter().copied()).arg("-C").arg(repo.path());
    cmd
}

/// The file `path` of the worktree diff (for stage) or the index diff (for unstage), read
/// with the backend as the command will do.
fn diff_file(repo: &Repo, path: &[u8], direction: PatchDirection) -> FileDiff {
    let path = Some(RepoPath::from_bytes(path));
    let spec = match direction {
        PatchDirection::Stage => DiffSpec::Worktree { path },
        PatchDirection::Unstage => DiffSpec::Index { path },
    };
    let mut diff = repo.open().diff(&spec).unwrap();
    assert_eq!(diff.files.len(), 1, "one file in the diff");
    diff.files.remove(0)
}

/// The reference of the line of `file` with that prefix (`'-'` or `'+'`) and content.
fn line(file: &FileDiff, prefix: char, content: &[u8]) -> LineRef {
    let kind = match prefix {
        '-' => LineKind::Removed,
        '+' => LineKind::Added,
        _ => panic!("prefix `{prefix}`"),
    };
    let mut found = file
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == kind && l.content.as_bytes() == content);
    let l = found.next().expect("line present in the diff");
    assert!(found.next().is_none(), "unique line");
    l.line_ref().unwrap()
}

fn lines(file: &FileDiff, picks: &[(char, &str)]) -> Vec<LineRef> {
    picks
        .iter()
        .map(|(p, c)| line(file, *p, c.as_bytes()))
        .collect()
}

/// `git apply <APPLY_ARGS> [--check]` with the patch on stdin.
fn git_apply(repo: &Repo, patch: &[u8], check: bool) {
    let mut cmd = git(repo);
    cmd.arg("apply").args(APPLY_ARGS);
    if check {
        cmd.arg("--check");
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(patch).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git apply {}: {}\n--- patch ---\n{}",
        if check { "--check" } else { "" },
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(patch)
    );
}

/// Builds the patch for the chosen lines, tries it and applies it; returns the patch.
fn apply_lines(
    repo: &Repo,
    path: &[u8],
    direction: PatchDirection,
    pick: impl FnOnce(&FileDiff) -> Vec<LineRef>,
) -> FileDiff {
    let file = diff_file(repo, path, direction);
    let refs = pick(&file);
    let patch = build_patch(&file, &refs, direction)
        .unwrap()
        .expect("non-empty patch");
    let worktree = std::fs::read(repo.path().join(os(path))).ok();
    let bytes = patch.to_patch();
    git_apply(repo, &bytes, true);
    git_apply(repo, &bytes, false);
    assert_eq!(
        std::fs::read(repo.path().join(os(path))).ok(),
        worktree,
        "the worktree does not change"
    );
    patch
}

fn stage(repo: &Repo, path: &str, picks: &[(char, &str)]) -> FileDiff {
    apply_lines(repo, path.as_bytes(), PatchDirection::Stage, |f| {
        lines(f, picks)
    })
}

fn unstage(repo: &Repo, path: &str, picks: &[(char, &str)]) -> FileDiff {
    apply_lines(repo, path.as_bytes(), PatchDirection::Unstage, |f| {
        lines(f, picks)
    })
}

/// The content of `path` in the index; `None` if absent.
fn index(repo: &Repo, path: &[u8]) -> Option<Vec<u8>> {
    let mut spec = b":".to_vec();
    spec.extend_from_slice(path);
    let out = git(repo)
        .args(["cat-file", "blob"])
        .arg(os(&spec))
        .output()
        .unwrap();
    out.status.success().then_some(out.stdout)
}

fn index_str(repo: &Repo, path: &str) -> Option<String> {
    index(repo, path.as_bytes()).map(|b| String::from_utf8(b).unwrap())
}

/// The mode of `path` in the index (`git ls-files -s`).
fn index_mode(repo: &Repo, path: &str) -> String {
    let out = git(repo)
        .args(["ls-files", "-s", "--", path])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()[..6].to_owned()
}

fn numbered(n: usize, changed: &[(usize, &str)]) -> String {
    (1..=n)
        .map(|i| match changed.iter().find(|(j, _)| *j == i) {
            Some((_, text)) => format!("{text}\n"),
            None => format!("line {i}\n"),
        })
        .collect()
}

#[test]
fn stage_and_unstage_of_lines_in_several_hunks() {
    let repo = Repo::new();
    repo.commit_file("f.txt", &numbered(30, &[]), "base");
    // Hunk 1: two changed lines close together; hunk 2: one line in the middle; hunk 3: at the end.
    repo.write(
        "f.txt",
        &numbered(
            30,
            &[
                (2, "two"),
                (3, "three"),
                (15, "fifteen"),
                (29, "twentynine"),
            ],
        ),
    );
    let file = diff_file(&repo, b"f.txt", PatchDirection::Stage);
    assert_eq!(file.hunks.len(), 3);

    // Only `line 2 → two` of the first hunk and the whole third.
    stage(
        &repo,
        "f.txt",
        &[
            ('-', "line 2"),
            ('+', "two"),
            ('-', "line 29"),
            ('+', "twentynine"),
        ],
    );
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(30, &[(2, "two"), (29, "twentynine")])
    );

    // Unstage of line 29: back to what it was in HEAD, line 2 stays staged.
    unstage(&repo, "f.txt", &[('-', "line 29"), ('+', "twentynine")]);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(30, &[(2, "two")])
    );

    // Stage the rest, then unstage everything: the index equals HEAD again.
    let rest = diff_file(&repo, b"f.txt", PatchDirection::Stage).line_refs();
    apply_lines(&repo, b"f.txt", PatchDirection::Stage, |_| rest);
    assert_eq!(
        index(&repo, b"f.txt"),
        Some(std::fs::read(repo.path().join("f.txt")).unwrap())
    );
    apply_lines(&repo, b"f.txt", PatchDirection::Unstage, |f| f.line_refs());
    assert_eq!(index_str(&repo, "f.txt").unwrap(), numbered(30, &[]));
}

#[test]
fn partial_change_of_a_group() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nx\ny\nb\n", "base");
    repo.write("f.txt", "a\nX\nY\nb\n");
    // Only `x → X`: line `y` stays after `X`.
    stage(&repo, "f.txt", &[('-', "x"), ('+', "X")]);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nX\ny\nb\n");
    // Only the removed line `y`, then only the added `Y`.
    stage(&repo, "f.txt", &[('-', "y")]);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nX\nb\n");
    stage(&repo, "f.txt", &[('+', "Y")]);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nX\nY\nb\n");
}

#[test]
fn adjacent_hunks() {
    let repo = Repo::new();
    repo.commit_file("f.txt", &numbered(20, &[]), "base");
    // Lines 4 and 12: between the two 3-line contexts only one line remains (line 8).
    repo.write("f.txt", &numbered(20, &[(4, "four"), (12, "twelve")]));
    assert_eq!(
        diff_file(&repo, b"f.txt", PatchDirection::Stage)
            .hunks
            .len(),
        2
    );
    stage(&repo, "f.txt", &[('-', "line 12"), ('+', "twelve")]);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(20, &[(12, "twelve")])
    );
    stage(&repo, "f.txt", &[('-', "line 4"), ('+', "four")]);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(20, &[(4, "four"), (12, "twelve")])
    );
    // And back, the first hunk first.
    unstage(&repo, "f.txt", &[('-', "line 4"), ('+', "four")]);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(20, &[(12, "twelve")])
    );
}

#[test]
fn end_of_file_without_newline() {
    let repo = Repo::new();
    repo.commit_file("tail.txt", "a\nb", "base");
    repo.write("tail.txt", "a\nb\nc");
    // Only the new last line: `b` gets the newline.
    stage(&repo, "tail.txt", &[('+', "c")]);
    assert_eq!(index_str(&repo, "tail.txt").unwrap(), "a\nb\nc");
    unstage(&repo, "tail.txt", &[('+', "c")]);
    assert_eq!(index_str(&repo, "tail.txt").unwrap(), "a\nb\n");
    // Unstaging line `b` with the newline and the return of the one without.
    unstage(&repo, "tail.txt", &[('-', "b"), ('+', "b")]);
    assert_eq!(index_str(&repo, "tail.txt").unwrap(), "a\nb");

    // Line without newline added at the end, the old one stays.
    repo.commit_file("removes.txt", "one\ntwo\n", "removes");
    repo.write("removes.txt", "one\nTWO");
    stage(&repo, "removes.txt", &[('+', "TWO")]);
    assert_eq!(index_str(&repo, "removes.txt").unwrap(), "one\ntwo\nTWO");
    stage(&repo, "removes.txt", &[('-', "two")]);
    assert_eq!(index_str(&repo, "removes.txt").unwrap(), "one\nTWO");
}

#[test]
fn crlf_and_non_utf8_content() {
    let repo = Repo::new();
    let base: &[u8] = b"one\r\ntwo\r\ncafe\xe8\r\n";
    std::fs::write(repo.path().join("crlf.txt"), base).unwrap();
    repo.stage(&["crlf.txt"]);
    repo.commit("base");
    std::fs::write(
        repo.path().join("crlf.txt"),
        b"ONE\r\ntwo\r\ncafe\xe8\r\nfour\xff\r\n",
    )
    .unwrap();
    apply_lines(&repo, b"crlf.txt", PatchDirection::Stage, |f| {
        vec![line(f, '+', b"four\xff\r")]
    });
    assert_eq!(
        index(&repo, b"crlf.txt").unwrap(),
        b"one\r\ntwo\r\ncafe\xe8\r\nfour\xff\r\n"
    );
    apply_lines(&repo, b"crlf.txt", PatchDirection::Unstage, |f| {
        vec![line(f, '+', b"four\xff\r")]
    });
    assert_eq!(index(&repo, b"crlf.txt").unwrap(), base);
}

#[test]
fn quoted_names_with_spaces_and_non_utf8() {
    let repo = Repo::new();
    let mut names: Vec<&[u8]> = vec![b"with\ttab.txt", b"with space.txt", b"quo\"tes.txt"];
    if cfg!(unix) {
        names.push(b"cafe\xff.txt");
    }
    for name in &names {
        std::fs::write(repo.path().join(os(name)), "a\nb\n").unwrap();
    }
    repo.git(&["add", "-A"]);
    repo.commit("names");
    for name in &names {
        std::fs::write(repo.path().join(os(name)), "a\nB\nc\n").unwrap();
        apply_lines(&repo, name, PatchDirection::Stage, |f| {
            vec![line(f, '+', b"c")]
        });
        assert_eq!(index(&repo, name).unwrap(), b"a\nb\nc\n");
        apply_lines(&repo, name, PatchDirection::Unstage, |f| {
            vec![line(f, '+', b"c")]
        });
        assert_eq!(index(&repo, name).unwrap(), b"a\nb\n");
    }
}

#[test]
fn new_file_with_intent_to_add() {
    let repo = Repo::new();
    repo.commit_file("base.txt", "base\n", "base");
    repo.write("new.txt", "one\ntwo\nthree\n");
    repo.git(&["add", "-N", "new.txt"]);
    let p = stage(&repo, "new.txt", &[('+', "two")]);
    assert_eq!(p.change, Change::Added);
    assert_eq!(index_str(&repo, "new.txt").unwrap(), "two\n");
    // Now the file is in the index: the rest is a change.
    stage(&repo, "new.txt", &[('+', "one")]);
    assert_eq!(index_str(&repo, "new.txt").unwrap(), "one\ntwo\n");
}

#[test]
fn file_deleted_from_the_worktree() {
    let repo = Repo::new();
    repo.commit_file("gone.txt", "one\ntwo\nthree\n", "base");
    std::fs::remove_file(repo.path().join("gone.txt")).unwrap();
    // Partial: the file stays in the index without the chosen line.
    let p = stage(&repo, "gone.txt", &[('-', "two")]);
    assert_eq!(p.change, Change::Modified);
    assert_eq!(index_str(&repo, "gone.txt").unwrap(), "one\nthree\n");
    // All the remaining lines: the file leaves the index.
    let p = apply_lines(&repo, b"gone.txt", PatchDirection::Stage, |f| f.line_refs());
    assert_eq!(p.change, Change::Deleted);
    assert_eq!(index(&repo, b"gone.txt"), None);
}

#[test]
fn unstage_of_new_and_deleted_file() {
    let repo = Repo::new();
    repo.commit_file("old.txt", "v1\nv2\nv3\n", "base");
    repo.write("new.txt", "n1\nn2\nn3\n");
    repo.stage(&["new.txt"]);
    repo.git(&["rm", "--cached", "--quiet", "old.txt"]);

    // New staged file: partial leaves it in the index, total removes it.
    let p = unstage(&repo, "new.txt", &[('+', "n2")]);
    assert_eq!(p.change, Change::Modified);
    assert_eq!(index_str(&repo, "new.txt").unwrap(), "n1\nn3\n");
    let p = apply_lines(&repo, b"new.txt", PatchDirection::Unstage, |f| {
        f.line_refs()
    });
    assert_eq!(p.change, Change::Deleted);
    assert_eq!(index(&repo, b"new.txt"), None);
    assert!(repo.path().join("new.txt").exists());

    // File deleted in the index: it returns to the index with only the chosen lines.
    let p = unstage(&repo, "old.txt", &[('-', "v1"), ('-', "v3")]);
    assert_eq!(p.change, Change::Added);
    assert_eq!(index_str(&repo, "old.txt").unwrap(), "v1\nv3\n");
}

#[test]
fn unstage_in_a_rename() {
    let repo = Repo::new();
    let base = numbered(8, &[]);
    repo.commit_file("old.txt", &base, "base");
    repo.git(&["mv", "old.txt", "new name.txt"]);
    repo.write("new name.txt", &numbered(8, &[(7, "seven")]));
    repo.stage(&["new name.txt"]);
    let file = diff_file(&repo, b"new name.txt", PatchDirection::Unstage);
    assert_eq!(file.change, Change::Renamed);

    let p = unstage(&repo, "new name.txt", &[('-', "line 7"), ('+', "seven")]);
    assert_eq!(p.change, Change::Modified);
    // The rename stays: the new path has the HEAD content, the old one does not come back.
    assert_eq!(index_str(&repo, "new name.txt").unwrap(), base);
    assert_eq!(index(&repo, b"old.txt"), None);
}

#[cfg(unix)]
#[test]
fn the_mode_does_not_change() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.commit_file("script.sh", "one\n", "base");
    repo.write("script.sh", "one\ntwo\n");
    let path = repo.path().join("script.sh");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let file = diff_file(&repo, b"script.sh", PatchDirection::Stage);
    assert_eq!(file.new_mode, Some(0o100755));
    stage(&repo, "script.sh", &[('+', "two")]);
    assert_eq!(index_str(&repo, "script.sh").unwrap(), "one\ntwo\n");
    assert_eq!(index_mode(&repo, "script.sh"), "100644");
}

#[test]
fn whitespace_configuration_neutralized() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a  b\nc\n", "base");
    repo.git(&["config", "apply.whitespace", "error"]);
    repo.git(&["config", "apply.ignoreWhitespace", "change"]);
    // A line with trailing spaces: with `apply.whitespace=error` git would reject it.
    repo.write("f.txt", "a  b\nc\ntrailing spaces   \n");
    stage(&repo, "f.txt", &[('+', "trailing spaces   ")]);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        "a  b\nc\ntrailing spaces   \n"
    );
    // With `apply.ignoreWhitespace=change` a preimage differing only in spaces would pass.
    let file = diff_file(&repo, b"f.txt", PatchDirection::Unstage);
    let patch = build_patch(&file, &file.line_refs(), PatchDirection::Unstage)
        .unwrap()
        .unwrap();
    let altered: Vec<u8> = String::from_utf8(patch.to_patch())
        .unwrap()
        .replace(" a  b\n", " a b\n")
        .into_bytes();
    let mut cmd = git(&repo);
    cmd.arg("apply").args(APPLY_ARGS).arg("--check");
    let mut child = cmd
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&altered).unwrap();
    assert!(
        !child.wait().unwrap().success(),
        "different preimage accepted"
    );
}
