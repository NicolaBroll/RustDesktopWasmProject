//! Test di integrazione di `CliBackend` su repo veri creati in cartelle temporanee, in ambiente
//! deterministico (§9): niente configurazione globale o di sistema, autore e date fissi.

use project_domain::{Change, EntryKind, Head, Oid, Operation, RefKind};
use project_engine::{EngineError, GitBackend, LogQuery, RepoOpener};
use project_git::{CliBackend, CliOpener, GitVersion};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Ambiente comune a git eseguito dai test e dal backend.
const ENV: &[(&str, &str)] = &[
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_AUTHOR_NAME", "Ada"),
    ("GIT_AUTHOR_EMAIL", "ada@example.com"),
    ("GIT_COMMITTER_NAME", "Bob"),
    ("GIT_COMMITTER_EMAIL", "bob@example.com"),
];

/// Un repo temporaneo, cancellato alla fine del test.
struct Repo {
    dir: TempDir,
    /// Data del prossimo commit, in secondi: ogni commit è un minuto dopo il precedente.
    clock: std::cell::Cell<i64>,
}

impl Repo {
    fn new() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
            clock: std::cell::Cell::new(1_700_000_000),
        };
        repo.git(&["init", "--quiet", "--initial-branch=main"]);
        repo
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn path_str(&self) -> &str {
        self.path().to_str().unwrap()
    }

    /// Esegue git nel repo e restituisce stdout; il test fallisce se git fallisce.
    fn git(&self, args: &[&str]) -> String {
        let out = self.git_output(args);
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
    }

    fn git_output(&self, args: &[&str]) -> std::process::Output {
        let date = format!("{} +0200", self.clock.get());
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(self.path()).args(args);
        cmd.envs(ENV.iter().copied())
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .env("LC_ALL", "C");
        cmd.output().unwrap()
    }

    fn write(&self, file: &str, content: &str) {
        let path = self.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Scrive `file`, lo mette in stage e crea un commit con `message`; restituisce l'id.
    fn commit_file(&self, file: &str, content: &str, message: &str) -> Oid {
        self.write(file, content);
        self.git(&["add", "--", file]);
        self.commit(&["-m", message])
    }

    fn commit(&self, args: &[&str]) -> Oid {
        self.git(&[&["commit", "--quiet"], args].concat());
        self.clock.set(self.clock.get() + 60);
        self.rev("HEAD")
    }

    fn rev(&self, rev: &str) -> Oid {
        self.git(&["rev-parse", "--verify", rev]).parse().unwrap()
    }

    fn opener() -> CliOpener {
        ENV.iter()
            .fold(CliOpener::new(), |opener, (k, v)| opener.env(*k, *v))
    }

    fn open(&self) -> CliBackend {
        Self::opener().open_cli(self.path_str()).unwrap()
    }
}

fn ids(commits: &[project_domain::Commit]) -> Vec<Oid> {
    commits.iter().map(|c| c.id).collect()
}

#[test]
fn git_version_is_supported() {
    let version = Repo::opener().git_version().unwrap();
    assert!(version >= GitVersion::MIN, "{version}");
}

#[test]
fn missing_git_is_a_clear_error() {
    let repo = Repo::new();
    let err = CliOpener::with_program("/nessun/git/qui")
        .open(repo.path_str())
        .err()
        .unwrap();
    assert!(matches!(err, EngineError::GitNotFound(_)), "{err}");
}

#[test]
fn opening_a_plain_folder_fails() {
    let dir = tempfile::tempdir().unwrap();
    // Senza questo git risalirebbe fino a un eventuale repo che contiene la cartella temporanea.
    let opener = Repo::opener().env("GIT_CEILING_DIRECTORIES", dir.path().parent().unwrap());
    let err = opener.open(dir.path().to_str().unwrap()).err().unwrap();
    let EngineError::RepoNotFound { reason, .. } = err else {
        panic!("{err}")
    };
    assert!(reason.contains("not a git repository"), "{reason}");

    let missing = dir.path().join("non-esiste");
    let err = opener.open(missing.to_str().unwrap()).err().unwrap();
    assert!(matches!(err, EngineError::RepoNotFound { .. }), "{err}");

    let err = opener.open("").err().unwrap();
    assert!(matches!(err, EngineError::RepoNotFound { .. }), "{err}");
}

