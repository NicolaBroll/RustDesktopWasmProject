//! Integration tests of `CliBackend::commit` and of the `commit.create` command on real repos:
//! messages, first commit, detached HEAD, `amend` (with the backup of the rewritten commit and the
//! confirmation for a published commit), hooks, signing, conflicts and operations in progress,
//! hostile configuration.

mod common;

use common::{Repo, repo_detached_head, repo_with_history, repo_with_merge_conflict};
use project_domain::{Head, Operation, RepoPath};
use project_engine::{CommitRequest, EngineError, GitIndex, GitRead, Registry, Session};
use project_git::CliOpener;
use serde_json::{Value, json};
use std::sync::Arc;

/// The full message of the last commit, as git reads it.
fn last_message(repo: &Repo) -> String {
    // `--encoding`: with `i18n.commitEncoding` set git would re-encode the output.
    repo.git(&["log", "-1", "--encoding=UTF-8", "--format=%B"])
}

/// The index entries (`git ls-files -s`), to check that an error does not change it.
fn index(repo: &Repo) -> String {
    repo.git(&["ls-files", "-s"])
}

/// A repo with one commit (`a.txt`) and a new `b.txt` staged.
fn repo_with_staged_file() -> Repo {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "first");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    repo
}

/// Writes the hook `name` (`sh` script) in the repo's hooks directory.
#[cfg(unix)]
fn hook(repo: &Repo, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let dir = repo.path().join(".git/hooks");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Checks that the commit was not made: HEAD and index as before.
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
            "Add b: naïve UTF-8 ✓\n\nBody on\ntwo lines.\n",
        ))
        .unwrap();
    assert_eq!(
        last_message(&repo),
        "Add b: naïve UTF-8 ✓\n\nBody on\ntwo lines."
    );
    assert_eq!(out.commit.id, repo.rev("HEAD"));
    assert_eq!(out.commit.parents, [before]);
    assert_eq!(out.commit.summary, "Add b: naïve UTF-8 ✓");
    assert_eq!(out.commit.body, "Body on\ntwo lines.");
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
    // The message does not go through the command line: a message that looks like an option stays
    // a message.
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
    // Without `commit.cleanup`, with `-F` git uses `whitespace`: the `#` lines stay.
    backend
        .commit(&CommitRequest::new("\n#123 fixes  \n\n\n# note\n"))
        .unwrap();
    assert_eq!(last_message(&repo), "#123 fixes\n\n# note");

    // With `commit.cleanup=strip` a message of only comments is empty: rejected by git.
    repo.git(&["config", "commit.cleanup", "strip"]);
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let err = backend
        .commit(&CommitRequest::new("# only comments\n"))
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
    let err = backend.commit(&CommitRequest::new("first")).unwrap_err();
    assert!(matches!(err, EngineError::NothingToCommit), "{err}");
    let err = backend
        .commit(&CommitRequest::new("first").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    repo.stage(&["a.txt"]);
    let out = backend.commit(&CommitRequest::new("first")).unwrap();
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
    let out = backend.commit(&CommitRequest::new("detached")).unwrap();
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
    repo.commit_file("a.txt", "a\n", "first");
    // Only unstaged changes and untracked files.
    repo.write("a.txt", "modified\n");
    repo.write("new.txt", "?\n");
    // An `intent-to-add` file does not count, as for git.
    repo.write("ita.txt", "ita\n");
    repo.git(&["add", "-N", "ita.txt"]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let mut backend = repo.open();
    let err = backend.commit(&CommitRequest::new("nothing")).unwrap_err();
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
    // No identity in the environment and nothing to guess from the system.
    repo.git(&["config", "user.useConfigOnly", "true"]);
    let opener = CliOpener::new()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let mut backend = opener.open_cli(repo.path_str()).unwrap();
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let err = backend
        .commit(&CommitRequest::new("who am I?"))
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
    let first = repo.commit_file("a.txt", "a\n", "first");
    let second = repo.commit_file("b.txt", "b\n", "second with typo");
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
    let author = repo.git(&["log", "-1", "--format=%an %ae %ad"]);
    let mut backend = repo.open();
    let out = backend
        .commit(&CommitRequest::new("second fixed").amend())
        .unwrap();
    assert_eq!(last_message(&repo), "second fixed");
    assert_eq!(out.previous, Some(second));
    assert_eq!(out.commit.parents, [first]);
    assert_eq!(repo.git(&["rev-parse", "HEAD^{tree}"]), tree);
    // The author (and the date) stays that of the rewritten commit.
    assert_eq!(repo.git(&["log", "-1", "--format=%an %ae %ad"]), author);
    // The old HEAD is saved in a backup ref.
    let backup = out.backup.unwrap();
    assert!(backup.starts_with("refs/project/backup/amend-"), "{backup}");
    assert_eq!(repo.rev(&backup), second);
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "2");
}

