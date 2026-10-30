//! Askpass bridge (S52): git and ssh, which run with no terminal, ask for credentials through
//! the app's [`CredentialPrompter`].
//!
//! For each network command with a prompter, [`Bridge::start`] opens a listener on
//! `127.0.0.1:0` with a random 32-byte token, and git runs with `GIT_ASKPASS` and `SSH_ASKPASS`
//! set to the app's own executable, `SSH_ASKPASS_REQUIRE=force` and
//! `PROJECT_ASKPASS=<port>:<token>`. git (or ssh) starts that executable with the prompt as its
//! only argument: the app's `main` calls [`askpass_client`] first thing, which connects, sends
//! token and prompt, prints the answer on stdout and exits 0, or exits 1 if the user cancelled
//! (git then fails). The listener closes when the command ends.
//!
//! Protocol, on one connection per question: the client sends `<token hex>\n<prompt>` and
//! closes its write side; the bridge answers `1<answer>` or `0` (cancelled) and closes. A
//! connection with a wrong token, or that is too long or too slow, is dropped without asking
//! anything. The answers are secrets: never logged, never kept here.

use project_engine::{CredentialPrompter, CredentialRequest};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The variable with `<port>:<token>` that tells the executable to run as askpass client.
pub const ASKPASS_ENV: &str = "PROJECT_ASKPASS";

/// Bytes of the token (64 hex digits).
const TOKEN_LEN: usize = 32;
/// The longest request accepted: token, newline and prompt (an ssh host key prompt is a few
/// hundred bytes).
const MAX_REQUEST: u64 = 16 * 1024;
/// How long a client has to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the listener checks whether the command ended.
const ACCEPT_POLL: Duration = Duration::from_millis(20);

/// How to ask for credentials: the prompter and the program git starts as askpass (the app's
/// executable). Shown by `Debug` without the prompter.
#[derive(Clone)]
pub(crate) struct Askpass {
    pub(crate) program: OsString,
    pub(crate) prompter: Arc<dyn CredentialPrompter>,
}

impl std::fmt::Debug for Askpass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Askpass")
            .field("program", &self.program)
            .finish_non_exhaustive()
    }
}

/// The listener of one command; it stops when dropped (a question being asked at that moment
/// finishes in the background, its answer goes nowhere).
pub(crate) struct Bridge {
    port: u16,
    token: String,
    stop: Arc<AtomicBool>,
    /// True while the prompter is asking.
    busy: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token is a secret too.
        f.debug_struct("Bridge")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl Bridge {
    /// Starts listening for the questions of a command on `remote` (name or URL, given to the
    /// prompter). Error if the port cannot be opened or no random token can be made.
    pub(crate) fn start(
        prompter: Arc<dyn CredentialPrompter>,
        remote: Option<String>,
    ) -> std::io::Result<Self> {
        let token = new_token()?;
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread = {
            let (token, stop, busy, cancelled) =
                (token.clone(), stop.clone(), busy.clone(), cancelled.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        // Accepted after the command ended: dropped.
                        Ok(_) if stop.load(Ordering::Acquire) => break,
                        Ok((stream, _)) => {
                            busy.store(true, Ordering::Release);
                            serve(
                                stream,
                                &token,
                                prompter.as_ref(),
                                remote.as_ref(),
                                &cancelled,
                            );
                            busy.store(false, Ordering::Release);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(ACCEPT_POLL);
                        }
                        Err(_) => std::thread::sleep(ACCEPT_POLL),
                    }
                }
            })
        };
        Ok(Self {
            port,
            token,
            stop,
            busy,
            cancelled,
            thread: Some(thread),
        })
    }

    /// The value of [`ASKPASS_ENV`] for git.
    pub(crate) fn env_value(&self) -> String {
        format!("{}:{}", self.port, self.token)
    }

    /// Whether the user cancelled a question of this command.
    pub(crate) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Waits for the listener to close, but not for a question still open (git was stopped
        // while the dialog was open): the command must not wait for the user.
        let Some(thread) = self.thread.take() else {
            return;
        };
        while !thread.is_finished() && !self.busy.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(5));
        }
        if thread.is_finished() {
            let _ = thread.join();
        }
    }
}

