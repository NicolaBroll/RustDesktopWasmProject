//! Test di integrazione di `CliBackend::commit` e del comando `commit.create` su repo veri:
//! messaggi, primo commit, HEAD staccata, `amend` (con il backup del commit riscritto e la
//! conferma per un commit pubblicato), hook, firma, conflitti e operazioni in corso,
//! configurazione ostile.

mod common;

use common::{Repo, repo_detached_head, repo_with_history, repo_with_merge_conflict};
use project_domain::{Head, Operation, RepoPath};
use project_engine::{CommitRequest, EngineError, GitBackend, Registry, Session};
use project_git::CliOpener;
use serde_json::{Value, json};
use std::sync::Arc;

/// Il messaggio completo dell'ultimo commit, come lo legge git.
fn last_message(repo: &Repo) -> String {
    // `--encoding`: con `i18n.commitEncoding` impostato git ricodificherebbe l'output.
    repo.git(&["log", "-1", "--encoding=UTF-8", "--format=%B"])
}

/// Le voci dell'indice (`git ls-files -s`), per controllare che un errore non lo cambi.
fn index(repo: &Repo) -> String {
    repo.git(&["ls-files", "-s"])
}

/// Un repo con un commit (`a.txt`) e `b.txt` nuovo in stage.
fn repo_with_staged_file() -> Repo {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "primo");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    repo
}

/// Scrive l'hook `name` (script `sh`) nella cartella degli hook del repo.
#[cfg(unix)]
fn hook(repo: &Repo, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let dir = repo.path().join(".git/hooks");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Controlla che il commit non sia stato fatto: HEAD e indice come prima.
fn assert_unchanged(repo: &Repo, head: &str, index_before: &str) {
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(index(repo), index_before);
}

#[test]
fn commit_with_multiline_utf8_message() {
    let repo = repo_with_staged_file();
    let before = repo.rev("HEAD");
    let mut backend = repo.open();
    let out = backend
        .commit(&CommitRequest::new(
            "Aggiungi b: è UTF-8 ✓\n\nCorpo su\ndue righe.\n",
        ))
        .unwrap();
    assert_eq!(
        last_message(&repo),
        "Aggiungi b: è UTF-8 ✓\n\nCorpo su\ndue righe."
    );
    assert_eq!(out.commit.id, repo.rev("HEAD"));
    assert_eq!(out.commit.parents, [before]);
    assert_eq!(out.commit.summary, "Aggiungi b: è UTF-8 ✓");
    assert_eq!(out.commit.body, "Corpo su\ndue righe.");
    assert_eq!(out.commit.author.name, "Ada");
    assert_eq!(out.commit.committer.name, "Bob");
    assert_eq!(out.previous, Some(before));
    assert_eq!(out.branch.as_deref(), Some("refs/heads/main"));
    assert_eq!(out.backup, None);
    assert_eq!(
        repo.git(&["show", "--format=", "--name-only", "HEAD"]),
        "b.txt"
    );
    assert!(backend.status().unwrap().entries.is_empty());
    // Il messaggio non passa dalla riga di comando: un messaggio che sembra un'opzione resta
    // un messaggio.
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    backend.commit(&CommitRequest::new("--amend")).unwrap();
    assert_eq!(last_message(&repo), "--amend");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "3");
}

#[test]
fn message_cleanup_follows_the_configuration() {
    let repo = repo_with_staged_file();
    let mut backend = repo.open();
    // Senza `commit.cleanup`, con `-F` git usa `whitespace`: le righe `#` restano.
    backend
        .commit(&CommitRequest::new("\n#123 corregge  \n\n\n# nota\n"))
        .unwrap();
    assert_eq!(last_message(&repo), "#123 corregge\n\n# nota");

    // Con `commit.cleanup=strip` un messaggio di soli commenti è vuoto: rifiutato da git.
    repo.git(&["config", "commit.cleanup", "strip"]);
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let err = backend
        .commit(&CommitRequest::new("# solo commenti\n"))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    assert!(err.to_string().contains("commit.cleanup"), "{err}");
    assert_unchanged(&repo, &head, &before);
}

