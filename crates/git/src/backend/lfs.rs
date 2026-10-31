//! Git LFS on the [`CliBackend`] (S63, `impl-docs/git/reading.md` "Git LFS"): which paths are
//! stored in LFS (`git check-attr filter -z --stdin`), whether the repo uses LFS (`git ls-files
//! ':(attr:filter=lfs)'`) and whether git-lfs is installed (`git lfs version`). The line commands
//! refuse an LFS file: what the diff shows is the pointer text, and a pointer with some of its
//! lines changed is not a pointer any more (git-lfs would store or write that text as is).

use super::CliBackend;
use super::failure_text;
use project_domain::RepoPath;
use project_engine::{EngineError, LFS_FILTER, LfsState};

impl CliBackend {
    /// [`GitRead::lfs_paths`](project_engine::GitRead::lfs_paths).
    pub(super) fn lfs_filtered(&self, paths: &[RepoPath]) -> Result<Vec<RepoPath>, EngineError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut input = Vec::new();
        for path in paths {
            input.extend_from_slice(path.as_bytes());
            input.push(0);
        }
        let args = ["check-attr", "-z", "--stdin", "filter"];
        let out = self.run_input(&args, &input, &[])?;
        // `<path> NUL filter NUL <value> NUL` for each path, in the order given: the value is
        // matched to the requested path by position, never by the text git echoes.
        let fields: Vec<&[u8]> = out.split(|&b| b == 0).collect();
        let (triples, _) = fields.as_chunks::<3>();
        if fields.len() != paths.len() * 3 + 1 || fields.last() != Some(&&b""[..]) {
            return Err(failure_text(&args, &out));
        }
        let mut seen = std::collections::BTreeSet::new();
        Ok(paths
            .iter()
            .zip(triples.iter())
            .filter(|(path, triple)| triple[2] == LFS_FILTER && seen.insert(*path))
            .map(|(path, _)| path.clone())
            .collect())
    }

    /// [`GitRead::lfs_state`](project_engine::GitRead::lfs_state).
    pub(super) fn lfs_status(&self) -> Result<LfsState, EngineError> {
        // `attr:` pathspec magic: only the index entries whose `filter` is `lfs`.
        let args = ["ls-files", "-z", "--", ":(attr:filter=lfs)"];
        let used = !self.run(&args)?.is_empty();
        // git runs `git-lfs` from `PATH`; a missing one exits 1 ("'lfs' is not a git
        // command"). The output is checked too: an alias named `lfs` is not git-lfs.
        let version = ["lfs", "version"];
        let out = self.git.output(Some(&self.workdir), &version)?;
        let installed = out.status.success() && out.stdout.starts_with(b"git-lfs/");
        Ok(LfsState { used, installed })
    }

    /// [`EngineError::NotAllowed`] if `path` is stored in Git LFS: for `index.stage_lines`,
    /// `index.unstage_lines` and `worktree.discard_lines`.
    pub(super) fn refuse_lfs_lines(&self, path: &RepoPath) -> Result<(), EngineError> {
        if self.lfs_filtered(std::slice::from_ref(path))?.is_empty() {
            Ok(())
        } else {
            Err(EngineError::NotAllowed(msg!(
                "engine-lines-lfs",
                path = path
            )))
        }
    }
}
