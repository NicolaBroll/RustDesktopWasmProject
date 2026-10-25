//! Tags on the [`CliBackend`] (`tag.create`), following §5:
//!
//! - lightweight: `update-ref --no-deref refs/tags/<name> <id> ""`, created only if it does not
//!   exist;
//! - annotated: `git tag -a --cleanup=whitespace -F - --end-of-options <name> <id>` with the message on
//!   stdin (never on the command line: visible in `ps`, limited in size), without
//!   `-f` (git refuses an existing tag), with the user's identity and `tag.gpgSign`, like a commit;
//!   `--cleanup=whitespace` keeps the lines starting with `#` (`#123` is not a comment);
//! - the name is already checked by `check_new_tag_name` (no leading `-`) and reaches git after
//!   `--end-of-options` or as `refs/tags/<name>`.

use super::CliBackend;
use crate::command::failure;
use project_domain::Oid;
use project_engine::{EngineError, check_new_tag_name, tag_ref};

impl CliBackend {
    /// [`GitBackend::create_tag`](project_engine::GitBackend::create_tag).
    pub(super) fn create_tag_ref(
        &mut self,
        name: &str,
        target: &Oid,
        message: Option<&str>,
    ) -> Result<Oid, EngineError> {
        check_new_tag_name("tag.create", name)?;
        let full = tag_ref(name);
        let oid = target.to_string();
        let reflog = format!("project: tag.create {name} at {oid}");
        let out = match message {
            None => {
                let args = ["update-ref", "--no-deref", "-m", &reflog, &full, &oid, ""];
                (self.git.output(Some(&self.workdir), &args)?, args.to_vec())
            }
            Some(message) => {
                let args = [
                    "tag",
                    "-a",
                    "--cleanup=whitespace",
                    "-F",
                    "-",
                    "--end-of-options",
                    name,
                    &oid,
                ];
                let out =
                    self.git
                        .output_with_input(Some(&self.workdir), &args, message.as_bytes())?;
                (out, args.to_vec())
            }
        };
        let (out, args) = out;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if self.ref_value(&full)?.is_some()
                || stderr.contains("already exists")
                || stderr.contains("exists; cannot create")
            {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-tag-name-taken-git",
                    name = name.escape_debug(),
                    stderr = stderr.trim()
                )));
            }
            return Err(failure(&args, &out));
        }
        self.ref_value(&full)?
            .ok_or_else(|| EngineError::UnknownRev(full.clone()))
    }
}
