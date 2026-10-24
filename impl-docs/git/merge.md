# Merge and operation in progress

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

Types and checks shared by the backends in `crates/engine/src/git/merge.rs` (`MergeRequest`,
`MergeMode`, `MergeOutcome`, `MergeAborted`, `OperationState`, `check_merge_state`,
`check_merging`, `merge_source`, `clean_merge_message`); commands in
`crates/engine/src/builtin/merge.rs`; `CliBackend` in `crates/git/src/backend/merge.rs`;
`MemoryBackend` in `crates/engine/src/git/memory/merge.rs`.

## Commands

### `merge.start`

Parameters: `{"rev", "no_ff"?, "ff_only"?, "message"?}`.

Writes (`writes`), goes into the journal when the merge concludes (§7). Merges `rev` into the HEAD
branch. `rev`: a local or remote branch, a tag, an id (even abbreviated) or another rev; it is resolved
to the commit with `GitBackend::log` (a rev that does not exist → `UnknownRev`) and **only the id reaches git**,
after `--end-of-options` (a branch `-x` does not become an option). Empty or with NUL → `InvalidParams`.

- **Mode** (from `unmerged_count`): nothing to do if the branch already contains the commit
  (`up_to_date`: nothing written, no backup or journal entry); fast-forward if the branch has no commits
  that `rev` lacks, unless `no_ff`; otherwise a merge commit. `ff_only` with diverged
  branches → `NotAllowed`; `no_ff` and `ff_only` together → `InvalidParams`.
- **Message** of the merge commit: `message` (non-empty, without NUL, otherwise `InvalidParams`),
  otherwise as `git merge` (`merge_source`): `Merge branch 'x'`, `Merge remote-tracking branch
  'origin/x'`, `Merge tag 'v1'`, `Merge commit '<id>'`, with ` into <branch>` outside `main`/
  `master`. The name is looked up in git's order (`refs/<rev>`, `refs/tags/`, `refs/heads/`,
  `refs/remotes/`) and counts only if it points to the resolved commit. With fast-forward it is not needed.
- **Refusals** (`NotAllowed`, nothing changes), before calling the backend and again in the backend
  on the state read just before writing (`check_merge_state`): detached HEAD (choice: the
  merge must be done on a branch; the message suggests `branch.checkout` or `branch.create`),
  branch without commits, an operation in progress (even another merge), conflicted files.
- **Local changes** (choice, like `remote.pull`): for a merge commit **any** change
  to tracked files (staged or not) → `NotAllowed` asking to commit them or set them aside
  (`stash.push`); never `--autostash`. Reason: a merge stopped on conflict would mix the local changes
  with those of the merge, and `git merge --abort` cannot always rebuild them. Untracked files
  are fine. The fast-forward accepts changes that do not touch the files that change: git itself refuses,
  before writing, to overwrite the others (`NotAllowed` with the files, the backup is removed).
  Always refused: files the merge brings in place of an untracked or **ignored** file (git would
  overwrite them without saying so) and `--assume-unchanged` files that differ between the two commits.
- **Backup**: before writing `refs/project/backup/merge-...` on the branch as it was (removed if afterwards
  nothing changes).
- **Conflicts** (choice): the merge **stays in progress** (`MERGE_HEAD`), as in `remote.pull` with
  merge strategy ([sync.md](sync.md); the pull's rebase, instead, is aborted): it is not an error, the result has `in_progress: true`, the files in `conflicts` (those still
  unmerged and those git reports as conflicted, even if `rerere` already resolved them) and git's text in
  `message`. The branch has not moved (`after == before`) and the backup stays. You exit with
  `merge.continue` (after resolving and staging the files with `index.stage`) or with
  `merge.abort`. A merge stopped by a `pre-merge-commit` hook is also `in_progress` (without
  conflicts).

Result: `{"rev", "source": {"kind": "branch" | "remote_branch" | "tag" | "commit", "name",
"full": string | null}, "target", "branch", "strategy": "fast_forward" | "merge" | "up_to_date",
"before", "after", "ahead", "behind", "in_progress", "conflicts": [path], "message", "backup":
string | null, "warnings", "status" | null, "refs" | null, "can_undo", "can_redo", "next_undo",
"next_redo"}`.

### `merge.continue`

Parameters: `{"message"?}`.

