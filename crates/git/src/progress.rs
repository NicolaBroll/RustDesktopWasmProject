//! Lettura dello stderr di `git fetch --progress` (§6): righe di avanzamento ed errori dei
//! remoti. Funzioni pure, provate con il testo che git scrive con `LC_ALL=C`.

use project_engine::{EngineError, Progress};

/// Una riga di avanzamento di git, come la scrive `progress.c`: `<fase>: <n>` oppure
/// `<fase>: <p>% (<n>/<totale>)`, poi forse `, <byte> | <velocità>` e `, done.`; con il
/// prefisso `remote: ` se viene dal remoto. `None` per le altre righe (`From <url>`,
/// ` * [new branch] ...`, `remote: Total 3 (delta 0) ...`, errori).
pub(crate) fn parse_progress(line: &str) -> Option<Progress> {
    let text = clean(line);
    let (remote, body) = match text.strip_prefix("remote: ") {
        Some(rest) => (true, rest.trim_start()),
        None => (false, text.as_str()),
    };
    let (phase, rest) = body.split_once(": ")?;
    if phase.is_empty()
        || phase.starts_with(' ')
        || !phase.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
    {
        return None;
    }
    let rest = rest.trim_start();
    let done = rest.ends_with("done.") || rest.ends_with("done");
    let (percent, current, total) = match number_prefix(rest) {
        Some((n, after)) if after.starts_with('%') => {
            let percent = u8::try_from(n.min(100)).ok()?;
            let after = after[1..].trim_start();
            let (current, total) = counts(after)?;
            (Some(percent), Some(current), Some(total))
        }
        Some((n, after)) if after.is_empty() || after.starts_with(',') => (None, Some(n), None),
        _ => return None,
    };
    Some(Progress {
        phase: phase.to_owned(),
        remote,
        percent,
        current,
        total,
        done,
        text,
    })
}

/// La riga senza spazi finali e senza la sequenza `ESC [K` (cancella fino a fine riga) che git
/// aggiunge alle righe del remoto quando stderr è un terminale.
fn clean(line: &str) -> String {
    line.replace("\u{1b}[K", "").trim_end().to_owned()
}

/// Le cifre all'inizio di `text` e il resto.
fn number_prefix(text: &str) -> Option<(u64, &str)> {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let n = text[..end].parse().ok()?;
    Some((n, &text[end..]))
}

/// `(<n>/<totale>)` all'inizio di `text`.
fn counts(text: &str) -> Option<(u64, u64)> {
    let inner = text.strip_prefix('(')?;
    let (current, rest) = number_prefix(inner)?;
    let (total, rest) = number_prefix(rest.strip_prefix('/')?)?;
    rest.starts_with(')').then_some((current, total))
}

/// Testi di git (`LC_ALL=C`, minuscoli) che dicono che il remoto ha rifiutato le credenziali o
/// che servirebbe chiederle: HTTP (`could not read Username ...: terminal prompts disabled`,
/// `Authentication failed`, 401/403) e SSH (`Permission denied (publickey)`, `Host key
/// verification failed`).
const AUTH: &[&str] = &[
    "authentication failed",
    "could not read username",
    "could not read password",
    "terminal prompts disabled",
    "permission denied (publickey",
    "permission denied, please try again",
    "permission denied (keyboard-interactive",
    "permission denied (password",
    "host key verification failed",
    "returned error: 401",
    "returned error: 403",
    "http 401",
    "http 403",
    "invalid username or password",
];

/// Testi di git (`LC_ALL=C`, minuscoli) che dicono che il remoto non si raggiunge: una
/// cartella che non c'è o non è un repository, host sconosciuto, rete, repository che il server
/// non ha. Si guardano dopo [`AUTH`]: `Could not read from remote repository` segue anche un
/// `Permission denied (publickey)`.
const UNREACHABLE: &[&str] = &[
    "does not appear to be a git repository",
    "not a git repository",
    "could not resolve host",
    "could not resolve hostname",
    "name or service not known",
    "temporary failure in name resolution",
    "connection refused",
    "connection timed out",
    "operation timed out",
    "network is unreachable",
    "no route to host",
    "failed to connect",
    "unable to connect",
    "connection reset",
    "repository not found",
    "returned error: 404",
    "unable to access '",
    "could not read from remote repository",
];

