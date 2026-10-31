# Submodules

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the
`CliBackend` and undo and backup of this function. "§n" always refers to the main document.

Code: `crates/engine/src/git/submodule.rs` (types, states, refusals, plan), `crates/engine/src/builtin/submodule.rs`
(commands), `crates/engine/src/git/memory/submodule.rs` (`MemoryBackend`),
`crates/git/src/backend/submodule.rs` (`CliBackend`). Cloning with submodules: `repo.clone` with
`"submodules": true` ([clone.md](clone.md)).

## Commands

### `submodule.list`

Parameters: `{}`. Read-only. Result: `{"submodules": [SubmoduleInfo]}` in path order (empty when the
repo has none). Each `SubmoduleInfo`:

- `path`: the gitlink in the superproject's index (string, or byte array if not UTF-8);
- `name`: the name in the worktree's `.gitmodules` (`submodule.<name>.path`), `null` if `.gitmodules`
  does not map the path; `url`: `submodule.<name>.url` of the repo configuration (written by init),
  otherwise the one in `.gitmodules` as written (a relative URL stays relative);
- `recorded`: the commit recorded in the **index** (what an update checks out; equals HEAD's unless
  the gitlink is staged), `null` when conflicted; `head`: the commit checked out in the submodule,
  `null` if not checked out; `detached`: its HEAD is detached;
- `state`: `conflict` (unmerged in the superproject) > `uninitialized` (no repo at the path) >
  `local_changes` (`changes` or `untracked`) > `different_commit` (`head != recorded`) > `in_sync`;
- `changes` (tracked files modified or staged, nested submodules included), `untracked`
  (untracked files; ignored ones do not count), `operation` (`"Merge"`, `"Rebase"`, ... in progress
  inside it, or `null`), `unreferenced` (the detached HEAD has commits on no branch, tag or remote
  branch of the submodule).

### `submodule.update`

Parameters: `{"paths"?: [path], "init"?: bool, "recursive"?: bool}`. Writes. Slow: progress and
cancellation from `Session::job`; may fetch (credentials through the prompter, as `remote.fetch`).

- `paths`: submodules or folders containing them (a literal path, like a pathspec without magic);
  absent = all. An empty list or an empty path → `InvalidParams`; a path that selects no submodule →
  `NotAllowed` `engine-submodule-unknown`, nothing runs.
- `init`: also the uninitialized ones (`--init`); without it they are refused (`not_initialized`).
- `recursive`: after this level, the same command (same `init`, all paths, same checks) inside every
  updated or unchanged submodule, level by level; nested paths in the result are prefixed
  (`lib/inner`). Without it nested submodules are never touched.

**Refused per submodule** (nothing run for it; the others go on), in this order, with `reason`:
`conflict` (unmerged in the superproject), `no_mapping` (no `.gitmodules` entry: git cannot update
it), `suspicious_url` (URL starting with `-`), `not_initialized`, `operation_in_progress`,
`local_changes`, `untracked_files`, `unreferenced_commits` (they would be left behind on a detached
HEAD, reachable only from the reflog). A submodule already on the recorded commit is `unchanged`,
never refused: git would do nothing for it, whatever its worktree holds.

Result: `{"updated": [{"path", "from", "to", "backup"}], "unchanged": [path], "refused": [{"path",
"reason", "message"}], "messages": [string], "warnings": [Message]}`. `from`: HEAD before (`null`:
not checked out); `to`: HEAD re-read after; `backup`: the ref saved inside the submodule (only when
HEAD was detached); `messages`: git's non-progress lines; `warnings`: a submodule that after a
successful run is not on the recorded commit (a hook, another process).

## Steps and guarantees (`CliBackend`)

