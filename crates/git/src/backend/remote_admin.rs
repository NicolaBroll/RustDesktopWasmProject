//! Remote management on the [`CliBackend`] (`remote.list`, `remote.add`, `remote.remove`, S27),
//! following §5:
//!
//! - reading with a single `git config --local -z --get-regexp` (only the repo file: no
//!   global configuration nor `include`) for `remote.<name>.*`, `remote.pushdefault` and
//!   `branch.*.(remote|merge|pushremote)`, plus `for-each-ref refs/remotes/<name>/` and `git
//!   symbolic-ref -q refs/remotes/<name>/HEAD`;
//! - `git remote add --end-of-options <name> <url>`: name and URL (already checked by the engine)
//!   never become options; git adds `remote.<name>.url` and the default refspec; no
//!   fetch (never `-f`);
//! - removal does **not** use `git remote remove`: the remote branches are removed by the engine with a
//!   `update-ref --stdin` transaction with the values read (`GitRefs::restore_refs`), and here
//!   exactly the entries saved in the journal are removed: `git config --local --remove-section
//!   --end-of-options remote.<name>`, `git config --local --fixed-value --unset-all
//!   --end-of-options <key> <value>` for upstream, `pushRemote` and `remote.pushDefault`, and
//!   `git symbolic-ref --delete refs/remotes/<name>/HEAD`. So nothing disappears without having been
//!   saved (with `git remote remove` git would choose by itself what to remove);
//! - restoring: `git config --local --add --end-of-options <key> <value>` for each entry
//!   and `git symbolic-ref -m <reflog> refs/remotes/<name>/HEAD <ref>`;
//! - changing in place (S59): `git remote set-url [--push] --end-of-options <name> <url>` (or
//!   `git config --local --unset-all` to remove a push URL on undo) and `git remote rename
//!   [--end-of-options] <from> <to>` (`--end-of-options` only for a name starting with `-`: git
//!   older than 2.41 refuses any option there; no `--no-progress`, also from 2.41).

use super::CliBackend;
use super::branch::regex_escape;
use crate::command::{failure, path_from_output};
use project_domain::Oid;
use project_engine::Message;
use project_engine::{
    ConfigEntry, EngineError, REMOTES_PREFIX, RemoteRef, RemoteSnapshot, mask_url, owned_by,
    remote_dependents,
};

