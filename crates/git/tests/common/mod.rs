//! Helper per i test di integrazione: repo git veri in cartelle temporanee, in ambiente
//! deterministico (`impl-docs/ARCHITETTURA-git.md` §9).
//!
//! Ogni processo git (quelli dei test e quelli del backend aperto con [`Repo::open`]) gira senza
//! configurazione globale o di sistema, con autore, committer e branch iniziale fissi. Le date
//! partono da [`START_TIME`] e ogni comando git eseguito dal repo le sposta avanti di
//! [`TICK`] secondi: lo stesso scenario dà sempre gli stessi id di commit.
//!
//! Gli scenari pronti sono le funzioni `repo_*` in fondo al file.

// Ogni file di test usa solo una parte dell'helper.
#![allow(dead_code)]

use project_domain::Oid;
use project_git::{CliBackend, CliOpener};
use std::cell::Cell;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

/// Ambiente di ogni processo git dei test.
pub const GIT_ENV: &[(&str, &str)] = &[
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_AUTHOR_NAME", "Ada"),
    ("GIT_AUTHOR_EMAIL", "ada@example.com"),
    ("GIT_COMMITTER_NAME", "Bob"),
    ("GIT_COMMITTER_EMAIL", "bob@example.com"),
    ("LC_ALL", "C"),
];

/// Data del primo comando git di ogni repo, in secondi dall'epoca Unix.
pub const START_TIME: i64 = 1_700_000_000;
/// Secondi tra un comando git e il successivo.
pub const TICK: i64 = 60;
/// Fuso orario delle date dei test.
pub const OFFSET: &str = "+0200";

/// Un repo temporaneo, cancellato quando esce dallo scope.
pub struct Repo {
    dir: TempDir,
    clock: Cell<i64>,
}

impl Repo {
    /// Repo vuoto con worktree, su `main`.
    pub fn new() -> Self {
        Self::init(&[])
    }

    /// Repo bare vuoto (`HEAD` su `main`): da usare come remoto locale, niente rete.
    pub fn bare() -> Self {
        Self::init(&["--bare"])
    }