/// L'errore di un `git fetch` da `remote` non riuscito, dal suo stderr (senza le righe di
/// avanzamento): [`EngineError::Auth`], [`EngineError::RemoteUnreachable`],
/// [`EngineError::NotAllowed`] per un branch che il remoto non ha (`couldn't find remote ref`),
/// altrimenti `None` (l'errore generico [`EngineError::Git`]).
pub(crate) fn remote_error(remote: &str, stderr: &str) -> Option<EngineError> {
    let lower = stderr.to_ascii_lowercase();
    let message = || stderr.trim().to_owned();
    if AUTH.iter().any(|p| lower.contains(p)) {
        return Some(EngineError::Auth {
            remote: remote.to_owned(),
            message: message(),
        });
    }
    if lower.contains("couldn't find remote ref") {
        return Some(EngineError::NotAllowed(format!(
            "il remoto `{remote}` non ha il branch chiesto, niente scaricato: {}",
            message()
        )));
    }
    if UNREACHABLE.iter().any(|p| lower.contains(p)) {
        return Some(EngineError::RemoteUnreachable {
            remote: remote.to_owned(),
            message: message(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(line: &str) -> Progress {
        parse_progress(line).unwrap_or_else(|| panic!("non letta: {line:?}"))
    }

    #[test]
    fn progress_lines_of_git_fetch() {
        let p = progress("Receiving objects:  45% (9/20), 1.20 MiB | 2.00 MiB/s");
        assert_eq!(p.phase, "Receiving objects");
        assert!(!p.remote);
        assert_eq!(
            (p.percent, p.current, p.total, p.done),
            (Some(45), Some(9), Some(20), false)
        );
        assert_eq!(
            p.text,
            "Receiving objects:  45% (9/20), 1.20 MiB | 2.00 MiB/s"
        );

        let p = progress("Resolving deltas: 100% (5/5), done.");
        assert_eq!(
            (p.percent, p.current, p.total, p.done),
            (Some(100), Some(5), Some(5), true)
        );

        let p = progress("remote: Enumerating objects: 5, done.        ");
        assert!(p.remote);
        assert_eq!(p.phase, "Enumerating objects");
        assert_eq!((p.percent, p.current, p.total), (None, Some(5), None));
        assert!(p.done);
        assert_eq!(p.text, "remote: Enumerating objects: 5, done.");

        let p = progress("remote: Counting objects:  20% (1/5)\u{1b}[K");
        assert_eq!(
            (p.percent, p.current, p.total),
            (Some(20), Some(1), Some(5))
        );
        assert!(!p.done);

        let p = progress("Unpacking objects: 100% (3/3), 254 bytes | 254.00 KiB/s, done.");
        assert_eq!(p.phase, "Unpacking objects");
        assert!(p.done);

        let p = progress("Counting objects: 1234");
        assert_eq!((p.percent, p.current), (None, Some(1234)));
    }

    #[test]
    fn other_lines_are_not_progress() {
        for line in [
            "From /tmp/remote",
            " * [new branch]      main       -> origin/main",
            "   1234567..89abcde  main       -> origin/main",
            "remote: Total 3 (delta 0), reused 0 (delta 0), pack-reused 0",
            "fatal: 'nessuno' does not appear to be a git repository",
            "error: cannot lock ref 'refs/remotes/origin/main': x",
            "warning: redirecting to https://x/",
            "remote: Create a pull request for 'x' on GitHub by visiting:",
            "",
        ] {
            assert_eq!(parse_progress(line), None, "{line:?}");
        }
    }

    #[test]
    fn credential_errors() {
        for stderr in [
            "fatal: could not read Username for 'https://example.com': terminal prompts disabled",
            "remote: Invalid username or password.\nfatal: Authentication failed for 'https://x/'",
            "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote \
             repository.\n\nPlease make sure you have the correct access rights",
            "Host key verification failed.\nfatal: Could not read from remote repository.",
            "fatal: unable to access 'https://x/r.git/': The requested URL returned error: 403",
            "fatal: unable to access 'https://x/r.git/': The requested URL returned error: 401",
        ] {
            let err = remote_error("origin", stderr).unwrap();
            assert!(
                matches!(&err, EngineError::Auth { remote, message } if remote == "origin" && message == stderr.trim()),
                "{stderr}: {err}"
            );
        }
    }

    #[test]
    fn network_and_missing_remote_errors() {
        for stderr in [
            "fatal: '/tmp/manca' does not appear to be a git repository\nfatal: Could not read \
             from remote repository.",
            "fatal: unable to access 'https://nessuno.invalid/r.git/': Could not resolve host: \
             nessuno.invalid",
            "ssh: connect to host x port 22: Connection refused\nfatal: Could not read from \
             remote repository.",
            "ssh: Could not resolve hostname x: Name or service not known",
            "remote: Repository not found.\nfatal: repository 'https://x/r.git/' not found",
            "fatal: unable to access 'https://x/': Failed to connect to x port 443",
        ] {
            let err = remote_error("origin", stderr).unwrap();
            assert!(
                matches!(err, EngineError::RemoteUnreachable { .. }),
                "{stderr}: {err}"
            );
        }
        let err = remote_error("origin", "fatal: couldn't find remote ref refs/heads/x").unwrap();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        assert!(
            remote_error(
                "origin",
                "error: cannot lock ref 'refs/remotes/origin/main'"
            )
            .is_none()
        );
    }
}