Writes (`writes`), goes into the journal. Concludes the merge in progress with the merge commit (parents
HEAD and `MERGE_HEAD`): goes through `GitBackend::commit` (`git commit -F -`, like `commit.create`: hooks
and signing according to configuration, same errors, `PostCommit` after a successful commit). Refusals
(`NotAllowed`, nothing committed): no merge in progress, another operation in progress, **files
still conflicted** (the error lists them: they must be resolved and staged with `index.stage`).
Message: `message` (non-empty, without NUL) or the one prepared by git, `MERGE_MSG`, without the
`# Conflicts:` / `#\t<file>` block that git appends at the end (`clean_merge_message`: `git commit -F` with
`whitespace` cleanup does not remove comments, which would end up in the commit; applies with any
`core.commentChar`); without `MERGE_MSG`, `Merge commit '<id>'`. It also works for a merge started
from the terminal. `commit.create` with a merge in progress stays possible and makes the same commit, but goes
into the journal as a commit (undo = `reset --soft`); `merge.continue` is the command to use.

Result: `{"commit": Commit, "previous", "branch", "backup", "also_on", "message", "status" |
null, "refs" | null, "can_undo", "can_redo", "next_undo", "next_redo"}`.

### `merge.abort`

Parameters: `{"confirm": true}`.

Writes (`writes`), does **not** go into the journal. Cancels the merge in progress (`git merge --abort`):
index and worktree return to HEAD for the files of the merge. The resolutions made in the conflicted
files, the staged changes and those made after the merge started would be lost: it **always**
requires `"confirm": true`; without it, `ConfirmRequired` listing the conflicted and changed
files, and nothing changes. Before aborting the backend saves index and worktree in
`refs/project/backup/merge-abort-...` (if the backup fails nothing is aborted; see below).
Refusals (`NotAllowed`): no merge in progress, another operation (rebase, cherry-pick...), a
staged file that also has unstaged changes (`reset --merge` would refuse it: it is checked first,
without backup or writes, and the message suggests staging or discarding them; same in the
two backends). If `git merge --abort` (i.e. `reset --merge`) fails, the error is `Git` with git's
text and **names the backup, which always stays**: even when git refuses, it may already have
rewritten part of the files (a file that cannot be deleted, a read-only directory, a file
open on Windows) with `MERGE_HEAD` and HEAD unchanged. If the state changes between the check and git
(`not uptodate`), the error also has the same suggestion.

Result: `{"backup", "files": [path], "warnings", "status" | null, "refs" | null,
"can_undo", "can_redo", "next_undo", "next_redo"}` (`files`: the files saved because they differ from
HEAD).

### `repo.operation`

Parameters: `{}`. Read-only: what the UI needs for the banner of an operation in progress.

Result: `{"operation": "Merge" | "Rebase" | "CherryPick" | "Revert" | "Bisect" | "Am" | null,
"head": Head, "conflicts": [path], "incoming": [{"id", "label", "summary": string | null,
"refs": [full name]}], "message": string | null, "onto": oid | null, "rebase_branch": string |
null, "ready": bool, "continue": string | null, "abort": string | null}`.

- `operation`: like `Status::operation` (`repo.status`), from the git directory files.
- `conflicts`: the conflicted files of the status (`u` entries).
- `incoming`: the incoming commits (`MERGE_HEAD`, even more than one for an octopus,
  `CHERRY_PICK_HEAD`, `REVERT_HEAD`; empty for rebase, bisect, `git am`): `label` the short name of the
  first local branch, remote branch or tag pointing to it (otherwise the abbreviated id), `summary`
  the commit subject, `refs` the refs (full names) pointing to it.
- `message`: the message `merge.continue` would use (`MERGE_MSG` without the conflicts
  block; also for cherry-pick and revert, where git prepares it).
- `onto`, `rebase_branch`: for a rebase, the commit it is reapplied onto and the rewritten branch
  (`rebase-merge/` or `rebase-apply/`: `onto` and `head-name`; `null` if started from detached HEAD).
- `ready`: there is an operation and no conflicted files remain.
- `continue` / `abort`: the ids of the commands that conclude or cancel the operation from here:
  `merge.continue` / `merge.abort` for the merge, `cherry_pick.continue` / `cherry_pick.abort` for
  a cherry-pick, `revert.continue` / `revert.abort` for a revert ([cherry-pick.md](cherry-pick.md));
  `null` for the others (rebase in S32: from the terminal for now) and for a multi-commit cherry-pick or
  revert sequence started from the terminal (`.git/sequencer`, `sequencer` field of
  `OperationState`), which stays with the terminal.