#[test]
fn amend_with_new_files() {
    let repo = Repo::new();
    let first = repo.commit_file("a.txt", "a\n", "first");
    let second = repo.commit_file("b.txt", "b\n", "second");
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let mut backend = repo.open();
    let out = backend
        .commit(&CommitRequest::new("second, with c").amend())
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
    repo.commit_file("a.txt", "a\n", "first");
    let published = repo.commit_file("b.txt", "b\n", "published");
    let origin = Repo::bare();
    repo.add_remote("origin", &origin);
    repo.push("origin", "main", "main");
    let backups_before = repo.git(&["for-each-ref", "refs/project/"]);
    let before = index(&repo);
    let mut backend = repo.open();

    let err = backend
        .commit(&CommitRequest::new("rewritten").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert!(
        err.to_string().contains("refs/remotes/origin/main"),
        "{err}"
    );
    assert_unchanged(&repo, &published.to_string(), &before);
    // No backup saved for a commit that was not made.
    assert_eq!(repo.git(&["for-each-ref", "refs/project/"]), backups_before);

    let out = backend
        .commit(&CommitRequest::new("rewritten").amend().confirmed())
        .unwrap();
    assert_eq!(out.previous, Some(published));
    assert_eq!(repo.rev(out.backup.as_deref().unwrap()), published);
    assert_eq!(last_message(&repo), "rewritten");

    // A new commit on top of a published commit asks for nothing.
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    backend.commit(&CommitRequest::new("new")).unwrap();
}

#[test]
fn amend_with_a_tag_needs_confirm_and_reports_diverging_refs() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "first");
    let old = repo.commit_file("b.txt", "b\n", "second");
    repo.branch("other");
    let mut backend = repo.open();
    // Another branch on the commit: no confirmation, but it stays behind and it is reported.
    let out = backend
        .commit(&CommitRequest::new("second fixed").amend())
        .unwrap();
    assert_eq!(out.also_on, ["refs/heads/other"]);
    assert_eq!(repo.rev("other"), old);

    // A tag on the commit: it would stay on the old commit, confirmation needed.
    repo.tag("v1", "HEAD");
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let err = backend
        .commit(&CommitRequest::new("third attempt").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert!(err.to_string().contains("refs/tags/v1"), "{err}");
    assert_unchanged(&repo, &head, &before);
    let out = backend
        .commit(&CommitRequest::new("third attempt").amend().confirmed())
        .unwrap();
    assert_eq!(out.also_on, ["refs/tags/v1"]);
    // A new commit rewrites nothing.
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let out = backend.commit(&CommitRequest::new("new")).unwrap();
    assert!(out.also_on.is_empty());
}

#[test]
fn amend_that_produces_the_same_commit_changes_nothing() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "first");
    repo.commit_file("b.txt", "b\n", "second");
    // Fixed dates set on purpose (inherited variables are removed instead): two identical
    // `amend`s give the same commit, like two `amend`s in the same second.
    let opener = Repo::opener()
        .env("GIT_AUTHOR_DATE", "1700000000 +0200")
        .env("GIT_COMMITTER_DATE", "1700000000 +0200");
    let mut backend = opener.open_cli(repo.path_str()).unwrap();
    let first = backend.commit(&CommitRequest::new("same").amend()).unwrap();
    let backups = repo.git(&["for-each-ref", "refs/project/"]);
    let out = backend.commit(&CommitRequest::new("same").amend()).unwrap();
    assert_eq!(out.commit.id, first.commit.id);
    assert_eq!(out.previous, Some(first.commit.id));
    // Nothing changed: no new backup (it pointed to the same commit).
    assert_eq!(out.backup, None);
    assert_eq!(repo.git(&["for-each-ref", "refs/project/"]), backups);
}

