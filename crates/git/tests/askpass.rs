//! Credential prompts (S52, `impl-docs/git/sync.md` "Credential prompts") with a real git and a
//! dummy HTTP server on `127.0.0.1` that asks for Basic authentication.
//!
//! git starts this test binary as its askpass (`GIT_ASKPASS` = the current executable): so it
//! has its own `main` (`harness = false`), which first runs as askpass client like the apps do,
//! and otherwise runs the tests one after the other.

mod common;

use common::Repo;
use project_engine::{
    CredentialKind, CredentialPrompter, CredentialRequest, EngineError, Registry, Session,
};
use project_git::CliOpener;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

fn main() {
    if let Some(code) = project_git::askpass_client() {
        std::process::exit(code);
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let tests: &[(&str, fn())] = &[
        ("answers_reach_git", answers_reach_git),
        ("cancel_stops_the_fetch", cancel_stops_the_fetch),
        ("cancel_stops_the_clone", cancel_stops_the_clone),
        ("cancel_stops_a_tag_push", cancel_stops_a_tag_push),
        (
            "without_prompter_nothing_is_asked",
            without_prompter_nothing_is_asked,
        ),
        ("env_askpass_is_kept", env_askpass_is_kept),
        ("config_askpass_is_kept", config_askpass_is_kept),
        ("ssh_askpass_is_kept", ssh_askpass_is_kept),
        (
            "ssh_askpass_require_keeps_git_side",
            ssh_askpass_require_keeps_git_side,
        ),
        (
            "remote_deletes_leave_no_backup",
            remote_deletes_leave_no_backup,
        ),
        (
            "client_only_when_started_as_askpass",
            client_only_when_started_as_askpass,
        ),
    ];
    for (name, test) in tests {
        if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        println!("test askpass::{name} ...");
        test();
    }
}

/// An HTTP server that answers 401 with `WWW-Authenticate: Basic` to every request and records
/// the `Authorization` headers it receives.
struct AuthServer {
    url: String,
    authorizations: Arc<Mutex<Vec<String>>>,
}

impl AuthServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://127.0.0.1:{}/r.git",
            listener.local_addr().unwrap().port()
        );
        let authorizations = Arc::new(Mutex::new(Vec::new()));
        let seen = authorizations.clone();
        // Ends with the test binary.
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.strip_prefix("Authorization: ") {
                        seen.lock().unwrap().push(value.trim().to_owned());
                    }
                    line.clear();
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"t\"\r\n\
                      Content-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        Self {
            url,
            authorizations,
        }
    }

    fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().unwrap().clone()
    }
}

/// Answers with `username` and `password`, or cancels (`None`); records the requests.
struct TestPrompter {
    username: Option<&'static str>,
    password: Option<&'static str>,
    asked: Mutex<Vec<CredentialRequest>>,
}

impl TestPrompter {
    fn new(username: Option<&'static str>, password: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            username,
            password,
            asked: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<CredentialRequest> {
        self.asked.lock().unwrap().clone()
    }
}

impl CredentialPrompter for TestPrompter {
    fn ask(&self, request: CredentialRequest) -> Option<String> {
        let answer = match request.kind {
            CredentialKind::Username => self.username,
            CredentialKind::Password => self.password,
            _ => None,
        };
        self.asked.lock().unwrap().push(request);
        answer.map(str::to_owned)
    }
}

/// The test opener, without proxies (the server is local).
fn opener() -> CliOpener {
    Repo::opener().env("no_proxy", "*").env("NO_PROXY", "*")
}

fn with_prompter(prompter: &Arc<TestPrompter>) -> CliOpener {
    opener().credential_prompter(prompter.clone())
}

/// A repo with one commit and `origin` pointing to `server`, opened with `opener`.
fn open(server: &AuthServer, opener: CliOpener) -> (Repo, Registry, Session) {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "first");
    repo.git(&["remote", "add", "origin", &server.url]);
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(opener));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (repo, r, s)
}

fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The prompter gets Username and Password with the URL, git sends the answers to the server,
/// which rejects them: [`EngineError::Auth`], with no trace of the password.
fn answers_reach_git() {
    let server = AuthServer::start();
    let prompter = TestPrompter::new(Some("ada"), Some("s3cret pass"));
    let (_repo, r, mut s) = open(&server, with_prompter(&prompter));

    let err = r
        .run("remote.fetch", &mut s, json!({ "remote": "origin" }))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::Auth { remote, .. } if remote == "origin"),
        "{err}"
    );
    assert!(!err.to_string().contains("s3cret"), "{err}");
    assert!(!format!("{err:?}").contains("s3cret"), "{err:?}");

    let asked = prompter.asked();
    let kinds: Vec<_> = asked.iter().map(|a| a.kind).collect();
    assert_eq!(
        kinds,
        [CredentialKind::Username, CredentialKind::Password],
        "{asked:?}"
    );
    let host = server.url.trim_end_matches("/r.git");
    assert_eq!(asked[0].url.as_deref(), Some(host));
    assert!(asked[0].prompt.starts_with("Username for '"), "{asked:?}");
    assert_eq!(asked[0].remote.as_deref(), Some("origin"));
    assert!(asked[1].prompt.contains("ada@"), "{asked:?}");

    let expected = format!("Basic {}", base64(b"ada:s3cret pass"));
    assert!(
        server.authorizations().contains(&expected),
        "{:?}",
        server.authorizations()
    );
}

/// Cancel → [`EngineError::AuthCancelled`] after one question, nothing sent, no retry.
fn cancel_stops_the_fetch() {
    let server = AuthServer::start();
    let prompter = TestPrompter::new(None, None);
    let (repo, r, mut s) = open(&server, with_prompter(&prompter));
    let refs_before = repo.git(&["for-each-ref"]);

    let err = r
        .run("remote.fetch", &mut s, json!({ "remote": "origin" }))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::AuthCancelled { remote } if remote == "origin"),
        "{err}"
    );
    assert_eq!(prompter.asked().len(), 1);
    assert!(server.authorizations().is_empty());
    assert_eq!(repo.git(&["for-each-ref"]), refs_before);
}

/// `repo.clone` cancelled at the prompt: `AuthCancelled` named after the URL, no folder left.
fn cancel_stops_the_clone() {
    let server = AuthServer::start();
    let prompter = TestPrompter::new(None, None);
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(with_prompter(&prompter)));
    let parent = tempfile::tempdir().unwrap();
    let dest = parent.path().join("clone");
    let err = r
        .run(
            "repo.clone",
            &mut s,
            json!({ "url": server.url, "path": dest.to_str().unwrap() }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::AuthCancelled { remote } if *remote == server.url),
        "{err}"
    );
    assert_eq!(prompter.asked().len(), 1);
    assert!(!dest.exists());
}

/// `remote.push_tag` cancelled at the prompt: `AuthCancelled`, the local tag untouched.
fn cancel_stops_a_tag_push() {
    let server = AuthServer::start();
    let prompter = TestPrompter::new(Some("ada"), None);
    let (repo, r, mut s) = open(&server, with_prompter(&prompter));
    repo.tag("v1", "HEAD");
    let err = r
        .run(
            "remote.push_tag",
            &mut s,
            json!({ "remote": "origin", "name": "v1" }),
        )
        .unwrap_err();
    assert!(matches!(&err, EngineError::AuthCancelled { .. }), "{err}");
    let kinds: Vec<_> = prompter.asked().iter().map(|a| a.kind).collect();
    assert_eq!(kinds, [CredentialKind::Username, CredentialKind::Password]);
    assert!(server.authorizations().is_empty());
    assert_eq!(repo.rev("v1"), repo.rev("HEAD"));
}

/// Without a prompter (MCP, tests) git cannot ask: [`EngineError::Auth`] as before S52, nothing
/// sent to the server.
fn without_prompter_nothing_is_asked() {
    let server = AuthServer::start();
    let (_repo, r, mut s) = open(&server, opener());
    let err = r
        .run("remote.fetch", &mut s, json!({ "remote": "origin" }))
        .unwrap_err();
    assert!(matches!(&err, EngineError::Auth { .. }), "{err}");
    assert!(server.authorizations().is_empty());
}

/// An askpass the user set in `GIT_ASKPASS` stays: git runs it (here a program that does not
/// exist, so git fails with `Auth`), our prompter is never called.
fn env_askpass_is_kept() {
    let server = AuthServer::start();
    let prompter = TestPrompter::new(Some("ada"), Some("pw"));
    let opener = with_prompter(&prompter).env("GIT_ASKPASS", "/nonexistent/user-askpass");
    let (_repo, r, mut s) = open(&server, opener);
    let err = r
        .run("remote.fetch", &mut s, json!({ "remote": "origin" }))
        .unwrap_err();
    assert!(matches!(&err, EngineError::Auth { .. }), "{err}");
    assert!(err.to_string().contains("user-askpass"), "{err}");
    assert!(prompter.asked().is_empty());
}