impl CliBackend {
    /// [`GitRemotes::remote_snapshot`](project_engine::GitRemotes::remote_snapshot).
    pub(super) fn snapshot_remote(&self, name: &str) -> Result<RemoteSnapshot, EngineError> {
        let pattern = format!(
            "^(remote\\.{}\\.[^.]+|remote\\.pushdefault|branch\\..+\\.(remote|merge|pushremote))$",
            regex_escape(name)
        );
        let entries = self.local_config(&pattern)?;
        let own = format!("remote.{name}.");
        let config: Vec<ConfigEntry> = entries
            .iter()
            .filter(|e| e.key.strip_prefix(&own).is_some_and(|v| !v.contains('.')))
            .cloned()
            .collect();
        let push_default = entries
            .iter()
            .any(|e| e.key == "remote.pushdefault" && e.value.as_deref() == Some(name));
        let branch_entries: Vec<ConfigEntry> = entries
            .iter()
            .filter(|e| e.key.starts_with("branch."))
            .cloned()
            .collect();
        let upstreams = remote_dependents(name, &branch_entries);

        let remotes = self.remote_names()?;
        let prefix = format!("{REMOTES_PREFIX}{name}/");
        let out = self.run(&[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(symref)",
            &prefix,
        ])?;
        let mut refs = Vec::new();
        for line in out.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
            let mut fields = line.split(|&b| b == 0);
            let (Some(refname), Some(hex), symref) = (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if symref.is_some_and(|s| !s.is_empty()) {
                continue;
            }
            let Ok(refname) = std::str::from_utf8(refname) else {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-remote-ref-not-utf8",
                    name = name
                )));
            };
            if owned_by(name, &remotes, refname) {
                refs.push(RemoteRef {
                    name: refname.to_owned(),
                    target: Oid::from_hex(hex)?,
                });
            }
        }
        refs.sort();

        let head_ref = format!("{prefix}HEAD");
        let args = ["symbolic-ref", "-q", "--end-of-options", &head_ref];
        let out = self.git.output(Some(&self.workdir), &args)?;
        let head = match out.status.code() {
            Some(0) => {
                let target = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
                Some(String::from_utf8_lossy(target).into_owned())
            }
            Some(1) => None,
            _ => return Err(failure(&args, &out)),
        };
        Ok(RemoteSnapshot {
            name: name.to_owned(),
            config,
            refs,
            head,
            upstreams,
            push_default,
        })
    }

    /// The entries of the repo file configuration whose key matches `pattern`
    /// (`git config --local -z --get-regexp`), in file order. A non-UTF-8 value could not
    /// be written back: [`EngineError::NotAllowed`].
    pub(super) fn local_config(&self, pattern: &str) -> Result<Vec<ConfigEntry>, EngineError> {
        let args = [
            "config",
            "--local",
            "-z",
            "--get-regexp",
            "--end-of-options",
            pattern,
        ];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.code() == Some(1) && out.stderr.is_empty() {
            return Ok(Vec::new());
        }
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        let mut entries = Vec::new();
        for record in out.stdout.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            let (key, value) = match record.iter().position(|&b| b == b'\n') {
                Some(i) => (&record[..i], Some(&record[i + 1..])),
                None => (record, None),
            };
            let key = String::from_utf8_lossy(key).into_owned();
            let value = match value.map(|v| String::from_utf8(v.to_vec())) {
                Some(Ok(v)) => Some(v),
                None => None,
                Some(Err(_)) => {
                    return Err(EngineError::NotAllowed(msg!(
                        "engine-remote-config-not-utf8",
                        key = key
                    )));
                }
            };
            entries.push(ConfigEntry { key, value });
        }
        Ok(entries)
    }

    /// [`GitRemotes::create_remote`](project_engine::GitRemotes::create_remote).
    pub(super) fn add_remote(&mut self, name: &str, url: &str) -> Result<(), EngineError> {
        let args = ["remote", "add", "--end-of-options", name, url];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            return Ok(());
        }
        // stderr may repeat the URL: never credentials in an error.
        let stderr = masked_stderr(&out, url);
        if stderr.contains("already exists") {
            Err(EngineError::NotAllowed(msg!(
                "engine-remote-exists",
                name = name.escape_debug()
            )))
        } else if stderr.contains("not a valid remote name") {
            Err(EngineError::InvalidParams {
                id: "remote.add",
                reason: msg!(
                    "engine-remote-git-rejects-name",
                    name = name.escape_debug(),
                    stderr = stderr.trim()
                ),
            })
        } else {
            Err(remote_failure(&out, url))
        }
    }

    /// [`GitRemotes::set_remote_url`](project_engine::GitRemotes::set_remote_url).
    pub(super) fn change_remote_url(
        &mut self,
        name: &str,
        push: bool,
        url: Option<&str>,
    ) -> Result<(), EngineError> {
        let Some(url) = url else {
            let key = format!("remote.{name}.{}", if push { "pushurl" } else { "url" });
            let args = ["config", "--local", "--unset-all", "--end-of-options", &key];
            return self.config_call(&args, &[]);
        };
        let mut args = vec!["remote", "set-url"];
        if push {
            args.push("--push");
        }
        args.extend(["--end-of-options", name, url]);
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            return Ok(());
        }
        Err(remote_failure(&out, url))
    }

    /// [`GitRemotes::rename_remote`](project_engine::GitRemotes::rename_remote).
    pub(super) fn rename_cli_remote(
        &mut self,
        from: &str,
        to: &str,
    ) -> Result<Vec<Message>, EngineError> {
        // Git older than 2.41 takes the two names literally and refuses any option, even
        // `--end-of-options`; newer git parses options, so a name starting with `-` needs it.
        let mut args = vec!["remote", "rename"];
        if from.starts_with('-') || to.starts_with('-') {
            args.push("--end-of-options");
        }
        args.extend([from, to]);
        let out = self.git.output(Some(&self.workdir), &args)?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        if out.status.success() {
            return Ok(stderr
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|l| msg!("engine-remote-git-says", text = l))
                .collect());
        }
        if stderr.contains("already exists") {
            Err(EngineError::NotAllowed(msg!(
                "engine-remote-exists",
                name = to.escape_debug()
            )))
        } else if stderr.contains("not a valid remote name") {
            Err(EngineError::InvalidParams {
                id: "remote.rename",
                reason: msg!(
                    "engine-remote-git-rejects-name",
                    name = to.escape_debug(),
                    stderr = stderr.trim()
                ),
            })
        } else {
            Err(remote_failure(&out, ""))
        }
    }

    /// [`GitRemotes::remote_config_elsewhere`](project_engine::GitRemotes::remote_config_elsewhere):
    /// all sources with the `include`s (`git config -z --show-scope --show-origin --includes
    /// --get-regexp`), minus those of the repo file (`--local --show-origin`, without
    /// `include`). A file included by the repo file has scope `local` but another origin.
    /// The files of the old format `.git/remotes/<name>` and `.git/branches/<name>` also count.
    pub(super) fn remote_elsewhere(&self, name: &str) -> Result<Vec<String>, EngineError> {
        let pattern = format!("^remote\\.{}\\.[^.]+$", regex_escape(name));
        let records = |extra: &[&str]| -> Result<Vec<Vec<u8>>, EngineError> {
            let mut args = vec!["config", "-z", "--show-origin"];
            args.extend_from_slice(extra);
            args.extend(["--get-regexp", "--end-of-options", &pattern]);
            let out = self.git.output(Some(&self.workdir), &args)?;
            if out.status.code() == Some(1) && out.stderr.is_empty() {
                return Ok(Vec::new());
            }
            if !out.status.success() {
                return Err(failure(&args, &out));
            }
            Ok(out.stdout.split(|&b| b == 0).map(<[u8]>::to_vec).collect())
        };
        // `--local`: origin, key+value.
        let local: Vec<Vec<u8>> = records(&["--local"])?
            .chunks(2)
            .filter_map(|c| c.first().cloned())
            .collect();
        let all = records(&["--show-scope", "--includes"])?;
        let mut out = Vec::new();
        // The old format: `.git/remotes/<name>` and `.git/branches/<name>` (git still reads them
        // as remotes, `git config` does not).
        for dir in ["remotes", "branches"] {
            let rel = format!("{dir}/{name}");
            let path = self.run(&["rev-parse", "--git-path", &rel])?;
            let path = self.workdir.join(path_from_output(&path));
            if path.is_file() {
                out.push(format!("the file `{}` (old format)", path.display()));
            }
        }
        for chunk in all.chunks(3) {
            let [scope, origin, entry] = chunk else {
                continue;
            };
            if local.contains(origin) {
                continue;
            }
            let key = entry.split(|&b| b == b'\n').next().unwrap_or_default();
            out.push(format!(
                "`{}` ({}, {})",
                String::from_utf8_lossy(key),
                String::from_utf8_lossy(scope),
                String::from_utf8_lossy(origin)
            ));
        }
        Ok(out)
    }

    /// [`GitRemotes::remove_remote_config`](project_engine::GitRemotes::remove_remote_config).
    pub(super) fn drop_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<Message> {
        let name = &snapshot.name;
        let mut warnings = Vec::new();
        if let Some(target) = &snapshot.head {
            let head_ref = format!("{REMOTES_PREFIX}{name}/HEAD");
            let args = ["symbolic-ref", "--delete", "--end-of-options", &head_ref];
            if let Err(e) = self.config_call(&args, &[]) {
                warnings.push(msg!(
                    "engine-remote-head-not-removed",
                    head_ref = head_ref,
                    target = target,
                    error = e
                ));
            }
        }
        if !snapshot.config.is_empty() {
            let section = format!("remote.{name}");
            let args = [
                "config",
                "--local",
                "--remove-section",
                "--end-of-options",
                &section,
            ];
            if let Err(e) = self.config_call(&args, &["no such section"]) {
                warnings.push(msg!(
                    "engine-remote-config-not-removed",
                    name = name,
                    error = e
                ));
            }
        }
        let push_default = ConfigEntry {
            key: "remote.pushdefault".to_owned(),
            value: Some(name.clone()),
        };
        let extra = snapshot.push_default.then_some(&push_default);
        for entry in snapshot.upstreams.iter().chain(extra) {
            let mut args = vec!["config", "--local"];
            match &entry.value {
                Some(value) => args.extend([
                    "--fixed-value",
                    "--unset-all",
                    "--end-of-options",
                    &entry.key,
                    value,
                ]),
                None => args.extend(["--unset-all", "--end-of-options", &entry.key]),
            }
            // Code 5: the entry is gone.
            if let Err(e) = self.config_call(&args, &[]) {
                warnings.push(msg!(
                    "engine-remote-key-not-removed",
                    key = entry.key,
                    error = e
                ));
            }
        }
        warnings
    }

    /// [`GitRemotes::write_remote_config`](project_engine::GitRemotes::write_remote_config).
    pub(super) fn put_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<Message> {
        let name = &snapshot.name;
        let mut warnings = Vec::new();
        let push_default = ConfigEntry {
            key: "remote.pushDefault".to_owned(),
            value: Some(name.clone()),
        };
        let extra = snapshot.push_default.then_some(&push_default);
        for entry in snapshot
            .config
            .iter()
            .chain(&snapshot.upstreams)
            .chain(extra)
        {
            let value = entry.value.as_deref().unwrap_or("true");
            let args = [
                "config",
                "--local",
                "--add",
                "--end-of-options",
                &entry.key,
                value,
            ];
            if let Err(e) = self.config_call(&args, &[]) {
                warnings.push(msg!(
                    "engine-remote-key-not-written",
                    key = entry.key,
                    error = e
                ));
            }
        }
        if let Some(target) = &snapshot.head {
            let head_ref = format!("{REMOTES_PREFIX}{name}/HEAD");
            let reflog = format!("project: restoring {head_ref}");
            let args = ["symbolic-ref", "-m", &reflog, &head_ref, target];
            if let Err(e) = self.config_call(&args, &[]) {
                warnings.push(msg!(
                    "engine-remote-head-not-put-back",
                    head_ref = head_ref,
                    target = target,
                    error = e
                ));
            }
        }
        warnings
    }

    /// Runs `args`; succeeds also with code 5 of `git config --unset` (nothing to
    /// remove) or if stderr contains one of the `ok` texts.
    pub(super) fn config_call(&self, args: &[&str], ok: &[&str]) -> Result<(), EngineError> {
        let out = self.git.output(Some(&self.workdir), args)?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        let unset_nothing = out.status.code() == Some(5) && args.contains(&"--unset-all");
        if out.status.success() || unset_nothing || ok.iter().any(|t| stderr.contains(t)) {
            Ok(())
        } else {
            Err(failure(args, &out))
        }
    }
}

/// The stderr of a `git remote` call with `url` (if not empty) masked ([`mask_url`]).
fn masked_stderr(out: &std::process::Output, url: &str) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    if url.is_empty() {
        stderr.into_owned()
    } else {
        stderr.replace(url, &mask_url(url).0)
    }
}

/// The error of a failed `git remote` call: git's text (with `url` masked) or the exit status.
fn remote_failure(out: &std::process::Output, url: &str) -> EngineError {
    let stderr = masked_stderr(out, url);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        EngineError::GitWithReason {
            command: "remote".to_owned(),
            reason: msg!("engine-git-exit-status", status = out.status.to_string()),
        }
    } else {
        EngineError::Git {
            command: "remote".to_owned(),
            stderr: stderr.to_owned(),
        }
    }
}