#[cfg(unix)]
#[test]
fn failed_commit_that_moved_head_keeps_the_backup() {
    let repo = Repo::new();
    let first = repo.commit_file("a.txt", "a\n", "first");
    let second = repo.commit_file("b.txt", "b\n", "second");
    // The hook moves HEAD and then rejects the commit.
    hook(&repo, "pre-commit", "git reset -q --soft HEAD~1\nexit 1");
    let mut backend = repo.open();
    let err = backend
        .commit(&CommitRequest::new("x").amend())
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::Git { .. } | EngineError::GitWithReason { .. }
        ),
        "{err}"
    );
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
    let first = repo.commit_file("a.txt", "a\n", "first");
    let second = repo.commit_file("b.txt", "b\n", "second");
    // The commit succeeds, then a `post-commit` moves HEAD.
    hook(&repo, "post-commit", "git reset -q --hard HEAD~1");
    let mut backend = repo.open();
    let err = backend
        .commit(&CommitRequest::new("rewritten").amend())
        .unwrap_err();
    let EngineError::PostCommit { commit, backup, .. } = &err else {
        panic!("{err}");
    };
    // HEAD is not the created commit: it is not passed off as such.
    assert_eq!(*commit, None);
    let backup = backup.clone().unwrap();
    assert_eq!(repo.rev(&backup), second);
    assert!(err.to_string().contains(&backup), "{err}");
    assert!(err.to_string().contains("do not repeat"), "{err}");
    assert_eq!(repo.rev("HEAD"), first);

    // The same through the command: the error is `PostCommit`, not a failed commit.
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
            json!({ "message": "again", "amend": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::PostCommit { .. }), "{err}");
}

#[test]
fn amend_of_a_merge_keeps_both_parents() {
    let h = repo_with_history();
    let mut backend = h.repo.open();
    let out = backend
        .commit(&CommitRequest::new("merge with a better message").amend())
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
        "echo 'lint: 3 errors'\necho 'details on stderr' >&2\nexit 1",
    );
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let mut backend = repo.open();
    let err = backend.commit(&CommitRequest::new("x")).unwrap_err();
    let EngineError::Git { command, stderr } = &err else {
        panic!("{err}");
    };
    assert_eq!(command, "commit");
    assert!(stderr.contains("lint: 3 errors"), "{stderr}");
    assert!(stderr.contains("details on stderr"), "{stderr}");
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
    assert!(matches!(err, EngineError::GitWithReason { .. }), "{err}");
    assert!(err.to_string().contains("without messages"), "{err}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    // HEAD did not change: the backup is not needed and is not left.
    assert_eq!(repo.git(&["for-each-ref", "refs/project/"]), "");
}

#[cfg(unix)]
#[test]
fn commit_msg_hook_can_reject() {
    let repo = repo_with_staged_file();
    hook(
        &repo,
        "commit-msg",
        "grep -q '^JIRA-' \"$1\" || { echo 'missing JIRA code'; exit 1; }",
    );
    let head = repo.git(&["rev-parse", "HEAD"]);
    let before = index(&repo);
    let mut backend = repo.open();
    let err = backend
        .commit(&CommitRequest::new("without code"))
        .unwrap_err();
    assert!(err.to_string().contains("missing JIRA code"), "{err}");
    assert_unchanged(&repo, &head, &before);
    backend
        .commit(&CommitRequest::new("JIRA-1 with code"))
        .unwrap();
    assert_eq!(last_message(&repo), "JIRA-1 with code");
}

#[cfg(unix)]
#[test]
fn prepare_commit_msg_hook_changes_the_message() {
    let repo = repo_with_staged_file();
    hook(
        &repo,
        "prepare-commit-msg",
        "printf '\\nAdded-by: hook\\n' >> \"$1\"",
    );
    let mut backend = repo.open();
    let out = backend.commit(&CommitRequest::new("title")).unwrap();
    assert_eq!(last_message(&repo), "title\n\nAdded-by: hook");
    // The result reports the message as written in the commit.
    assert_eq!(out.commit.body, "Added-by: hook");
}