The UI banner: `operation` not `null` → banner with the type, `incoming[].label`, the number of
`conflicts` (and the files, which open like changes), and the `continue` (active if `ready`)
and `abort` buttons (in two steps: without `confirm` the engine responds `ConfirmRequired` with the reason to
show). Re-read after every command that writes (in particular `merge.start` with `in_progress`,
`index.stage`, `merge.continue`, `merge.abort`).

## CliBackend

**Merge** (`merge.start`): reads the state without fsmonitor (`check_merge_state`), HEAD on the branch
with the expected value, `--assume-unchanged` files and files arriving in place of an untracked or
ignored one (`diff-tree --diff-filter=A HEAD <oid>` and `symlink_metadata`, like the pull), then the backup
and, with `GIT_REFLOG_ACTION=project: merge.start` and `GIT_EDITOR=:`:

- fast-forward: `git -c core.fsmonitor=false merge --ff-only --no-squash --commit --no-autostash
  --no-overwrite-ignore --no-edit --end-of-options <oid>` (`--no-squash`/`--commit` against
  `branch.<name>.mergeOptions=--squash`, which would squash without moving the branch);
- merge commit: `git -c core.fsmonitor=false merge --no-autostash --no-overwrite-ignore
  --no-edit --commit --no-squash --ff|--no-ff -F <file> --end-of-options <oid>`, the message from a
  temporary file in the git directory (git does not read `-F -` from stdin for `merge`; the path as
  `OsStr`). Explicit `--ff`/`--no-ff`, `--commit`, `--no-squash` against `merge.ff` and
  `branch.<name>.mergeOptions`; `--no-autostash` against `merge.autoStash`. Hooks
  (`pre-merge-commit`, `commit-msg`, `post-merge`) and signing follow the configuration: never
  `--no-verify`.

Afterwards: succeeded and with no operation in progress, but with the branch still at its previous value → `Git`
error naming the backup (git did something other than the merge requested); succeeded → the branch afterwards
(`ref_value`), with `warnings` if HEAD
is no longer on the branch or the branch does not contain the merged commit (a hook or another process); merge
in progress (`MERGE_HEAD`) → `in_progress` with the unmerged files of the status and those of the output
(`Merge conflict in <file>`, and with `rerere` `Resolved '<file>' using previous resolution`), no
`--abort`; otherwise nothing started: the backup is removed (only if the branch still has its previous
value) and the error is `NotAllowed` with the files if git refused so as not to overwrite
(`would be overwritten`, `would be removed`, `would lose untracked files`), otherwise `Git` with
stdout and stderr.

**Abort** (`merge.abort`): status without fsmonitor and `check_merging`, index not locked
(`IndexLocked`), then the backup like a stash, built in temporary indexes (the real index, with the
conflicts, is not touched; no split index, fsmonitor or sparse index):

1. from the real index (`ls-files -s -z`) the stage-0 entries and, for conflicted files, the
   ours version (stage 2), with `update-index -z --index-info` into an empty index, `write-tree` and
   `commit-tree -p HEAD` (fixed backup identity, `--no-gpg-sign`): the index commit;
2. `read-tree` of that tree into another temporary index, then `git --literal-pathspecs add -A
   -f --pathspec-from-file=- --pathspec-file-nul` (`-f`: a conflict on an ignored path,
   e.g. a tracked `*.log`, deleted by us and modified by them, does not block the backup) of the conflicted
   or changed files (staged or not)
   that exist on disk or in the index (deleted ones are removed), `write-tree`, `commit-tree -p HEAD
   -p <index commit>`: the worktree commit, with the markers and resolutions as they were;
3. `update-ref` of the new ref `refs/project/backup/merge-abort-<s>-<ns>` (only if it does not exist).

It is recovered with `git stash apply [--index] <ref>` (it has the shape of a stash) or file by file with
`git show <ref>:<path>`. Then `git -c core.fsmonitor=false merge --abort` with
`GIT_REFLOG_ACTION=project: merge.abort`; if it fails (or the merge stays in progress) → `Git` naming
the backup, which is never removed: `reset --merge` may already have rewritten part of the files
even with `MERGE_HEAD` and HEAD unchanged. Limit: untracked files are not in the backup (`git merge --abort` does not
touch them); between the backup and `merge --abort` there remains the instant in which another process could
change a file (which `reset --merge` would bring back to HEAD without it being in the backup).