/// Likewise an askpass configured in `core.askPass`.
fn config_askpass_is_kept() {
    let server = AuthServer::start();
    let prompter = TestPrompter::new(Some("ada"), Some("pw"));
    let (repo, r, mut s) = open(&server, with_prompter(&prompter));
    repo.git(&["config", "core.askPass", "/nonexistent/config-askpass"]);
    let err = r
        .run("remote.fetch", &mut s, json!({ "remote": "origin" }))
        .unwrap_err();
    assert!(matches!(&err, EngineError::Auth { .. }), "{err}");
    assert!(err.to_string().contains("config-askpass"), "{err}");
    assert!(prompter.asked().is_empty());
}

/// `SSH_ASKPASS` set by the user: git falls back to it for its own prompts, so neither side is
/// bridged and our prompter is never called.
fn ssh_askpass_is_kept() {
    let server = AuthServer::start();
    let prompter = TestPrompter::new(Some("ada"), Some("pw"));
    let opener = with_prompter(&prompter).env("SSH_ASKPASS", "/nonexistent/ssh-askpass");
    let (_repo, r, mut s) = open(&server, opener);
    let err = r
        .run("remote.fetch", &mut s, json!({ "remote": "origin" }))
        .unwrap_err();
    assert!(matches!(&err, EngineError::Auth { .. }), "{err}");
    assert!(err.to_string().contains("ssh-askpass"), "{err}");
    assert!(prompter.asked().is_empty());
}

/// `SSH_ASKPASS_REQUIRE` set by the user keeps only the ssh side out: git's own prompts still
/// reach the prompter.
fn ssh_askpass_require_keeps_git_side() {
    let server = AuthServer::start();
    let prompter = TestPrompter::new(None, None);
    let opener = with_prompter(&prompter).env("SSH_ASKPASS_REQUIRE", "never");
    let (_repo, r, mut s) = open(&server, opener);
    let err = r
        .run("remote.fetch", &mut s, json!({ "remote": "origin" }))
        .unwrap_err();
    assert!(matches!(&err, EngineError::AuthCancelled { .. }), "{err}");
    assert_eq!(prompter.asked().len(), 1);
}

/// `remote.delete_branch` and `remote.delete_tag` cancelled at the prompt (in the read of the
/// remote value before the backup): `AuthCancelled`, no backup ref left, local refs untouched.
fn remote_deletes_leave_no_backup() {
    let server = AuthServer::start();
    let prompter = TestPrompter::new(None, None);
    let (repo, r, mut s) = open(&server, with_prompter(&prompter));
    let head = repo.rev("HEAD").to_string();
    repo.git(&["update-ref", "refs/remotes/origin/feature", &head]);
    repo.tag("v1", "HEAD");
    let before = repo.git(&["for-each-ref"]);
    let calls = [
        (
            "remote.delete_branch",
            json!({ "remote": "origin", "branch": "feature", "expected": head, "confirm": true }),
        ),
        (
            "remote.delete_tag",
            json!({ "remote": "origin", "name": "v1", "expected": head, "confirm": true }),
        ),
    ];
    for (id, params) in calls {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(
            matches!(&err, EngineError::AuthCancelled { .. }),
            "{id}: {err}"
        );
        assert_eq!(repo.git(&["for-each-ref", "refs/project/"]), "", "{id}");
        assert_eq!(repo.git(&["for-each-ref"]), before, "{id}");
    }
    assert!(server.authorizations().is_empty());
}

/// `PROJECT_ASKPASS` alone (e.g. inherited by a hook that starts the app) does not make the
/// executable an askpass client: it starts normally. With `GIT_ASKPASS` naming it, it is one.
fn client_only_when_started_as_askpass() {
    let exe = std::env::current_exe().unwrap();
    let run = |git_askpass: Option<&std::path::Path>| {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("no-test-has-this-name")
            .env(project_git::ASKPASS_ENV, "1:0000")
            .env_remove("GIT_ASKPASS")
            .env_remove("SSH_ASKPASS");
        if let Some(path) = git_askpass {
            cmd.env("GIT_ASKPASS", path);
        }
        cmd.output().unwrap()
    };
    // Normal start: the test runner runs (no test matches the filter) and exits 0.
    let normal = run(None);
    assert!(normal.status.success(), "{normal:?}");
    // As askpass: the bridge `1:0000` does not answer, so the client exits 1.
    let client = run(Some(&exe));
    assert_eq!(client.status.code(), Some(1), "{client:?}");
    assert!(client.stdout.is_empty(), "{client:?}");
}