#[cfg(unix)]
#[test]
fn hooks_with_large_output_do_not_block() {
    let repo = repo_with_staged_file();
    // About 2 MB on stdout and on stderr, more than any pipe buffer.
    let big = "i=0; while [ $i -lt 20000 ]; do \
               echo \"line $i of hook output, long enough to fill the pipe\"; \
               echo \"error $i of the hook, long enough to fill the pipe\" >&2; \
               i=$((i+1)); done";
    hook(&repo, "pre-commit", big);
    let mut backend = repo.open();
    backend.commit(&CommitRequest::new("noisy")).unwrap();
    assert_eq!(last_message(&repo), "noisy");

    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    hook(
        &repo,
        "pre-commit",
        &format!("{big}\necho 'last line'\nexit 1"),
    );
    let err = backend.commit(&CommitRequest::new("rejected")).unwrap_err();
    assert!(err.to_string().contains("last line"), "...");
    assert!(err.to_string().contains("error 19999"), "...");
}

#[cfg(unix)]
#[test]
fn pre_commit_hook_that_stages_files_is_respected() {
    // Documented: a pre-commit that changes the index (e.g. a formatter) changes what gets
    // committed, as with `git commit`.
    let repo = repo_with_staged_file();
    repo.write("generated.txt", "g\n");
    hook(&repo, "pre-commit", "git add generated.txt");
    let mut backend = repo.open();
    backend.commit(&CommitRequest::new("with hook")).unwrap();
    assert_eq!(
        repo.git(&["show", "--format=", "--name-only", "HEAD"]),
        "b.txt\ngenerated.txt"
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
    let err = backend.commit(&CommitRequest::new("signed")).unwrap_err();
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

    // Once the conflict is resolved, the commit is the merge commit and concludes the merge. There
    // is no `amend` during a merge.
    repo.write("a.txt", "resolved\n");
    backend.stage(&[RepoPath::from("a.txt")]).unwrap();
    let err = backend
        .commit(&CommitRequest::new("merge").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let out = backend
        .commit(&CommitRequest::new("Merge of other"))
        .unwrap();
    assert_eq!(out.commit.parents.len(), 2);
    assert_eq!(out.commit.parents[1], repo.rev("other"));
    assert_eq!(backend.status().unwrap().operation, None);
    assert_eq!(last_message(&repo), "Merge of other");
}

#[test]
fn rebase_and_cherry_pick_in_progress_are_refused() {
    let (repo, theirs) = common::repo_with_divergent_branches();
    repo.git_fails(&["cherry-pick", &theirs.to_string()]);
    repo.write("a.txt", "resolved\n");
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

    repo.git_fails(&["rebase", "other"]);
    repo.write("a.txt", "resolved\n");
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
    let template = repo.path().join("template.txt");
    std::fs::write(&template, "TEMPLATE\n").unwrap();
    for (key, value) in [
        // An editor that fails: if git started it, the commit would fail.
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
        .commit(&CommitRequest::new("Message naïve UTF-8"))
        .unwrap();
    assert_eq!(last_message(&repo), "Message naïve UTF-8");
    // No `encoding` header: the message is recorded as UTF-8.
    let raw = repo.git(&["cat-file", "commit", "HEAD"]);
    assert!(!raw.contains("\nencoding "), "{raw}");

    // `amend` with the same configuration.
    backend
        .commit(&CommitRequest::new("Fixed").amend())
        .unwrap();
    assert_eq!(last_message(&repo), "Fixed");
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
            json!({ "message": "From command\n\nBody." }),
        )
        .unwrap();
    assert_eq!(out["commit"]["id"], repo.rev("HEAD").to_string());
    assert_eq!(out["commit"]["summary"], "From command");
    assert_eq!(out["commit"]["body"], "Body.");
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
    // The returned commit is the one of `repo.log`.
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
            json!({ "message": "From command, fixed", "amend": true }),
        )
        .unwrap();
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(repo.rev(backup).to_string(), out["previous"]);
}