1. List: `ls-files -s -z` (mode `160000`; stages 1-3 = conflict), `git config -z --file .gitmodules
   --get-regexp` (worktree file; a missing file = no names) and `git config -z --get-regexp
   '^submodule\..*\.url$'`. A submodule is checked out when `<path>/.git` exists **and** the repo git
   opens there has its top level in that folder: an empty folder would otherwise lead git up to the
   superproject, and nothing is ever run in the superproject in its place. Inside it: `status
   --porcelain=v2 --untracked-files=normal --ignore-submodules=none` (overrides
   `status.showUntrackedFiles` and `submodule.<name>.ignore`), the operation files of its git
   directory, and when detached `rev-list --max-count=1 HEAD --not --branches --tags --remotes`.
2. Plan (`plan_update`, shared with the `MemoryBackend`): unchanged / refused / to run.
3. Backups: for each submodule to run whose HEAD is detached, `update-ref
   refs/project/backup/submodule-<s>-<ns> <head> ""` **inside the submodule** (create-only). If one
   fails the command fails and **nothing runs** (backups already written stay).
4. `git -c protocol.ext.allow=never -c protocol.fd.allow=never [-c protocol.file.allow=user]
   --literal-pathspecs submodule update [--init] --no-recommend-shallow --checkout --progress --
   <paths>` with `Git::run_network` (askpass bridge, no terminal, cancellable), the paths as bytes
   after `--`, never empty. Never `--force`, `--remote`, `--merge`, `--rebase`: `--checkout`
   overrides `submodule.<name>.update` (`merge`, `rebase`, `none`, `!command`), so the result is
   always a detached HEAD on the recorded commit, as the user asked from the UI.
5. `protocol.file.allow`: `-c protocol.file.allow=user` is passed **only when the user's
   configuration (any scope, read with `git config --get` in the repo) sets neither
   `protocol.file.allow` nor `protocol.allow`**. `user` is git's default since 2.38.1, so with a
   modern git this changes nothing; with older git (default `always`) it refuses a submodule URL
   that is a local path (CVE-2022-39253, `git submodule` clones with `GIT_PROTOCOL_FROM_USER=0`).
   A user who configured `always` keeps it, as in a terminal. The product never adds `always`; the
   tests set it as the user's configuration (`GIT_CONFIG_COUNT` on the test opener).
   `repo.clone` with `submodules` does the same, reading the configuration from the current
   directory (no repo yet); the `-c` reaches the submodule clones through `GIT_CONFIG_PARAMETERS`.
6. Afterwards HEAD of each run submodule is re-read (`to`); not on `recorded` → `warnings`.
   Failure (fetch, checkout, a `post-checkout` hook) or cancellation: if backups were saved the
   error is `engine-submodule-update-failed` with git's text and the backups (path: ref); git
   may already have checked out some submodules, which `submodule.list` shows as they are. Without
   backups the error is git's (`Auth`, `RemoteUnreachable`, `Cancelled`, `Git`).

Not handled on purpose: a race between the checks and git's checkout (another process writing in
the submodule meanwhile) cannot be prevented, `git submodule update` takes no expected value;
`to` and `warnings` report what happened. Ignored files in the submodule follow git's checkout rules:
an ignored file at a path the recorded commit tracks is overwritten, as `git checkout` does in a
terminal (git treats ignored files as expendable; the recorded commit may not even be fetched yet
when the checks run, so the overlap cannot be computed beforehand).

The `MemoryBackend` keeps the list as the test sets it (`MemoryBackend::set_submodules`): update
applies the same plan, moves HEAD to `recorded` (detached), records a backup
(`refs/project/backup/submodule-<n>`, `MemoryBackend::submodule_backups`) for a detached HEAD; nested
submodules are not modelled. A memory clone with `submodules` has them `in_sync`, without
`uninitialized`.

## Undo and backup

No `JournalEntry`: no ref of the superproject moves (the gitlinks in its index and HEAD are not
touched). Nothing to confirm: every submodule that could lose something is refused. The HEAD a
submodule had is in the result (`from`) and, when it was detached, in
`refs/project/backup/submodule-...` inside the submodule (`git -C <path> switch --detach <ref>` puts it
back); a HEAD on a branch stays on that branch, which keeps its commits.
