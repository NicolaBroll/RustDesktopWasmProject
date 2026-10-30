# Rules at a glance and safety checklist

To be read **always** before touching the git client, in place of all of
[ARCHITECTURE-git.md](../ARCHITECTURE-git.md): that file and the per-function files in this folder
are opened only for the part being touched (the section cited here, the function's file).

## Invariants

- Every operation is a `Registry` command with a stable id (never renamed) and JSON parameters
  `deny_unknown_fields`, deserialized by the `Registry` into the handler's type; writers are
  registered with `Access::Write`. UI, CLI, MCP and tests go through
  `Registry::run`.
- **Do not lose data** (§1, §7): anything that can delete work requires `"confirm": true` and first
  saves a backup in `refs/project/backup/<type>-...`; if the backup fails, nothing is written.
  Stage/unstage save the index backup on their own, without `confirm`.
- Whatever moves refs records a `JournalEntry` (HEAD and refs before/after); undo and redo refuse if
  the refs were changed from outside or if an operation is in progress (merge, rebase, cherry-pick,
  revert, bisect, am).
- **Calls to git** (§5): never a shell; `--end-of-options` before user-supplied revs and names,
  `--` and `--literal-pathspecs` for paths (lists via stdin with `--pathspec-file-nul`, never
  empty); messages via stdin or file; `GIT_TERMINAL_PROMPT=0`, `LC_ALL=C`; no `--autostash`,
  `--force` (only `--force-with-lease` with the expected oid), `git pull`.
- Paths stay bytes (`RepoPath`): `from_utf8_lossy` only for displayed text.
- Every command: tests on the `MemoryBackend`; every `CliBackend` method: integration tests in
  `crates/git/tests` with the scenarios in `common/`.

## Checklist before delivering a write

These are the problems reviews find most often: check them first, one by one.

1. **git fails midway.** A command that exits with an error may already have written some of the
   files or moved a ref (read-only folder, open file, lock). A backup is **never
   removed** after a failure if HEAD, the branch or the operation in progress have changed or it
   cannot be ruled out that they have; the error names the backup.
2. **User configuration.** Options that change the command's behavior
   (`branch.<b>.mergeOptions`, `pull.*`, `merge.ff`, `commit.gpgsign`, `core.hooksPath`,
   `rerere.autoUpdate`...): pass the explicit options needed (`--no-squash`,
   `--commit`, ...) and afterwards verify that the ref is really where it should be; if not, an
   error, not success.
3. **Untracked, ignored, `--assume-unchanged`, `--skip-worktree` files.** They must not be
   overwritten or lost; `git add` of an ignored path needs `-f` (with explicit paths).
4. **Races.** Between read and write another process can move refs or change files: pass
   git the expected value when possible (`update-ref <new> <old>`, lease) and re-check
   afterwards; the journal records only the expected result, otherwise a warning in `warnings`.
5. **Hooks and signing** that fail (`pre-commit`, `commit-msg`, `pre-merge-commit`,
   `post-checkout`, `gpg.program=false`): the state git leaves must be reported as it is, with git's
   text.
6. **Non-UTF-8 names**, with spaces, starting with `-`: no duplicates or alterations.
7. **Operation in progress or detached HEAD**: refuse or handle explicitly.
8. **Tests** for: the case without `confirm`, restoring from the backup, failure after the backup
   and, where it can be triggered, each of the points above. A test that depends on permissions is
   skipped if the folder stays writable (root).

## Cost: output and reports

- During development: `cargo test -q -p <crate> [--test <file>] [<filter>]`; at the end, just
  once, `cargo xtask ci --quiet` (one line per step, output only from the step that fails).
- Read large files in pieces (`grep -n` to find the spot, then the lines needed).
- Short final report: at most 15-20 lines with what changes in the contract (id, parameters,
  result fields), files touched, non-obvious choices, CI result.