/// A random token as hex digits, from the system's random source.
fn new_token() -> std::io::Result<String> {
    let mut bytes = [0u8; TOKEN_LEN];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Comparison that does not stop at the first different byte.
fn same_token(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Serves one connection: dropped without asking if the token is wrong, the request malformed
/// or an I/O error occurs; `cancelled` becomes true (before the reply) if the user cancelled.
fn serve(
    mut stream: TcpStream,
    token: &str,
    prompter: &dyn CredentialPrompter,
    remote: Option<&String>,
    cancelled: &AtomicBool,
) -> Option<()> {
    stream.set_nonblocking(false).ok()?;
    // One deadline for the whole request, not per read: a client that sends a byte at a time
    // does not hold the bridge.
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        stream
            .set_read_timeout(Some(left.max(Duration::from_millis(1))))
            .ok()?;
        match stream.read(&mut buf).ok()? {
            0 => break,
            n => request.extend_from_slice(&buf[..n]),
        }
        if request.len() as u64 > MAX_REQUEST {
            return None;
        }
    }
    let newline = request.iter().position(|&b| b == b'\n')?;
    if !same_token(&request[..newline], token.as_bytes()) {
        let _ = stream.shutdown(Shutdown::Both);
        return None;
    }
    let prompt = String::from_utf8_lossy(&request[newline + 1..]).into_owned();
    let answer = prompter.ask(CredentialRequest::new(prompt, remote.cloned()));
    let mut reply = Vec::new();
    match &answer {
        Some(text) => {
            reply.push(b'1');
            reply.extend_from_slice(text.as_bytes());
        }
        None => {
            cancelled.store(true, Ordering::Release);
            reply.push(b'0');
        }
    }
    let _ = stream.write_all(&reply);
    let _ = stream.shutdown(Shutdown::Both);
    Some(())
}

/// The askpass side, for the apps' `main`: if [`ASKPASS_ENV`] is set and there is exactly one
/// argument (the prompt), this process was started by git or ssh to ask for a credential. It
/// asks the app over the bridge, prints the answer and a newline on stdout and returns the
/// exit code: 0 answered, 1 cancelled or any error. `None` = normal start: run the app.
///
/// The apps call it before anything else and exit with the code it returns, without starting.
pub fn askpass_client() -> Option<i32> {
    let value = std::env::var(ASKPASS_ENV).ok()?;
    let mut args = std::env::args_os().skip(1);
    let prompt = args.next()?;
    if args.next().is_some() {
        return None;
    }
    // ssh only shows a notice (e.g. "Confirm user presence" of a FIDO key) and does not wait
    // for an answer: nothing to ask.
    if std::env::var_os("SSH_ASKPASS_PROMPT").is_some_and(|v| v == "none") {
        return Some(0);
    }
    let prompt = prompt.to_string_lossy();
    Some(match ask_bridge(&value, &prompt) {
        Some(answer) => {
            let mut out = std::io::stdout().lock();
            let written = out
                .write_all(&answer)
                .and_then(|()| out.write_all(b"\n"))
                .and_then(|()| out.flush());
            if written.is_ok() { 0 } else { 1 }
        }
        None => 1,
    })
}

/// Sends `prompt` to the bridge named by `value` (`<port>:<token>`): the answer, or `None` if
/// cancelled or on any error.
fn ask_bridge(value: &str, prompt: &str) -> Option<Vec<u8>> {
    let (port, token) = value.split_once(':')?;
    let port: u16 = port.parse().ok()?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, REQUEST_TIMEOUT).ok()?;
    let mut request = Vec::with_capacity(token.len() + 1 + prompt.len());
    request.extend_from_slice(token.as_bytes());
    request.push(b'\n');
    request.extend_from_slice(prompt.as_bytes());
    stream.write_all(&request).ok()?;
    stream.shutdown(Shutdown::Write).ok()?;
    // No timeout: the user may take a while to answer.
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).ok()?;
    match reply.split_first() {
        Some((b'1', answer)) => Some(answer.to_vec()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_engine::CredentialKind;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        asked: Mutex<Vec<CredentialRequest>>,
        answer: Option<String>,
    }

    impl CredentialPrompter for Recorder {
        fn ask(&self, request: CredentialRequest) -> Option<String> {
            self.asked.lock().unwrap().push(request);
            self.answer.clone()
        }
    }

    fn bridge(answer: Option<&str>) -> (Arc<Recorder>, Bridge) {
        let recorder = Arc::new(Recorder {
            answer: answer.map(str::to_owned),
            ..Recorder::default()
        });
        let bridge = Bridge::start(recorder.clone(), Some("origin".into())).unwrap();
        (recorder, bridge)
    }

    #[test]
    fn the_right_token_gets_the_answer() {
        let (recorder, bridge) = bridge(Some("s3cret"));
        let answer = ask_bridge(&bridge.env_value(), "Password for 'https://a@h': ");
        assert_eq!(answer.as_deref(), Some(&b"s3cret"[..]));
        assert!(!bridge.cancelled());
        let asked = recorder.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].kind, CredentialKind::Password);
        assert_eq!(asked[0].remote.as_deref(), Some("origin"));
        assert_eq!(asked[0].url.as_deref(), Some("https://a@h"));
    }

    #[test]
    fn a_wrong_token_is_dropped_without_asking() {
        let (recorder, bridge) = bridge(Some("s3cret"));
        let port = bridge.port;
        let wrong = format!("{port}:{}", "0".repeat(TOKEN_LEN * 2));
        assert_eq!(ask_bridge(&wrong, "Password: "), None);
        // A prefix of the right token is wrong too.
        let prefix = format!("{port}:{}", &bridge.token[..10]);
        assert_eq!(ask_bridge(&prefix, "Password: "), None);
        assert_eq!(ask_bridge(&format!("{port}:"), "Password: "), None);
        assert!(recorder.asked.lock().unwrap().is_empty());
        assert!(!bridge.cancelled());
        // The bridge still answers the right client afterwards.
        assert!(ask_bridge(&bridge.env_value(), "Password: ").is_some());
    }

    #[test]
    fn cancel_is_recorded() {
        let (recorder, bridge) = bridge(None);
        assert_eq!(ask_bridge(&bridge.env_value(), "Username for 'x': "), None);
        assert!(bridge.cancelled());
        assert_eq!(recorder.asked.lock().unwrap().len(), 1);
    }

    #[test]
    fn tokens_are_random_and_hidden_from_debug() {
        let (_, a) = bridge(None);
        let (_, b) = bridge(None);
        assert_eq!(a.token.len(), TOKEN_LEN * 2);
        assert_ne!(a.token, b.token);
        assert!(!format!("{a:?}").contains(&a.token));
    }

    #[test]
    fn the_listener_closes_with_the_bridge() {
        let (_, bridge) = bridge(Some("x"));
        let value = bridge.env_value();
        drop(bridge);
        assert_eq!(ask_bridge(&value, "Password: "), None);
    }
}