#[test]
fn opening_a_bare_repo_fails() {
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new("git")
        .args(["init", "--quiet", "--bare"])
        .arg(dir.path())
        .envs(ENV.iter().copied())
        .status()
        .unwrap();
    assert!(status.success());
    let err = Repo::opener()
        .open(dir.path().to_str().unwrap())
        .err()
        .unwrap();
    let EngineError::RepoNotFound { reason, .. } = err else {
        panic!("{err}")
    };
    assert!(reason.contains("bare"), "{reason}");
}

#[test]
fn opening_a_subfolder_uses_the_worktree_root() {
    let repo = Repo::new();
    repo.commit_file("src/main.rs", "fn main() {}\n", "primo");
    let backend = Repo::opener()
        .open_cli(repo.path().join("src").to_str().unwrap())
        .unwrap();
    assert_eq!(
        backend.workdir().canonicalize().unwrap(),
        repo.path().canonicalize().unwrap()
    );
    assert_eq!(backend.git_dir().file_name().unwrap(), ".git");
}

#[test]
fn empty_repo() {
    let repo = Repo::new();
    let backend = repo.open();
    let head = Head::Branch {
        name: "main".to_owned(),
        target: None,
    };
    assert_eq!(backend.head().unwrap(), head);
    assert!(backend.refs().unwrap().is_empty());
    assert!(backend.log(&LogQuery::all()).unwrap().is_empty());
    let status = backend.status().unwrap();
    assert_eq!(status.head, head);
    assert!(status.is_clean());
    assert_eq!(status.operation, None);
}

/// Repo con due branch e un merge:
///
/// ```text
/// m  merge di feature in main
/// |\
/// | f2
/// | f1
/// c2 |
/// |/
/// c1
/// ```
struct History {
    repo: Repo,
    c1: Oid,
    c2: Oid,
    f1: Oid,
    f2: Oid,
    m: Oid,
}

fn history() -> History {
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "uno\n", "primo commit");
    repo.git(&["branch", "feature"]);
    let c2 = repo.commit_file("a.txt", "due\n", "secondo su main\n\nCorpo\nsu due righe.");
    repo.git(&["checkout", "--quiet", "feature"]);
    let f1 = repo.commit_file("b.txt", "b\n", "feature 1");
    let f2 = repo.commit_file("b.txt", "bb\n", "feature 2");
    repo.git(&["checkout", "--quiet", "main"]);
    repo.git(&["merge", "--quiet", "--no-ff", "--no-edit", "feature"]);
    repo.clock.set(repo.clock.get() + 60);
    let m = repo.rev("HEAD");
    History {
        repo,
        c1,
        c2,
        f1,
        f2,
        m,
    }
}

#[test]
fn log_reads_the_whole_history() {
    let h = history();
    let backend = h.repo.open();
    let log = backend.log(&LogQuery::all()).unwrap();
    // Figli prima dei genitori, poi il più recente per data del committer.
    assert_eq!(ids(&log), [h.m, h.f2, h.f1, h.c2, h.c1]);

    let merge = &log[0];
    assert!(merge.is_merge());
    assert_eq!(merge.parents, [h.c2, h.f2]);
    assert_eq!(merge.summary, "Merge branch 'feature'");

    let c2 = &log[3];
    assert_eq!(c2.summary, "secondo su main");
    assert_eq!(c2.body, "Corpo\nsu due righe.");
    assert_eq!(c2.author.name, "Ada");
    assert_eq!(c2.author.email, "ada@example.com");
    assert_eq!(c2.committer.name, "Bob");
    assert_eq!(c2.author.time, 1_700_000_060);
    assert_eq!(c2.author.offset_min, 120);

    let root = &log[4];
    assert!(root.parents.is_empty());
    assert_eq!(root.body, "");
}

