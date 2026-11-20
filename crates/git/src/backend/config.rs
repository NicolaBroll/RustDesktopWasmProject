//! The git identity (`config.identity`, `config.set_identity`, `impl-docs/git/config.md`):
//! `user.name` and `user.email` read with their scope and written in the repo's or in the
//! user's global file. Only options git 2.30 has.

use super::CliBackend;
use crate::command::{Git, failure, path_from_output};
use project_engine::{
    ConfigScope, EngineError, GitConfig, Identity, IdentityElsewhere, IdentityFile,
    IdentityFileKey, IdentityKey, IdentityScope,
};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The two keys, as a `git config --get-regexp` pattern (an extended regular expression on the
/// lowercase key).
const KEYS: &str = r"^user\.(name|email)$";

/// Where a `git config` call runs.
#[derive(Debug, Clone, Copy)]
pub(super) enum Place<'a> {
    /// In the repo with this worktree (`git -C <workdir>`): the configuration as git reads it
    /// there.
    Repo(&'a Path),
    /// Outside any repository, whatever the current directory of the process is: `GIT_DIR`
    /// points to something that is not a repository ([`no_repo_git_dir`]), so git looks for no
    /// repo around the current directory. The configuration of a repo the app was started in
    /// is not read (not even to fail on a broken one), and an `includeIf "gitdir:"` that would
    /// match it does not apply.
    Outside,
}

/// A `GIT_DIR` that is not a repository and makes git run without one (`git config` reads and
/// writes the global file all the same). On Unix the null device: it exists, and can never be a
/// git directory. Elsewhere a name in the temporary folder that nobody creates: git accepts a
/// missing `GIT_DIR` whose parent exists (a missing parent is a fatal "Invalid path").
fn no_repo_git_dir(null_device: bool) -> OsString {
    if null_device {
        "/dev/null".into()
    } else {
        std::env::temp_dir()
            .join("project-git-no-repository")
            .into_os_string()
    }
}

/// Runs `git config <args>` in `place`.
fn config(git: &Git, place: Place, args: &[&str]) -> Result<std::process::Output, EngineError> {
    match place {
        Place::Repo(dir) => git.output(Some(dir), args),
        Place::Outside => config_outside(git, &no_repo_git_dir(cfg!(unix)), args),
    }
}

/// [`config`] outside any repository, with `git_dir` as the `GIT_DIR` that is not one.
fn config_outside(
    git: &Git,
    git_dir: &OsStr,
    args: &[&str],
) -> Result<std::process::Output, EngineError> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    git.output_os_env(None, &args, &[("GIT_DIR", git_dir)])
}

/// A value of one of the two keys as git lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    /// `local`, `global`, `system`, `worktree`, `command`; empty for a listing of one file.
    scope: String,
    /// The file, as git prints it after `file:` (relative to the worktree for the repo's own).
    origin: Vec<u8>,
    key: IdentityKey,
    value: String,
}

/// `git config --show-scope --show-origin --includes -z --get-regexp --end-of-options <KEYS>`:
/// every value of the two keys with the scope and the file that sets it (an included file has
/// the scope of the file that includes it), in the order git reads them. Outside a repo the
/// `local` and `worktree` records (there should be none: [`Place::Outside`]) are dropped.
fn scoped_entries(git: &Git, place: Place) -> Result<Vec<Entry>, EngineError> {
    let args = [
        "config",
        "--show-scope",
        "--show-origin",
        "--includes",
        "-z",
        "--get-regexp",
        "--end-of-options",
        KEYS,
    ];
    let out = config(git, place, &args)?;
    let mut entries = parse_entries(&listing(&args, out)?, true);
    if matches!(place, Place::Outside) {
        entries.retain(|e| !matches!(e.scope.as_str(), "local" | "worktree"));
    }
    Ok(entries)
}

/// `git config --local|--global --no-includes --show-origin -z --get-regexp --end-of-options
/// <KEYS>`: the values the file of `scope` itself holds, in file order. For `--global` it is
/// the one file git would write (`~/.gitconfig`, or the XDG one when only that exists).
fn own_entries(git: &Git, place: Place, scope: IdentityScope) -> Result<Vec<Entry>, EngineError> {
    let args = [
        "config",
        file_flag(scope),
        "--no-includes",
        "--show-origin",
        "-z",
        "--get-regexp",
        "--end-of-options",
        KEYS,
    ];
    let out = config(git, place, &args)?;
    Ok(parse_entries(&listing(&args, out)?, false))
}

/// The stdout of a listing; empty for exit code 1 without messages (no key is set, or the file
/// does not exist).
fn listing(args: &[&str], out: std::process::Output) -> Result<Vec<u8>, EngineError> {
    let none = out.status.code() == Some(1) && out.stderr.is_empty();
    if !out.status.success() && !none {
        return Err(failure(args, &out));
    }
    Ok(out.stdout)
}

