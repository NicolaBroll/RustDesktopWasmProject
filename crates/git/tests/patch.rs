//! Le patch per righe del costruttore del dominio (`project_domain::build_patch`) applicate
//! davvero all'indice con `git apply` e gli argomenti di `APPLY_ARGS`, su repo veri.
//!
//! Ogni patch si prova prima con `--check`, poi si applica; il risultato si legge dall'indice
//! con `git show :<percorso>` e si confronta byte per byte. Il worktree non deve cambiare.

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

/// Il file `path` del diff del worktree (per lo stage) o dell'indice (per l'unstage), letto
/// con il backend come farà il comando.
fn diff_file(repo: &Repo, path: &[u8], direction: PatchDirection) -> FileDiff {
    let path = Some(RepoPath::from_bytes(path));
    let spec = match direction {
        PatchDirection::Stage => DiffSpec::Worktree { path },
        PatchDirection::Unstage => DiffSpec::Index { path },
    };
    let mut diff = repo.open().diff(&spec).unwrap();
    assert_eq!(diff.files.len(), 1, "un file nel diff");
    diff.files.remove(0)
}

/// Il riferimento della riga di `file` con quel prefisso (`'-'` o `'+'`) e contenuto.
fn line(file: &FileDiff, prefix: char, content: &[u8]) -> LineRef {
    let kind = match prefix {
        '-' => LineKind::Removed,
        '+' => LineKind::Added,
        _ => panic!("prefisso `{prefix}`"),
    };
    let mut found = file
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == kind && l.content.as_bytes() == content);
    let l = found.next().expect("riga presente nel diff");
    assert!(found.next().is_none(), "riga unica");
    l.line_ref().unwrap()
}

fn lines(file: &FileDiff, picks: &[(char, &str)]) -> Vec<LineRef> {
    picks
        .iter()
        .map(|(p, c)| line(file, *p, c.as_bytes()))
        .collect()
}

/// `git apply <APPLY_ARGS> [--check]` con la patch su stdin.
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

/// Costruisce la patch per le righe scelte, la prova e la applica; restituisce la patch.
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
        .expect("patch non vuota");
    let worktree = std::fs::read(repo.path().join(os(path))).ok();
    let bytes = patch.to_patch();
    git_apply(repo, &bytes, true);
    git_apply(repo, &bytes, false);
    assert_eq!(
        std::fs::read(repo.path().join(os(path))).ok(),
        worktree,
        "il worktree non cambia"
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

/// Il contenuto di `path` nell'indice; `None` se non c'è.
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

/// Il modo di `path` nell'indice (`git ls-files -s`).
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
            None => format!("riga {i}\n"),
        })
        .collect()
}

#[test]
fn stage_e_unstage_di_righe_in_piu_hunk() {
    let repo = Repo::new();
    repo.commit_file("f.txt", &numbered(30, &[]), "base");
    // Hunk 1: due righe cambiate vicine; hunk 2: una riga a metà; hunk 3: in fondo.
    repo.write(
        "f.txt",
        &numbered(
            30,
            &[(2, "due"), (3, "tre"), (15, "quindici"), (29, "ventinove")],
        ),
    );
    let file = diff_file(&repo, b"f.txt", PatchDirection::Stage);
    assert_eq!(file.hunks.len(), 3);

    // Solo `riga 2 → due` del primo hunk e tutto il terzo.
    stage(
        &repo,
        "f.txt",
        &[
            ('-', "riga 2"),
            ('+', "due"),
            ('-', "riga 29"),
            ('+', "ventinove"),
        ],
    );
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(30, &[(2, "due"), (29, "ventinove")])
    );

    // Unstage della riga 29: torna com'era in HEAD, la riga 2 resta in stage.
    unstage(&repo, "f.txt", &[('-', "riga 29"), ('+', "ventinove")]);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(30, &[(2, "due")])
    );

    // Stage del resto, poi unstage di tutto: l'indice torna uguale a HEAD.
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
fn modifica_parziale_di_un_gruppo() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nx\ny\nb\n", "base");
    repo.write("f.txt", "a\nX\nY\nb\n");
    // Solo `x → X`: la riga `y` resta dopo `X`.
    stage(&repo, "f.txt", &[('-', "x"), ('+', "X")]);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nX\ny\nb\n");
    // Solo la riga tolta `y`, poi solo l'aggiunta `Y`.
    stage(&repo, "f.txt", &[('-', "y")]);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nX\nb\n");
    stage(&repo, "f.txt", &[('+', "Y")]);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nX\nY\nb\n");
}