#[test]
fn log_pages_and_revs() {
    let h = history();
    let backend = h.repo.open();
    let page = backend.log(&LogQuery::page(1, 2)).unwrap();
    assert_eq!(ids(&page), [h.f2, h.f1]);
    assert!(backend.log(&LogQuery::page(0, 0)).unwrap().is_empty());
    assert!(backend.log(&LogQuery::page(10, 5)).unwrap().is_empty());

    let feature = backend.log(&LogQuery::all().from_rev("feature")).unwrap();
    assert_eq!(ids(&feature), [h.f2, h.f1, h.c1]);
    let short = h.c2.to_string()[..8].to_owned();
    let from_id = backend.log(&LogQuery::page(0, 1).from_rev(short)).unwrap();
    assert_eq!(ids(&from_id), [h.c2]);
    let full = backend
        .log(&LogQuery::all().from_rev("refs/heads/feature"))
        .unwrap();
    assert_eq!(full.len(), 3);
}

#[test]
fn log_rejects_unknown_revs_without_running_options() {
    let h = history();
    let backend = h.repo.open();
    for rev in [
        "nessuno",
        "--all",
        "-n1",
        "--output=/tmp/x",
        "",
        "main..feature",
    ] {
        let err = backend.log(&LogQuery::all().from_rev(rev)).unwrap_err();
        assert!(
            matches!(err, EngineError::UnknownRev(ref r) if r == rev),
            "{rev}: {err}"
        );
    }
}

#[test]
fn head_on_branch_and_detached() {
    let h = history();
    let backend = h.repo.open();
    assert_eq!(
        backend.head().unwrap(),
        Head::Branch {
            name: "main".to_owned(),
            target: Some(h.m),
        }
    );
    h.repo
        .git(&["checkout", "--quiet", "--detach", &h.f1.to_string()]);
    assert_eq!(backend.head().unwrap(), Head::Detached(h.f1));
    assert_eq!(ids(&backend.log(&LogQuery::all()).unwrap()), [h.f1, h.c1]);
    let status = backend.status().unwrap();
    assert_eq!(status.head, Head::Detached(h.f1));
    assert_eq!(status.upstream, None);
}

#[test]
fn refs_with_tags_stash_and_upstream() {
    let h = history();
    let repo = &h.repo;
    repo.git(&["tag", "leggero", &h.c1.to_string()]);
    repo.git(&["tag", "-a", "-m", "versione 1", "v1", &h.c2.to_string()]);
    // Un remoto finto: senza rete, bastano il ref e la configurazione (il refspec di fetch dice
    // a git che `refs/heads/main` di origin è `refs/remotes/origin/main`).
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://example.invalid/repo.git",
    ]);
    repo.git(&["update-ref", "refs/remotes/origin/main", &h.c2.to_string()]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    repo.git(&["config", "branch.main.remote", "origin"]);
    repo.git(&["config", "branch.main.merge", "refs/heads/main"]);
    repo.write("a.txt", "modificato\n");
    repo.git(&["stash", "--quiet"]);
    let stash = repo.rev("refs/stash");

    let backend = repo.open();
    let refs = backend.refs().unwrap();
    let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "refs/heads/feature",
            "refs/heads/main",
            "refs/remotes/origin/main",
            "refs/stash",
            "refs/tags/leggero",
            "refs/tags/v1",
        ]
    );
    let by_name = |name: &str| refs.iter().find(|r| r.name == name).unwrap();

    let main = by_name("refs/heads/main");
    assert_eq!(main.kind, RefKind::LocalBranch);
    assert_eq!(main.target, h.m);
    let upstream = main.upstream.as_ref().unwrap();
    assert_eq!(upstream.name, "refs/remotes/origin/main");
    // main ha f1, f2 e il merge in più rispetto a origin/main.
    assert_eq!((upstream.ahead, upstream.behind), (Some(3), Some(0)));
    assert_eq!(by_name("refs/heads/feature").upstream, None);

    assert_eq!(
        by_name("refs/remotes/origin/main").kind,
        RefKind::RemoteBranch
    );
    assert_eq!(by_name("refs/stash").kind, RefKind::Stash);
    assert_eq!(by_name("refs/stash").target, stash);

    let light = by_name("refs/tags/leggero");
    assert_eq!(
        (light.kind, light.target, light.tag_object),
        (RefKind::Tag, h.c1, None)
    );
    let annotated = by_name("refs/tags/v1");
    assert_eq!(annotated.target, h.c2);
    assert_eq!(annotated.tag_object, Some(repo.rev("refs/tags/v1")));

    let status = backend.status().unwrap();
    let upstream = status.upstream.unwrap();
    assert_eq!(upstream.name, "origin/main");
    assert_eq!((upstream.ahead, upstream.behind), (Some(3), Some(0)));

    // Il tag annotato si usa anche come rev del log.
    let from_tag = backend.log(&LogQuery::all().from_rev("v1")).unwrap();
    assert_eq!(ids(&from_tag), [h.c2, h.c1]);
}