/// The records of a `-z --show-origin --get-regexp` listing: `[<scope> NUL] <origin> NUL <key>
/// LF <value> NUL` (a key without a value, `[user] name`, has no LF: an empty value here). The
/// value is text (`from_utf8_lossy`: it is shown, never written back). Records of other keys
/// are skipped.
fn parse_entries(stdout: &[u8], with_scope: bool) -> Vec<Entry> {
    let mut fields = stdout.split(|&b| b == 0);
    let mut entries = Vec::new();
    loop {
        let scope = match with_scope {
            true => match fields.next() {
                Some(scope) => String::from_utf8_lossy(scope).into_owned(),
                None => break,
            },
            false => String::new(),
        };
        let (Some(origin), Some(record)) = (fields.next(), fields.next()) else {
            break;
        };
        let (key, value) = match record.iter().position(|&b| b == b'\n') {
            Some(i) => (&record[..i], &record[i + 1..]),
            None => (record, &[][..]),
        };
        let Some(key) = std::str::from_utf8(key)
            .ok()
            .and_then(IdentityKey::from_key)
        else {
            continue;
        };
        entries.push(Entry {
            scope,
            origin: origin.strip_prefix(b"file:").unwrap_or(origin).to_vec(),
            key,
            value: String::from_utf8_lossy(value).into_owned(),
        });
    }
    entries
}

/// `user.name` and `user.email` as git reads them in `place`, with the scope each comes from.
pub(super) fn read_identity(git: &Git, place: Place) -> Result<Identity, EngineError> {
    let entries = scoped_entries(git, place)?
        .into_iter()
        .map(|e| (ConfigScope::from_git(&e.scope), e.key, e.value));
    Ok(Identity::from_entries(entries, |name| {
        git.env_var(name).map(|v| v.to_string_lossy().into_owned())
    }))
}

/// The file `config.set_identity` writes for `scope`, seen from `place`: its own values and, per
/// key, the other file that decides what git reads in that scope — the last value of the scope
/// (with `include`s) comes from a file that is not the written one.
pub(super) fn read_identity_file(
    git: &Git,
    place: Place,
    scope: IdentityScope,
) -> Result<IdentityFile, EngineError> {
    let own = own_entries(git, place, scope)?;
    let scoped = scoped_entries(git, place)?;
    let dir = match place {
        Place::Repo(dir) => Some(dir),
        Place::Outside => None,
    };
    let key = |key: IdentityKey| {
        let values: Vec<&Entry> = own.iter().filter(|e| e.key == key).collect();
        let decides = scoped
            .iter()
            .rfind(|e| e.key == key && e.scope == scope.as_str());
        let elsewhere = decides
            .filter(|last| {
                !values
                    .last()
                    .is_some_and(|mine| same_file(dir, &mine.origin, &last.origin))
            })
            .map(|last| IdentityElsewhere {
                file: String::from_utf8_lossy(&last.origin).into_owned(),
                value: last.value.clone(),
            });
        IdentityFileKey {
            values: values.into_iter().map(|e| e.value.clone()).collect(),
            elsewhere,
        }
    };
    Ok(IdentityFile {
        name: key(IdentityKey::Name),
        email: key(IdentityKey::Email),
    })
}

/// Whether two origins printed by git name the same file: the same text (git prints the file of
/// a scope the same way in both listings) or, failing that, the same file once resolved
/// (relative to `dir`, links followed).
fn same_file(dir: Option<&Path>, a: &[u8], b: &[u8]) -> bool {
    if a == b {
        return true;
    }
    let resolve = |origin: &[u8]| -> Option<PathBuf> {
        let path = path_from_output(origin);
        let path = match dir {
            Some(dir) => dir.join(path),
            None => path,
        };
        path.canonicalize().ok()
    };
    matches!((resolve(a), resolve(b)), (Some(a), Some(b)) if a == b)
}

fn file_flag(scope: IdentityScope) -> &'static str {
    match scope {
        IdentityScope::Local => "--local",
        IdentityScope::Global => "--global",
    }
}

/// One key in the file of `scope`: `git config --local|--global --replace-all --end-of-options
/// <key> <value>` (every value the file has for the key becomes this one, where the last one
/// was), or `--unset-all --end-of-options <key>` for `None` (code 5, nothing to remove, is
/// success). git rewrites the file through a lock file and a rename: a failure leaves the old
/// file. Never `--system`, never `--file`.
pub(super) fn write_identity(
    git: &Git,
    place: Place,
    scope: IdentityScope,
    key: IdentityKey,
    value: Option<&str>,
) -> Result<(), EngineError> {
    let mut args = vec!["config", file_flag(scope)];
    match value {
        Some(value) => args.extend(["--replace-all", "--end-of-options", key.key(), value]),
        None => args.extend(["--unset-all", "--end-of-options", key.key()]),
    }
    let out = config(git, place, &args)?;
    let nothing_to_unset = value.is_none() && out.status.code() == Some(5);
    if out.status.success() || nothing_to_unset {
        Ok(())
    } else {
        Err(failure(&args, &out))
    }
}

