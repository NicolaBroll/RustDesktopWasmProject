//! Gestione dei remoti sul [`CliBackend`] (`remote.list`, `remote.add`, `remote.remove`, S27),
//! secondo §5:
//!
//! - lettura con un solo `git config --local -z --get-regexp` (solo il file del repo: niente
//!   configurazione globale né `include`) per `remote.<nome>.*`, `remote.pushdefault` e
//!   `branch.*.(remote|merge|pushremote)`, più `for-each-ref refs/remotes/<nome>/` e `git
//!   symbolic-ref -q refs/remotes/<nome>/HEAD`;
//! - `git remote add --end-of-options <nome> <url>`: nome e URL (già controllati dal motore)
//!   non diventano mai opzioni; git aggiunge `remote.<nome>.url` e il refspec di default; nessun
//!   fetch (`-f` mai);
//! - la rimozione **non** usa `git remote remove`: i branch remoti li toglie il motore con una
//!   transazione `update-ref --stdin` con i valori letti (`GitBackend::restore_refs`), e qui si
//!   tolgono esattamente le voci salvate nel diario: `git config --local --remove-section
//!   --end-of-options remote.<nome>`, `git config --local --fixed-value --unset-all
//!   --end-of-options <chiave> <valore>` per upstream, `pushRemote` e `remote.pushDefault`, e
//!   `git symbolic-ref --delete refs/remotes/<nome>/HEAD`. Così niente sparisce senza essere
//!   stato salvato (con `git remote remove` git sceglierebbe da sé cosa togliere);
//! - il ripristino: `git config --local --add --end-of-options <chiave> <valore>` per ogni voce
//!   e `git symbolic-ref -m <reflog> refs/remotes/<nome>/HEAD <ref>`.

use super::CliBackend;
use super::branch::regex_escape;
use crate::command::{failure, path_from_output};
use project_domain::Oid;
use project_engine::{
    ConfigEntry, EngineError, REMOTES_PREFIX, RemoteRef, RemoteSnapshot, mask_url, owned_by,
    remote_dependents,
};

impl CliBackend {
    /// [`GitBackend::remote_snapshot`](project_engine::GitBackend::remote_snapshot).
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
                return Err(EngineError::NotAllowed(format!(
                    "il remoto `{name}` ha un branch remoto con un nome non UTF-8: non si può \
                     salvare per l'undo, niente modificato"
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

    /// Le voci della configurazione del file del repo con la chiave che corrisponde a `pattern`
    /// (`git config --local -z --get-regexp`), nell'ordine del file. Un valore non UTF-8 non si
    /// potrebbe riscrivere: [`EngineError::NotAllowed`].
    fn local_config(&self, pattern: &str) -> Result<Vec<ConfigEntry>, EngineError> {
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
                    return Err(EngineError::NotAllowed(format!(
                        "il valore di `{key}` non è UTF-8: non si può salvare per l'undo, niente \
                         modificato (correggerlo con `git config`)"
                    )));
                }
            };
            entries.push(ConfigEntry { key, value });
        }
        Ok(entries)
    }

    /// [`GitBackend::create_remote`](project_engine::GitBackend::create_remote).
    pub(super) fn add_remote(&mut self, name: &str, url: &str) -> Result<(), EngineError> {
        let args = ["remote", "add", "--end-of-options", name, url];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            return Ok(());
        }
        // stderr potrebbe ripetere l'URL: mai le credenziali in un errore.
        let stderr = String::from_utf8_lossy(&out.stderr).replace(url, &mask_url(url).0);
        if stderr.contains("already exists") {
            Err(EngineError::NotAllowed(format!(
                "esiste già un remoto `{}`, niente modificato",
                name.escape_debug()
            )))
        } else if stderr.contains("not a valid remote name") {
            Err(EngineError::InvalidParams {
                id: "remote.add",
                reason: format!(
                    "git non accetta il nome del remoto `{}`: {}",
                    name.escape_debug(),
                    stderr.trim()
                ),
            })
        } else {
            let stderr = stderr.trim();
            Err(EngineError::Git {
                command: "remote".to_owned(),
                stderr: if stderr.is_empty() {
                    format!("terminato con {}", out.status)
                } else {
                    stderr.to_owned()
                },
            })
        }
    }

    /// [`GitBackend::remote_config_elsewhere`](project_engine::GitBackend::remote_config_elsewhere):
    /// tutte le fonti con gli `include` (`git config -z --show-scope --show-origin --includes
    /// --get-regexp`), meno quelle del file del repo (`--local --show-origin`, senza
    /// `include`). Un file incluso dal file del repo ha ambito `local` ma un'altra provenienza.
    /// Contano anche i file del formato vecchio `.git/remotes/<nome>` e `.git/branches/<nome>`.
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
        // `--local`: origine, chiave+valore.
        let local: Vec<Vec<u8>> = records(&["--local"])?
            .chunks(2)
            .filter_map(|c| c.first().cloned())
            .collect();
        let all = records(&["--show-scope", "--includes"])?;
        let mut out = Vec::new();
        // Il formato vecchio: `.git/remotes/<nome>` e `.git/branches/<nome>` (git li legge
        // ancora come remoti, `git config` no).
        for dir in ["remotes", "branches"] {
            let rel = format!("{dir}/{name}");
            let path = self.run(&["rev-parse", "--git-path", &rel])?;
            let path = self.workdir.join(path_from_output(&path));
            if path.is_file() {
                out.push(format!("il file `{}` (formato vecchio)", path.display()));
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

    /// [`GitBackend::remove_remote_config`](project_engine::GitBackend::remove_remote_config).
    pub(super) fn drop_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<String> {
        let name = &snapshot.name;
        let mut warnings = Vec::new();
        if let Some(target) = &snapshot.head {
            let head_ref = format!("{REMOTES_PREFIX}{name}/HEAD");
            let args = ["symbolic-ref", "--delete", "--end-of-options", &head_ref];
            if let Err(e) = self.config_call(&args, &[]) {
                warnings.push(format!(
                    "`{head_ref}` (verso `{target}`) non si è tolto: {e}"
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
                warnings.push(format!(
                    "la configurazione `remote.{name}.*` non si è tolta: {e}"
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
            // Codice 5: la voce non c'è più.
            if let Err(e) = self.config_call(&args, &[]) {
                warnings.push(format!("`{}` non si è tolto: {e}", entry.key));
            }
        }
        warnings
    }

    /// [`GitBackend::write_remote_config`](project_engine::GitBackend::write_remote_config).
    pub(super) fn put_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<String> {
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
                warnings.push(format!("`{}` non è stato riscritto: {e}", entry.key));
            }
        }
        if let Some(target) = &snapshot.head {
            let head_ref = format!("{REMOTES_PREFIX}{name}/HEAD");
            let reflog = format!("project: ripristino di {head_ref}");
            let args = ["symbolic-ref", "-m", &reflog, &head_ref, target];
            if let Err(e) = self.config_call(&args, &[]) {
                warnings.push(format!(
                    "`{head_ref}` non è stato rimesso (verso `{target}`): {e}"
                ));
            }
        }
        warnings
    }

    /// Esegue `args`; riuscito anche con il codice 5 di `git config --unset` (niente da
    /// togliere) o se stderr contiene uno dei testi `ok`.
    fn config_call(&self, args: &[&str], ok: &[&str]) -> Result<(), EngineError> {
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