#[test]
fn hunk_adiacenti() {
    let repo = Repo::new();
    repo.commit_file("f.txt", &numbered(20, &[]), "base");
    // Righe 4 e 12: fra i due contesti di 3 righe resta una riga sola (la 8).
    repo.write("f.txt", &numbered(20, &[(4, "quattro"), (12, "dodici")]));
    assert_eq!(
        diff_file(&repo, b"f.txt", PatchDirection::Stage)
            .hunks
            .len(),
        2
    );
    stage(&repo, "f.txt", &[('-', "riga 12"), ('+', "dodici")]);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(20, &[(12, "dodici")])
    );
    stage(&repo, "f.txt", &[('-', "riga 4"), ('+', "quattro")]);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(20, &[(4, "quattro"), (12, "dodici")])
    );
    // E indietro, prima il primo hunk.
    unstage(&repo, "f.txt", &[('-', "riga 4"), ('+', "quattro")]);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(20, &[(12, "dodici")])
    );
}

#[test]
fn fine_file_senza_a_capo() {
    let repo = Repo::new();
    repo.commit_file("coda.txt", "a\nb", "base");
    repo.write("coda.txt", "a\nb\nc");
    // Solo la nuova ultima riga: `b` prende l'a capo.
    stage(&repo, "coda.txt", &[('+', "c")]);
    assert_eq!(index_str(&repo, "coda.txt").unwrap(), "a\nb\nc");
    unstage(&repo, "coda.txt", &[('+', "c")]);
    assert_eq!(index_str(&repo, "coda.txt").unwrap(), "a\nb\n");
    // L'unstage della riga `b` con l'a capo e il ritorno di quella senza.
    unstage(&repo, "coda.txt", &[('-', "b"), ('+', "b")]);
    assert_eq!(index_str(&repo, "coda.txt").unwrap(), "a\nb");

    // Riga senza a capo aggiunta in fondo, la vecchia resta.
    repo.commit_file("toglie.txt", "uno\ndue\n", "toglie");
    repo.write("toglie.txt", "uno\nDUE");
    stage(&repo, "toglie.txt", &[('+', "DUE")]);
    assert_eq!(index_str(&repo, "toglie.txt").unwrap(), "uno\ndue\nDUE");
    stage(&repo, "toglie.txt", &[('-', "due")]);
    assert_eq!(index_str(&repo, "toglie.txt").unwrap(), "uno\nDUE");
}

#[test]
fn crlf_e_contenuto_non_utf8() {
    let repo = Repo::new();
    let base: &[u8] = b"uno\r\ndue\r\ncaff\xe8\r\n";
    std::fs::write(repo.path().join("crlf.txt"), base).unwrap();
    repo.stage(&["crlf.txt"]);
    repo.commit("base");
    std::fs::write(
        repo.path().join("crlf.txt"),
        b"UNO\r\ndue\r\ncaff\xe8\r\nquattro\xff\r\n",
    )
    .unwrap();
    apply_lines(&repo, b"crlf.txt", PatchDirection::Stage, |f| {
        vec![line(f, '+', b"quattro\xff\r")]
    });
    assert_eq!(
        index(&repo, b"crlf.txt").unwrap(),
        b"uno\r\ndue\r\ncaff\xe8\r\nquattro\xff\r\n"
    );
    apply_lines(&repo, b"crlf.txt", PatchDirection::Unstage, |f| {
        vec![line(f, '+', b"quattro\xff\r")]
    });
    assert_eq!(index(&repo, b"crlf.txt").unwrap(), base);
}