    fn init(extra: &[&str]) -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
            clock: Cell::new(START_TIME),
        };
        let path = repo.path_str().to_owned();
        let mut args = vec!["-c", "init.defaultBranch=main", "init", "--quiet"];
        args.extend_from_slice(extra);
        args.push(&path);
        let out = repo.command(&args).output().unwrap();
        assert!(out.status.success(), "git init: {}", stderr(&out));
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn path_str(&self) -> &str {
        self.path().to_str().unwrap()
    }

    /// Opener con lo stesso ambiente dei test.
    pub fn opener() -> CliOpener {
        GIT_ENV
            .iter()
            .fold(CliOpener::new(), |opener, (k, v)| opener.env(*k, *v))
    }

    /// Il backend sotto test, aperto sulla radice del repo.
    pub fn open(&self) -> CliBackend {
        Self::opener().open_cli(self.path_str()).unwrap()
    }

    fn command(&self, args: &[&str]) -> Command {
        let date = format!("{} {OFFSET}", self.clock.get());
        self.clock.set(self.clock.get() + TICK);
        let mut cmd = Command::new("git");
        cmd.args(args)
            .envs(GIT_ENV.iter().copied())
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date);
        cmd
    }

    /// Esegue git nel repo, qualunque sia il risultato.
    pub fn git_output(&self, args: &[&str]) -> Output {
        let mut cmd = self.command(&[]);
        cmd.arg("-C").arg(self.path()).args(args);
        cmd.output().unwrap()
    }

    /// Esegue git nel repo e restituisce stdout senza gli a capo finali; il test fallisce se
    /// git fallisce.
    pub fn git(&self, args: &[&str]) -> String {
        let out = self.git_output(args);
        assert!(out.status.success(), "git {args:?}: {}", stderr(&out));
        String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
    }

    /// Esegue git nel repo e verifica che fallisca (es. un merge in conflitto).
    pub fn git_fails(&self, args: &[&str]) {
        let out = self.git_output(args);
        assert!(!out.status.success(), "git {args:?} doveva fallire");
    }

    /// L'id a cui porta `rev`.
    pub fn rev(&self, rev: &str) -> Oid {
        self.git(&["rev-parse", "--verify", "--end-of-options", rev])
            .parse()
            .unwrap()
    }

    /// Scrive `file` (con le cartelle intermedie) senza metterlo in stage.
    pub fn write(&self, file: &str, content: &str) {
        let path = self.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Mette in stage `files`.
    pub fn stage(&self, files: &[&str]) {
        self.git(&[&["add", "--"], files].concat());
    }

    /// Commit di quello che è in stage; restituisce l'id.
    pub fn commit(&self, message: &str) -> Oid {
        self.git(&["commit", "--quiet", "-m", message]);
        self.rev("HEAD")
    }

    /// Scrive `file`, lo mette in stage e crea un commit; restituisce l'id.
    pub fn commit_file(&self, file: &str, content: &str, message: &str) -> Oid {
        self.write(file, content);
        self.stage(&[file]);
        self.commit(message)
    }

    /// Crea il branch `name` su HEAD, senza spostarsi.
    pub fn branch(&self, name: &str) {
        self.git(&["branch", "--end-of-options", name]);
    }

    /// Passa al branch `name`. Il `--` finale fa leggere `name` come branch, non come file.
    pub fn checkout(&self, name: &str) {
        self.git(&["checkout", "--quiet", name, "--"]);
    }

    /// Stacca HEAD sul commit `id`.
    pub fn detach(&self, id: Oid) {
        self.git(&["checkout", "--quiet", "--detach", &id.to_string()]);
    }

    /// Merge di `branch` nel branch corrente con un commit di merge (`--no-ff`) e il messaggio
    /// di default di git; restituisce l'id del merge.
    pub fn merge(&self, branch: &str) -> Oid {
        self.git(&["merge", "--quiet", "--no-ff", "--no-edit", branch]);
        self.rev("HEAD")
    }

    /// Prepara un conflitto su `file`: dal commit corrente, il branch `other` lo scrive con
    /// `theirs` e il branch corrente con `ours`. Restituisce l'id del commit di `other`; HEAD
    /// resta sul branch corrente, che deve esistere.
    pub fn diverge(&self, other: &str, file: &str, ours: &str, theirs: &str) -> Oid {
        let current = self.git(&["symbolic-ref", "--short", "HEAD"]);
        self.branch(other);
        self.checkout(other);
        let theirs = self.commit_file(file, theirs, &format!("{file} su {other}"));
        self.checkout(&current);
        self.commit_file(file, ours, &format!("{file} su {current}"));
        theirs
    }

    /// Merge di `branch` che finisce in conflitto: il repo resta in merge.
    pub fn conflict(&self, branch: &str) {
        self.git_fails(&["merge", "--no-edit", branch]);
    }

    /// Tag leggero `name` su `rev`.
    pub fn tag(&self, name: &str, rev: &str) {
        self.git(&["tag", "--end-of-options", name, rev]);
    }

    /// Tag annotato `name` su `rev`; restituisce l'id dell'oggetto tag.
    pub fn annotated_tag(&self, name: &str, rev: &str, message: &str) -> Oid {
        self.git(&["tag", "-a", "-m", message, "--end-of-options", name, rev]);
        self.rev(&format!("refs/tags/{name}"))
    }

    /// `git stash` delle modifiche ai file tracciati; restituisce l'id dello stash.
    pub fn stash(&self) -> Oid {
        self.git(&["stash", "--quiet"]);
        self.rev("refs/stash")
    }

    /// Aggiunge il remoto `name` che punta al repo `remote` (di solito [`Repo::bare`]).
    pub fn add_remote(&self, name: &str, remote: &Repo) {
        self.git(&["remote", "add", "--", name, remote.path_str()]);
    }

    /// Push di `rev` nel branch `branch` del remoto, poi fetch: il branch remoto locale
    /// (`refs/remotes/<remote>/<branch>`) è aggiornato.
    pub fn push(&self, remote: &str, rev: &str, branch: &str) {
        self.git(&[
            "push",
            "--quiet",
            remote,
            &format!("{rev}:refs/heads/{branch}"),
        ]);
        self.git(&["fetch", "--quiet", remote]);
    }

    /// Il branch locale `branch` segue `<remote>/<branch>`.
    pub fn set_upstream(&self, branch: &str, remote: &str) {
        let upstream = format!("{remote}/{branch}");
        self.git(&["branch", "--quiet", "--set-upstream-to", &upstream, branch]);
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// --- Scenari ---

/// Gli id di [`repo_with_history`].
pub struct History {
    pub repo: Repo,
    pub c1: Oid,
    pub c2: Oid,
    pub f1: Oid,
    pub f2: Oid,
    pub m: Oid,
}

/// Due branch e un merge, HEAD su `main`:
///
/// ```text
/// m   Merge branch 'feature'        (main)
/// |\
/// | f2  feature 2                   (feature)
/// | f1  feature 1
/// c2 |  secondo su main, con corpo su due righe
/// |/
/// c1  primo commit                  (radice)
/// ```
pub fn repo_with_history() -> History {
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "uno\n", "primo commit");
    repo.branch("feature");
    let c2 = repo.commit_file("a.txt", "due\n", "secondo su main\n\nCorpo\nsu due righe.");
    repo.checkout("feature");
    let f1 = repo.commit_file("b.txt", "b\n", "feature 1");
    let f2 = repo.commit_file("b.txt", "bb\n", "feature 2");
    repo.checkout("main");
    let m = repo.merge("feature");
    History {
        repo,
        c1,
        c2,
        f1,
        f2,
        m,
    }
}

/// [`repo_with_history`] con ref di ogni tipo:
///
/// - remoto `origin` (repo bare locale, restituito) con `main` su `c2`; `main` lo segue ed è
///   avanti di 3 (`f1`, `f2`, `m`);
/// - tag leggero `leggero` su `c1`, tag annotato `v1` su `c2`;
/// - uno stash con una modifica ad `a.txt`; il worktree resta pulito.
pub fn repo_with_all_refs() -> (History, Repo) {
    let h = repo_with_history();
    let origin = Repo::bare();
    h.repo.add_remote("origin", &origin);
    h.repo.push("origin", &h.c2.to_string(), "main");
    h.repo.set_upstream("main", "origin");
    h.repo.tag("leggero", &h.c1.to_string());
    h.repo.annotated_tag("v1", &h.c2.to_string(), "versione 1");
    h.repo.write("a.txt", "modificato\n");
    h.repo.stash();
    (h, origin)
}

/// Worktree con una modifica di ogni tipo, rispetto a due commit:
///
/// - `a.txt` modificato e non in stage;
/// - `nuovo.txt` aggiunto in stage;
/// - `vecchio.txt` rinominato in `rinominato.txt` (in stage);
/// - `cartella/non tracciato.txt` non tracciato (con uno spazio nel nome).
pub fn repo_with_changes() -> Repo {
    let repo = Repo::new();
    repo.commit_file("a.txt", "uno\n", "primo");
    repo.commit_file(
        "vecchio.txt",
        "contenuto abbastanza lungo da riconoscere\n",
        "secondo",
    );
    repo.write("a.txt", "modificato\n");
    repo.write("nuovo.txt", "in stage\n");
    repo.stage(&["nuovo.txt"]);
    repo.git(&["mv", "vecchio.txt", "rinominato.txt"]);
    repo.write("cartella/non tracciato.txt", "?\n");
    repo
}

/// `main` e `altro` cambiano in modo diverso `a.txt` da un commit comune; nessuna operazione
/// in corso. Restituisce il repo e l'id del commit di `altro`.
pub fn repo_with_divergent_branches() -> (Repo, Oid) {
    let repo = Repo::new();
    repo.commit_file("a.txt", "base\n", "base");
    let theirs = repo.diverge("altro", "a.txt", "nostra\n", "loro\n");
    (repo, theirs)
}

/// [`repo_with_divergent_branches`] con il merge di `altro` in `main` fermo sul conflitto in
/// `a.txt`.
pub fn repo_with_merge_conflict() -> Repo {
    let (repo, _) = repo_with_divergent_branches();
    repo.conflict("altro");
    repo
}

/// Gli id di [`repo_with_file_changes`].
pub struct FileChanges {
    pub repo: Repo,
    /// Commit radice: `vecchio.txt`, `sposta.txt`, `eliminato.txt`, `script.sh`.
    pub base: Oid,
    /// Rispetto a `base`: `vecchio.txt` → `nuovo.txt` (rinomina esatta), `sposta.txt` →
    /// `spostato.txt` (rinomina con modifiche), `eliminato.txt` eliminato, `script.sh` reso
    /// eseguibile, aggiunti `cartella/con spazio.txt` e un file con nome non UTF-8 (byte `0xff`,
    /// solo su Unix).
    pub changes: Oid,
    /// Commit vuoto (`--allow-empty`) dopo `changes`.
    pub empty: Oid,
}

/// Il nome non UTF-8 di [`repo_with_file_changes`].
pub const NON_UTF8_NAME: &[u8] = b"caff\xff.txt";

/// Un commit con una modifica di ogni tipo, e un commit vuoto dopo.
pub fn repo_with_file_changes() -> FileChanges {
    let repo = Repo::new();
    repo.write(
        "vecchio.txt",
        "contenuto abbastanza lungo da riconoscere la rinomina\n",
    );
    repo.write(
        "sposta.txt",
        "prima riga abbastanza lunga\nseconda riga\nterza riga\nquarta riga\n",
    );
    repo.write("eliminato.txt", "da eliminare\n");
    repo.write("script.sh", "#!/bin/sh\n");
    repo.git(&["add", "-A"]);
    let base = repo.commit("base");

    repo.git(&["mv", "vecchio.txt", "nuovo.txt"]);
    repo.git(&["mv", "sposta.txt", "spostato.txt"]);
    let moved = std::fs::read_to_string(repo.path().join("spostato.txt")).unwrap();
    repo.write("spostato.txt", &format!("{moved}quinta riga\n"));
    repo.git(&["rm", "--quiet", "eliminato.txt"]);
    repo.write("cartella/con spazio.txt", "spazio\n");
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        std::fs::write(
            repo.path().join(OsStr::from_bytes(NON_UTF8_NAME)),
            "latin\n",
        )
        .unwrap();
    }
    repo.git(&["add", "-A"]);
    // Dopo `add -A`, che riprenderebbe il modo dal file system.
    repo.git(&["update-index", "--chmod=+x", "script.sh"]);
    let changes = repo.commit("ogni tipo di modifica");
    repo.git(&["commit", "--quiet", "--allow-empty", "-m", "vuoto"]);
    let empty = repo.rev("HEAD");
    FileChanges {
        repo,
        base,
        changes,
        empty,
    }
}

/// HEAD staccata sul commit `f1` di [`repo_with_history`].
pub fn repo_detached_head() -> History {
    let h = repo_with_history();
    h.repo.detach(h.f1);
    h
}