#[test]
fn first_commit() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    let mut backend = repo.open();
    let err = backend.commit(&CommitRequest::new("primo")).unwrap_err();
    assert!(matches!(err, EngineError::NothingToCommit), "{err}");
    let err = backend
        .commit(&CommitRequest::new("primo").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    repo.stage(&["a.txt"]);
    let out = backend.commit(&CommitRequest::new("primo")).unwrap();
    assert_eq!(out.previous, None);
    assert!(out.commit.parents.is_empty());
    assert_eq!(out.branch.as_deref(), Some("refs/heads/main"));
    assert_eq!(out.commit.id, repo.rev("main"));
    assert_eq!(
        backend.head().unwrap(),
        Head::Branch {
            name: "main".to_owned(),
            target: Some(out.commit.id)
        }
    );
}

#[test]
fn detached_head_moves_only_head() {
    let h = repo_detached_head();
    let main = h.repo.rev("main");
    let feature = h.repo.rev("feature");
    h.repo.write("c.txt", "c\n");
    h.repo.stage(&["c.txt"]);
    let mut backend = h.repo.open();
    let out = backend.commit(&CommitRequest::new("staccato")).unwrap();
    assert_eq!(out.branch, None);
    assert_eq!(out.previous, Some(h.f1));
    assert_eq!(out.commit.parents, [h.f1]);
    assert_eq!(backend.head().unwrap(), Head::Detached(out.commit.id));
    assert_eq!(h.repo.rev("main"), main);
    assert_eq!(h.repo.rev("feature"), feature);
}

#[test]
fn nothing_to_commit_and_empty_messages_change_nothing() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "primo");
    // Solo modifiche fuori stage e file non tracciati.
    repo.write("a.txt", "modificato\n");
    repo.write("nuovo.txt", "?\n");
    // Un file `intent-to-add` non conta, come per git.
    repo.write("ita.txt", "ita\n");
    repo.git(&["add", "-N", "ita.txt"]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let mut backend = repo.open();
    let err = backend.commit(&CommitRequest::new("niente")).unwrap_err();
    assert!(matches!(err, EngineError::NothingToCommit), "{err}");
    assert_unchanged(&repo, &head, &before);

    repo.stage(&["a.txt"]);
    let before = index(&repo);
    for message in ["", "   ", "\n\n\t\n", "a\0b"] {
        let err = backend.commit(&CommitRequest::new(message)).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{message:?}: {err}"
        );
        assert_unchanged(&repo, &head, &before);
    }
}

#[test]
fn missing_identity_is_git_error() {
    let repo = repo_with_staged_file();
    // Niente identità nell'ambiente e niente da indovinare dal sistema.
    repo.git(&["config", "user.useConfigOnly", "true"]);
    let opener = CliOpener::new()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let mut backend = opener.open_cli(repo.path_str()).unwrap();
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let err = backend
        .commit(&CommitRequest::new("chi sono?"))
        .unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err}");
    assert!(
        err.to_string().contains("Please tell me who you are"),
        "{err}"
    );
    assert_unchanged(&repo, &head, &before);
}

#[test]
fn amend_only_the_message() {
    let repo = Repo::new();
    let first = repo.commit_file("a.txt", "a\n", "primo");
    let second = repo.commit_file("b.txt", "b\n", "secondo con refuso");
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
    let author = repo.git(&["log", "-1", "--format=%an %ae %ad"]);
    let mut backend = repo.open();
    let out = backend
        .commit(&CommitRequest::new("secondo corretto").amend())
        .unwrap();
    assert_eq!(last_message(&repo), "secondo corretto");
    assert_eq!(out.previous, Some(second));
    assert_eq!(out.commit.parents, [first]);
    assert_eq!(repo.git(&["rev-parse", "HEAD^{tree}"]), tree);
    // L'autore (anche la data) resta quello del commit riscritto.
    assert_eq!(repo.git(&["log", "-1", "--format=%an %ae %ad"]), author);
    // Il vecchio HEAD è salvato in un ref di backup.
    let backup = out.backup.unwrap();
    assert!(backup.starts_with("refs/project/backup/amend-"), "{backup}");
    assert_eq!(repo.rev(&backup), second);
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "2");
}

#[test]
fn amend_with_new_files() {
    let repo = Repo::new();
    let first = repo.commit_file("a.txt", "a\n", "primo");
    let second = repo.commit_file("b.txt", "b\n", "secondo");
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let mut backend = repo.open();
    let out = backend
        .commit(&CommitRequest::new("secondo, con c").amend())
        .unwrap();
    assert_eq!(out.commit.parents, [first]);
    assert_eq!(
        repo.git(&["show", "--format=", "--name-only", "HEAD"]),
        "b.txt\nc.txt"
    );
    assert_eq!(repo.rev(out.backup.as_deref().unwrap()), second);
    assert!(backend.status().unwrap().entries.is_empty());
}