#[test]
fn status_lists_changes() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "uno\n", "primo");
    repo.commit_file(
        "vecchio.txt",
        "contenuto abbastanza lungo da riconoscere\n",
        "secondo",
    );
    repo.write("a.txt", "modificato\n");
    repo.write("nuovo.txt", "in stage\n");
    repo.git(&["add", "nuovo.txt"]);
    repo.git(&["mv", "vecchio.txt", "rinominato.txt"]);
    repo.write("cartella/non tracciato.txt", "?\n");

    let status = repo.open().status().unwrap();
    assert!(!status.is_clean());
    let entry = |path: &str| {
        status
            .entries
            .iter()
            .find(|e| e.path.to_str() == Some(path))
            .unwrap_or_else(|| panic!("{path} non in {:?}", status.entries))
    };
    let a = entry("a.txt");
    assert_eq!(
        (a.index, a.worktree),
        (Change::Unmodified, Change::Modified)
    );
    assert!(a.is_unstaged() && !a.is_staged());
    let added = entry("nuovo.txt");
    assert_eq!(added.index, Change::Added);
    let renamed = entry("rinominato.txt");
    assert_eq!(renamed.index, Change::Renamed);
    assert_eq!(
        renamed.original_path().and_then(|p| p.to_str()),
        Some("vecchio.txt")
    );
    // Con `--untracked-files=all` il file, non la sola cartella.
    let untracked = entry("cartella/non tracciato.txt");
    assert!(untracked.is_untracked());
    assert!(matches!(untracked.kind, EntryKind::Untracked));
    assert_eq!(status.entries.len(), 4);
}

#[test]
fn status_reports_merge_conflict() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "base\n", "base");
    repo.git(&["branch", "altro"]);
    repo.commit_file("a.txt", "nostra\n", "nostra");
    repo.git(&["checkout", "--quiet", "altro"]);
    repo.commit_file("a.txt", "loro\n", "loro");
    repo.git(&["checkout", "--quiet", "main"]);
    let out = repo.git_output(&["merge", "altro"]);
    assert!(!out.status.success(), "il merge doveva andare in conflitto");

    let status = repo.open().status().unwrap();
    assert_eq!(status.operation, Some(Operation::Merge));
    assert!(status.has_conflicts());
    assert!(status.entries[0].is_conflict());

    repo.git(&["merge", "--abort"]);
    let status = repo.open().status().unwrap();
    assert_eq!(status.operation, None);
    assert!(status.is_clean());
}

#[test]
fn status_reports_cherry_pick_and_bisect() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "base\n", "base");
    repo.git(&["branch", "altro"]);
    repo.commit_file("a.txt", "nostra\n", "nostra");
    repo.git(&["checkout", "--quiet", "altro"]);
    let theirs = repo.commit_file("a.txt", "loro\n", "loro");
    repo.git(&["checkout", "--quiet", "main"]);
    let backend = repo.open();

    let out = repo.git_output(&["cherry-pick", &theirs.to_string()]);
    assert!(!out.status.success());
    assert_eq!(
        backend.status().unwrap().operation,
        Some(Operation::CherryPick)
    );
    repo.git(&["cherry-pick", "--abort"]);

    repo.git(&["bisect", "start"]);
    assert_eq!(backend.status().unwrap().operation, Some(Operation::Bisect));
    repo.git(&["bisect", "reset"]);
    assert_eq!(backend.status().unwrap().operation, None);
}

#[cfg(unix)]
#[test]
fn old_git_is_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("git");
    std::fs::write(&fake, "#!/bin/sh\necho 'git version 2.20.1'\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let repo = Repo::new();
    let err = CliOpener::with_program(&fake)
        .open(repo.path_str())
        .err()
        .unwrap();
    let EngineError::GitTooOld { found, required } = err else {
        panic!("{err}")
    };
    assert_eq!((found.as_str(), required.as_str()), ("2.20.1", "2.30.0"));
}