impl GitConfig for CliBackend {
    fn identity(&self) -> Result<Identity, EngineError> {
        read_identity(&self.git, Place::Repo(&self.workdir))
    }

    fn identity_file(&self, scope: IdentityScope) -> Result<IdentityFile, EngineError> {
        read_identity_file(&self.git, Place::Repo(&self.workdir), scope)
    }

    fn set_local_identity(
        &mut self,
        key: IdentityKey,
        value: Option<&str>,
    ) -> Result<(), EngineError> {
        write_identity(
            &self.git,
            Place::Repo(&self.workdir),
            IdentityScope::Local,
            key,
            value,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, IdentityKey, parse_entries};

    fn entry(scope: &str, origin: &str, key: IdentityKey, value: &str) -> Entry {
        Entry {
            scope: scope.to_owned(),
            origin: origin.as_bytes().to_vec(),
            key,
            value: value.to_owned(),
        }
    }

    /// Both ways of pointing git away from any repository (the null device on Unix, a missing
    /// name in the temporary folder elsewhere) read and write the global file, and neither
    /// reads the repo these tests run in. The global file is in a temporary `HOME`.
    #[test]
    fn git_runs_outside_any_repository_with_either_git_dir() {
        use super::{KEYS, config_outside, no_repo_git_dir};
        use crate::command::Git;
        let home = tempfile::tempdir().unwrap();
        let mut git = Git::new("git");
        git.unset_env("GIT_CONFIG_GLOBAL".into());
        git.set_env("HOME".into(), home.path().into());
        git.set_env("XDG_CONFIG_HOME".into(), home.path().join("xdg").into());
        git.set_env("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        let mut dirs = vec![no_repo_git_dir(false)];
        if cfg!(unix) {
            dirs.push(no_repo_git_dir(true));
        }
        for (n, dir) in dirs.iter().enumerate() {
            assert!(!std::path::Path::new(dir).is_dir(), "{dir:?}");
            let name = format!("Name {n}");
            let write = ["config", "--global", "--replace-all", "user.name", &name];
            let out = config_outside(&git, dir, &write).unwrap();
            assert!(out.status.success(), "{dir:?}: {out:?}");
            let list = [
                "config",
                "--show-scope",
                "--show-origin",
                "-z",
                "--get-regexp",
                KEYS,
            ];
            let out = config_outside(&git, dir, &list).unwrap();
            assert!(out.status.success(), "{dir:?}: {out:?}");
            let entries = parse_entries(&out.stdout, true);
            assert_eq!(entries.len(), 1, "{entries:?}");
            assert_eq!(
                (entries[0].scope.as_str(), entries[0].value.as_str()),
                ("global", &*name)
            );
            // Inside a repository git would at least say so.
            let out = config_outside(&git, dir, &["rev-parse", "--git-dir"]).unwrap();
            assert!(!out.status.success(), "{dir:?} is taken for a repository");
        }
        let text = std::fs::read_to_string(home.path().join(".gitconfig")).unwrap();
        assert!(text.contains("name = Name"), "{text}");
    }

    #[test]
    fn parses_scoped_records() {
        use IdentityKey::{Email, Name};
        assert!(parse_entries(b"", true).is_empty());
        assert!(parse_entries(b"", false).is_empty());
        let out = b"global\0file:/h/.gitconfig\0user.name\nGlob Al\0\
                    global\0file:/h/inc.cfg\0user.email\ng@x\0\
                    local\0file:.git/config\0user.name\n-x\0\
                    command\0command line:\0user.email\ncmd@x\0";
        assert_eq!(
            parse_entries(out, true),
            [
                entry("global", "/h/.gitconfig", Name, "Glob Al"),
                entry("global", "/h/inc.cfg", Email, "g@x"),
                entry("local", ".git/config", Name, "-x"),
                entry("command", "command line:", Email, "cmd@x"),
            ]
        );
        // An empty value, a key without a value, a value with a line break, bytes that are not
        // UTF-8, another key.
        let out = b"local\0file:c\0user.email\n\0local\0file:c\0user.name\0\
                    worktree\0file:w\0user.name\na\nb\0system\0file:s\0user.name\n\xff\0\
                    local\0file:c\0user.signingkey\nK\0";
        assert_eq!(
            parse_entries(out, true),
            [
                entry("local", "c", Email, ""),
                entry("local", "c", Name, ""),
                entry("worktree", "w", Name, "a\nb"),
                entry("system", "s", Name, "\u{fffd}"),
            ]
        );
        // The listing of one file has no scope.
        let out = b"file:/h/.gitconfig\0user.name\nMe\0file:/h/.gitconfig\0user.name\nSecond\0";
        assert_eq!(
            parse_entries(out, false),
            [
                entry("", "/h/.gitconfig", Name, "Me"),
                entry("", "/h/.gitconfig", Name, "Second"),
            ]
        );
    }
}