#[test]
fn amend_of_a_published_commit_needs_confirm() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "primo");
    let published = repo.commit_file("b.txt", "b\n", "pubblicato");
    let origin = Repo::bare();
    repo.add_remote("origin", &origin);
    repo.push("origin", "main", "main");
    let backups_before = repo.git(&["for-each-ref", "refs/project/"]);
    let before = index(&repo);
    let mut backend = repo.open();

    let err = backend
        .commit(&CommitRequest::new("riscritto").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert!(
        err.to_string().contains("refs/remotes/origin/main"),
        "{err}"
    );
    assert_unchanged(&repo, &published.to_string(), &before);
    // Nessun backup salvato per un commit non fatto.
    assert_eq!(repo.git(&["for-each-ref", "refs/project/"]), backups_before);

    let out = backend
        .commit(&CommitRequest::new("riscritto").amend().confirmed())
        .unwrap();
    assert_eq!(out.previous, Some(published));
    assert_eq!(repo.rev(out.backup.as_deref().unwrap()), published);
    assert_eq!(last_message(&repo), "riscritto");

    // Un commit nuovo sopra un commit pubblicato non chiede niente.
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    backend.commit(&CommitRequest::new("nuovo")).unwrap();
}

#[test]
fn amend_with_a_tag_needs_confirm_and_reports_diverging_refs() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "primo");
    let old = repo.commit_file("b.txt", "b\n", "secondo");
    repo.branch("altro");
    let mut backend = repo.open();
    // Un altro branch sul commit: niente conferma, ma resta indietro e lo si dice.
    let out = backend
        .commit(&CommitRequest::new("secondo corretto").amend())
        .unwrap();
    assert_eq!(out.also_on, ["refs/heads/altro"]);
    assert_eq!(repo.rev("altro"), old);

    // Un tag sul commit: resterebbe sul commit vecchio, serve la conferma.
    repo.tag("v1", "HEAD");
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let err = backend
        .commit(&CommitRequest::new("terzo tentativo").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert!(err.to_string().contains("refs/tags/v1"), "{err}");
    assert_unchanged(&repo, &head, &before);
    let out = backend
        .commit(&CommitRequest::new("terzo tentativo").amend().confirmed())
        .unwrap();
    assert_eq!(out.also_on, ["refs/tags/v1"]);
    // Un commit nuovo non riscrive niente.
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let out = backend.commit(&CommitRequest::new("nuovo")).unwrap();
    assert!(out.also_on.is_empty());
}

#[test]
fn amend_that_produces_the_same_commit_changes_nothing() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "primo");
    repo.commit_file("b.txt", "b\n", "secondo");
    // Date fisse impostate apposta (le variabili ereditate invece si tolgono): due `amend`
    // uguali danno lo stesso commit, come due `amend` nello stesso secondo.
    let opener = Repo::opener()
        .env("GIT_AUTHOR_DATE", "1700000000 +0200")
        .env("GIT_COMMITTER_DATE", "1700000000 +0200");
    let mut backend = opener.open_cli(repo.path_str()).unwrap();
    let first = backend
        .commit(&CommitRequest::new("uguale").amend())
        .unwrap();
    let backups = repo.git(&["for-each-ref", "refs/project/"]);
    let out = backend
        .commit(&CommitRequest::new("uguale").amend())
        .unwrap();
    assert_eq!(out.commit.id, first.commit.id);
    assert_eq!(out.previous, Some(first.commit.id));
    // Niente è cambiato: nessun backup nuovo (puntava allo stesso commit).
    assert_eq!(out.backup, None);
    assert_eq!(repo.git(&["for-each-ref", "refs/project/"]), backups);
}