#[test]
fn nomi_tra_virgolette_con_spazi_e_non_utf8() {
    let repo = Repo::new();
    let mut names: Vec<&[u8]> = vec![b"con\ttab.txt", b"con spazio.txt", b"vir\"golette.txt"];
    if cfg!(unix) {
        names.push(b"caff\xff.txt");
    }
    for name in &names {
        std::fs::write(repo.path().join(os(name)), "a\nb\n").unwrap();
    }
    repo.git(&["add", "-A"]);
    repo.commit("nomi");
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
fn file_nuovo_con_intent_to_add() {
    let repo = Repo::new();
    repo.commit_file("base.txt", "base\n", "base");
    repo.write("nuovo.txt", "uno\ndue\ntre\n");
    repo.git(&["add", "-N", "nuovo.txt"]);
    let p = stage(&repo, "nuovo.txt", &[('+', "due")]);
    assert_eq!(p.change, Change::Added);
    assert_eq!(index_str(&repo, "nuovo.txt").unwrap(), "due\n");
    // Ora il file è nell'indice: il resto è una modifica.
    stage(&repo, "nuovo.txt", &[('+', "uno")]);
    assert_eq!(index_str(&repo, "nuovo.txt").unwrap(), "uno\ndue\n");
}

#[test]
fn file_eliminato_dal_worktree() {
    let repo = Repo::new();
    repo.commit_file("via.txt", "uno\ndue\ntre\n", "base");
    std::fs::remove_file(repo.path().join("via.txt")).unwrap();
    // Parziale: il file resta nell'indice senza la riga scelta.
    let p = stage(&repo, "via.txt", &[('-', "due")]);
    assert_eq!(p.change, Change::Modified);
    assert_eq!(index_str(&repo, "via.txt").unwrap(), "uno\ntre\n");
    // Tutte le righe rimaste: il file esce dall'indice.
    let p = apply_lines(&repo, b"via.txt", PatchDirection::Stage, |f| f.line_refs());
    assert_eq!(p.change, Change::Deleted);
    assert_eq!(index(&repo, b"via.txt"), None);
}

#[test]
fn unstage_di_file_nuovo_ed_eliminato() {
    let repo = Repo::new();
    repo.commit_file("vecchio.txt", "v1\nv2\nv3\n", "base");
    repo.write("nuovo.txt", "n1\nn2\nn3\n");
    repo.stage(&["nuovo.txt"]);
    repo.git(&["rm", "--cached", "--quiet", "vecchio.txt"]);

    // File nuovo in stage: parziale lo lascia nell'indice, totale lo toglie.
    let p = unstage(&repo, "nuovo.txt", &[('+', "n2")]);
    assert_eq!(p.change, Change::Modified);
    assert_eq!(index_str(&repo, "nuovo.txt").unwrap(), "n1\nn3\n");
    let p = apply_lines(&repo, b"nuovo.txt", PatchDirection::Unstage, |f| {
        f.line_refs()
    });
    assert_eq!(p.change, Change::Deleted);
    assert_eq!(index(&repo, b"nuovo.txt"), None);
    assert!(repo.path().join("nuovo.txt").exists());

    // File eliminato in stage: torna nell'indice con le sole righe scelte.
    let p = unstage(&repo, "vecchio.txt", &[('-', "v1"), ('-', "v3")]);
    assert_eq!(p.change, Change::Added);
    assert_eq!(index_str(&repo, "vecchio.txt").unwrap(), "v1\nv3\n");
}

#[test]
fn unstage_in_una_rinomina() {
    let repo = Repo::new();
    let base = numbered(8, &[]);
    repo.commit_file("vecchio.txt", &base, "base");
    repo.git(&["mv", "vecchio.txt", "nuovo nome.txt"]);
    repo.write("nuovo nome.txt", &numbered(8, &[(7, "sette")]));
    repo.stage(&["nuovo nome.txt"]);
    let file = diff_file(&repo, b"nuovo nome.txt", PatchDirection::Unstage);
    assert_eq!(file.change, Change::Renamed);

    let p = unstage(&repo, "nuovo nome.txt", &[('-', "riga 7"), ('+', "sette")]);
    assert_eq!(p.change, Change::Modified);
    // La rinomina resta: il percorso nuovo ha il contenuto di HEAD, il vecchio non torna.
    assert_eq!(index_str(&repo, "nuovo nome.txt").unwrap(), base);
    assert_eq!(index(&repo, b"vecchio.txt"), None);
}

#[cfg(unix)]
#[test]
fn il_modo_non_cambia() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.commit_file("script.sh", "uno\n", "base");
    repo.write("script.sh", "uno\ndue\n");
    let path = repo.path().join("script.sh");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let file = diff_file(&repo, b"script.sh", PatchDirection::Stage);
    assert_eq!(file.new_mode, Some(0o100755));
    stage(&repo, "script.sh", &[('+', "due")]);
    assert_eq!(index_str(&repo, "script.sh").unwrap(), "uno\ndue\n");
    assert_eq!(index_mode(&repo, "script.sh"), "100644");
}

#[test]
fn configurazione_degli_spazi_neutralizzata() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a  b\nc\n", "base");
    repo.git(&["config", "apply.whitespace", "error"]);
    repo.git(&["config", "apply.ignoreWhitespace", "change"]);
    // Una riga con spazi finali: con `apply.whitespace=error` git la rifiuterebbe.
    repo.write("f.txt", "a  b\nc\nspazi finali   \n");
    stage(&repo, "f.txt", &[('+', "spazi finali   ")]);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        "a  b\nc\nspazi finali   \n"
    );
    // Con `apply.ignoreWhitespace=change` un preimage diverso solo negli spazi passerebbe.
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
        "preimage diverso accettato"
    );
}