**Read** (`repo.operation`): the operation as for `repo.status` (`MERGE_HEAD`,
`rebase-apply/applying`, `rebase-merge`/`rebase-apply`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`,
`sequencer/`, `BISECT_LOG`), then from this worktree's git directory: the ids of `MERGE_HEAD` /
`CHERRY_PICK_HEAD` / `REVERT_HEAD` (one per line), `MERGE_MSG`, and for the rebase `onto` and
`head-name` of `rebase-merge/` or `rebase-apply/` (`head-name` counts only if it starts with `refs/`).

**Continue**: `GitBackend::commit` (see [commit.md](commit.md)); with `MERGE_HEAD` git creates the
merge commit and removes the merge state.

## Undo and backup

- **Concluded merge** (`merge.start` with fast-forward or merge commit, `merge.continue`): `JournalEntry::merge`
  entry with the HEAD branch from `before` to `after`, HEAD equal before and after, the
  backup `refs/project/backup/merge-...` (only `merge.start`; for `merge.continue` the previous
  commit is the first parent of the merge) and the description `Merge of <name> into `<branch>`
  (fast-forward | merge commit: from a1b2c3d to e4f5a6b)`. Undo and redo like the pull of the active
  branch (see [sync.md](sync.md)): `GitBackend::reset_branch`, i.e. `git reset --keep` with the
  same checks (HEAD and branch as the merge left them, otherwise `UndoConflict`; files to
  restore with local changes → `NotAllowed`, the entry stays) and the backup of the abandoned merge commit
  in `refs/project/backup/undo-...`. `merge.continue` with detached HEAD (merge started
  from the terminal) goes into the journal as a commit (`JournalEntry::commit`).
- **Only an expected result**: `merge.start` records the entry only if the branch afterwards is the
  merged commit (fast-forward) or a merge commit that has the previous branch as its first parent;
  otherwise (a `post-merge` hook or another process moved it in the meantime) the merge is
  done but does not go into the journal, with a warning in `warnings` naming the backup: an undo
  would also roll back the work of others.
- With an operation in progress the undo and redo of `worktree.discard` are also refused
  (`NotAllowed`), like those that move refs: they would rewrite merge files.
- **Merge stopped on conflict: nothing in the journal** (choice). The branch has not moved; while the
  merge is in progress `edit.undo` and `edit.redo` are refused anyway (`NotAllowed`, operation in
  progress), and the entry arrives with `merge.continue`. This way an undo can never land in the middle of a
  merge, and the way out of the merge in progress is only one: `merge.abort`.
- **`merge.abort` does not go into the journal** (choice): redoing an aborted merge would mean redoing
  the merge with the conflicts and putting back the resolutions, not a ref move. Recovery is the
  backup `refs/project/backup/merge-abort-...` (above). `can_undo` after an abort refers to the previous
  entry.
- `refs/project/backup/merge-...` of a merge left in progress remains even after `merge.abort`: it is
  the branch before the merge, equal to HEAD; a future backup cleanup may remove it. The same
  for `refs/project/backup/pull-...` of a pull in merge left in progress.
- **Merge started by `remote.pull`**: `merge.continue` and `merge.abort` work the same way
  (the merge in progress is a normal `MERGE_HEAD`); the `merge.continue` message is the pull's
  (`Merge remote-tracking branch 'origin/main'`) and the journal entry `Merge of origin/main into
  `main``.

The `MemoryBackend` does the same file-by-file merge as the in-memory pull (a file changed differently
on the two sides is a conflict), but **leaves the merge in progress**: merged files staged, the
conflicts as `u` entries (`BothModified`, `BothAdded`, `DeletedByUs`, `DeletedByThem`) with the
markers `<<<<<<< HEAD` / `=======` / `>>>>>>> <name>` in the worktree if the contents are known, and
`MERGE_HEAD`/`MERGE_MSG` (with the `# Conflicts:` block) in memory. A commit with a merge in progress
makes the merge commit (parents HEAD and `MERGE_HEAD`, even without changes); a merge set up with
`set_operation` has no `MERGE_HEAD` and the commit stays refused. The abort saves a backup commit
(`refs/project/backup/merge-abort-<n>`) with the same shape as a stash (parents HEAD and a commit
with the index; the worktree of the changed files) and
returns all tracked files to HEAD (stricter than git, which keeps the unstaged changes of the
files the merge does not touch: here they are only in the backup); it refuses, like git, a staged file with
unstaged changes too. Merge backups in `commit_backups` (`refs/project/backup/merge-<n>`).