#[cfg(unix)]
#[test]
fn failed_commit_that_moved_head_keeps_the_backup() {
    let repo = Repo::new();
    let first = repo.commit_file("a.txt", "a\n", "primo");
    let second = repo.commit_file("b.txt", "b\n", "secondo");
    // L'hook sposta HEAD e poi rifiuta il commit.
    hook(&repo, "pre-commit", "git reset -q --soft HEAD~1\nexit 1");
    let mut backend = repo.open();
    let err = backend
        .commit(&CommitRequest::new("x").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err}");
    assert_eq!(repo.rev("HEAD"), first);
    let backup = repo.git(&["for-each-ref", "--format=%(refname)", "refs/project/"]);
    assert!(backup.starts_with("refs/project/backup/amend-"), "{backup}");
    assert_eq!(repo.rev(&backup), second);
    assert!(err.to_string().contains(&backup), "{err}");
}

#[cfg(unix)]
#[test]
fn error_after_a_successful_amend_names_the_backup() {
    let repo = Repo::new();
    let first = repo.commit_file("a.txt", "a\n", "primo");
    let second = repo.commit_file("b.txt", "b\n", "secondo");
    // Il commit riesce, poi un `post-commit` sposta HEAD.
    hook(&repo, "post-commit", "git reset -q --hard HEAD~1");
    let mut backend = repo.open();
    let err = backend
        .commit(&CommitRequest::new("riscritto").amend())
        .unwrap_err();
    let EngineError::PostCommit { commit, backup, .. } = &err else {
        panic!("{err}");
    };
    // HEAD non è il commit creato: non lo si spaccia per quello.
    assert_eq!(*commit, None);
    let backup = backup.clone().unwrap();
    assert_eq!(repo.rev(&backup), second);
    assert!(err.to_string().contains(&backup), "{err}");
    assert!(err.to_string().contains("non ripetere"), "{err}");
    assert_eq!(repo.rev("HEAD"), first);

    // Lo stesso tramite il comando: l'errore è `PostCommit`, non un commit fallito.
    repo.git(&["reset", "-q", "--hard", &second.to_string()]);
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(Repo::opener()));
    registry
        .run(
            "repo.open",
            &mut session,
            json!({ "path": repo.path_str() }),
        )
        .unwrap();
    let err = registry
        .run(
            "commit.create",
            &mut session,
            json!({ "message": "ancora", "amend": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::PostCommit { .. }), "{err}");
}

#[test]
fn amend_of_a_merge_keeps_both_parents() {
    let h = repo_with_history();
    let mut backend = h.repo.open();
    let out = backend
        .commit(&CommitRequest::new("merge con un messaggio migliore").amend())
        .unwrap();
    assert_eq!(out.previous, Some(h.m));
    assert_eq!(out.commit.parents, [h.c2, h.f2]);
    assert_eq!(h.repo.rev(out.backup.as_deref().unwrap()), h.m);
}

#[cfg(unix)]
#[test]
fn failing_pre_commit_hook_creates_nothing() {
    let repo = repo_with_staged_file();
    hook(
        &repo,
        "pre-commit",
        "echo 'lint: 3 errori'\necho 'dettagli su stderr' >&2\nexit 1",
    );
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let mut backend = repo.open();
    let err = backend.commit(&CommitRequest::new("x")).unwrap_err();
    let EngineError::Git { command, stderr } = &err else {
        panic!("{err}");
    };
    assert_eq!(command, "commit");
    assert!(stderr.contains("lint: 3 errori"), "{stderr}");
    assert!(stderr.contains("dettagli su stderr"), "{stderr}");
    assert_unchanged(&repo, &head, &before);
    assert!(backend.status().unwrap().entries[0].is_staged());
}

#[cfg(unix)]
#[test]
fn failing_hook_during_amend_removes_the_backup() {
    let repo = repo_with_staged_file();
    hook(&repo, "pre-commit", "exit 1");
    let head = repo.git(&["rev-parse", "HEAD"]);
    let mut backend = repo.open();
    let err = backend
        .commit(&CommitRequest::new("x").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err}");
    assert!(err.to_string().contains("senza messaggi"), "{err}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    // HEAD non è cambiato: il backup non serve e non resta.
    assert_eq!(repo.git(&["for-each-ref", "refs/project/"]), "");
}

#[cfg(unix)]
#[test]
fn commit_msg_hook_can_reject() {
    let repo = repo_with_staged_file();
    hook(
        &repo,
        "commit-msg",
        "grep -q '^JIRA-' \"$1\" || { echo 'manca il codice JIRA'; exit 1; }",
    );
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let mut backend = repo.open();
    let err = backend
        .commit(&CommitRequest::new("senza codice"))
        .unwrap_err();
    assert!(err.to_string().contains("manca il codice JIRA"), "{err}");
    assert_unchanged(&repo, &head, &before);
    backend
        .commit(&CommitRequest::new("JIRA-1 con codice"))
        .unwrap();
    assert_eq!(last_message(&repo), "JIRA-1 con codice");
}

#[cfg(unix)]
#[test]
fn prepare_commit_msg_hook_changes_the_message() {
    let repo = repo_with_staged_file();
    hook(
        &repo,
        "prepare-commit-msg",
        "printf '\\nAggiunto-da: hook\\n' >> \"$1\"",
    );
    let mut backend = repo.open();
    let out = backend.commit(&CommitRequest::new("titolo")).unwrap();
    assert_eq!(last_message(&repo), "titolo\n\nAggiunto-da: hook");
    // Il risultato riporta il messaggio come scritto nel commit.
    assert_eq!(out.commit.body, "Aggiunto-da: hook");
}

#[cfg(unix)]
#[test]
fn hooks_with_large_output_do_not_block() {
    let repo = repo_with_staged_file();
    // Circa 2 MB su stdout e su stderr, più di qualunque buffer di una pipe.
    let big = "i=0; while [ $i -lt 20000 ]; do \
               echo \"riga $i di output dell'hook, abbastanza lunga da riempire la pipe\"; \
               echo \"errore $i dell'hook, abbastanza lungo da riempire la pipe\" >&2; \
               i=$((i+1)); done";
    hook(&repo, "pre-commit", big);
    let mut backend = repo.open();
    backend.commit(&CommitRequest::new("rumoroso")).unwrap();
    assert_eq!(last_message(&repo), "rumoroso");

    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    hook(
        &repo,
        "pre-commit",
        &format!("{big}\necho 'ultima riga'\nexit 1"),
    );
    let err = backend
        .commit(&CommitRequest::new("rifiutato"))
        .unwrap_err();
    assert!(err.to_string().contains("ultima riga"), "...");
    assert!(err.to_string().contains("errore 19999"), "...");
}

#[cfg(unix)]
#[test]
fn pre_commit_hook_that_stages_files_is_respected() {
    // Documentato: un pre-commit che cambia l'indice (es. un formattatore) cambia quello che
    // viene committato, come con `git commit`.
    let repo = repo_with_staged_file();
    repo.write("generato.txt", "g\n");
    hook(&repo, "pre-commit", "git add generato.txt");
    let mut backend = repo.open();
    backend.commit(&CommitRequest::new("con hook")).unwrap();
    assert_eq!(
        repo.git(&["show", "--format=", "--name-only", "HEAD"]),
        "b.txt\ngenerato.txt"
    );
}

#[test]
fn signing_that_fails_creates_nothing() {
    let repo = repo_with_staged_file();
    repo.git(&["config", "commit.gpgSign", "true"]);
    repo.git(&["config", "gpg.program", "false"]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let mut backend = repo.open();
    let err = backend.commit(&CommitRequest::new("firmato")).unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err}");
    assert!(err.to_string().contains("gpg failed to sign"), "{err}");
    assert_unchanged(&repo, &head, &before);
}

#[test]
fn unresolved_conflicts_are_refused_and_merge_is_concluded() {
    let repo = repo_with_merge_conflict();
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let mut backend = repo.open();
    assert_eq!(backend.status().unwrap().operation, Some(Operation::Merge));
    let err = backend.commit(&CommitRequest::new("merge")).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    assert_unchanged(&repo, &head, &before);

    // Risolto il conflitto, il commit è il commit di merge e conclude il merge. Non si fa
    // `amend` durante il merge.
    repo.write("a.txt", "risolto\n");
    backend.stage(&[RepoPath::from("a.txt")]).unwrap();
    let err = backend
        .commit(&CommitRequest::new("merge").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let out = backend
        .commit(&CommitRequest::new("Merge di altro"))
        .unwrap();
    assert_eq!(out.commit.parents.len(), 2);
    assert_eq!(out.commit.parents[1], repo.rev("altro"));
    assert_eq!(backend.status().unwrap().operation, None);
    assert_eq!(last_message(&repo), "Merge di altro");
}

#[test]
fn rebase_and_cherry_pick_in_progress_are_refused() {
    let (repo, theirs) = common::repo_with_divergent_branches();
    repo.git_fails(&["cherry-pick", &theirs.to_string()]);
    repo.write("a.txt", "risolto\n");
    repo.stage(&["a.txt"]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let mut backend = repo.open();
    assert_eq!(
        backend.status().unwrap().operation,
        Some(Operation::CherryPick)
    );
    let err = backend.commit(&CommitRequest::new("x")).unwrap_err();
    assert!(err.to_string().contains("cherry-pick"), "{err}");
    assert_unchanged(&repo, &head, &before);
    repo.git(&["cherry-pick", "--abort"]);

    repo.git_fails(&["rebase", "altro"]);
    repo.write("a.txt", "risolto\n");
    repo.stage(&["a.txt"]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    assert_eq!(backend.status().unwrap().operation, Some(Operation::Rebase));
    let err = backend.commit(&CommitRequest::new("x")).unwrap_err();
    assert!(err.to_string().contains("rebase"), "{err}");
    assert_unchanged(&repo, &head, &before);
}

#[test]
fn hostile_configuration_is_neutralized() {
    let repo = repo_with_staged_file();
    let template = repo.path().join("modello.txt");
    std::fs::write(&template, "MODELLO\n").unwrap();
    for (key, value) in [
        // Un editor che fallisce: se git lo avviasse, il commit fallirebbe.
        ("core.editor", "false"),
        ("commit.verbose", "true"),
        ("commit.template", template.to_str().unwrap()),
        ("commit.status", "true"),
        ("i18n.commitEncoding", "ISO-8859-1"),
    ] {
        repo.git(&["config", key, value]);
    }
    let mut backend = repo.open();
    backend
        .commit(&CommitRequest::new("Messaggio è UTF-8"))
        .unwrap();
    assert_eq!(last_message(&repo), "Messaggio è UTF-8");
    // Nessuna intestazione `encoding`: il messaggio è registrato come UTF-8.
    let raw = repo.git(&["cat-file", "commit", "HEAD"]);
    assert!(!raw.contains("\nencoding "), "{raw}");

    // `amend` con la stessa configurazione.
    backend
        .commit(&CommitRequest::new("Corretto").amend())
        .unwrap();
    assert_eq!(last_message(&repo), "Corretto");
}

#[test]
fn locked_index_is_reported() {
    let repo = repo_with_staged_file();
    std::fs::write(repo.path().join(".git/index.lock"), "").unwrap();
    let head = repo.git(&["rev-parse", "HEAD"]);
    let mut backend = repo.open();
    let err = backend.commit(&CommitRequest::new("x")).unwrap_err();
    assert!(matches!(err, EngineError::IndexLocked { .. }), "{err}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
}

#[test]
fn commit_create_command() {
    let repo = repo_with_staged_file();
    let before = repo.rev("HEAD");
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(Repo::opener()));
    registry
        .run(
            "repo.open",
            &mut session,
            json!({ "path": repo.path_str() }),
        )
        .unwrap();
    let out = registry
        .run(
            "commit.create",
            &mut session,
            json!({ "message": "Da comando\n\nCorpo." }),
        )
        .unwrap();
    assert_eq!(out["commit"]["id"], repo.rev("HEAD").to_string());
    assert_eq!(out["commit"]["summary"], "Da comando");
    assert_eq!(out["commit"]["body"], "Corpo.");
    assert_eq!(out["previous"], before.to_string());
    assert_eq!(out["branch"], "refs/heads/main");
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["status"]["entries"], json!([]));
    assert_eq!(
        out["status"],
        registry
            .run("repo.status", &mut session, json!({}))
            .unwrap()
    );
    // Il commit restituito è quello di `repo.log`.
    let log = registry
        .run("repo.log", &mut session, json!({ "limit": 1 }))
        .unwrap();
    assert_eq!(log["commits"][0], out["commit"]);

    let err = registry
        .run("commit.create", &mut session, json!({ "message": "x" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NothingToCommit), "{err}");
    let err = registry
        .run(
            "commit.create",
            &mut session,
            json!({ "message": "x", "no_verify": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");

    let out = registry
        .run(
            "commit.create",
            &mut session,
            json!({ "message": "Da comando, corretto", "amend": true }),
        )
        .unwrap();
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(repo.rev(backup).to_string(), out["previous"]);
}
