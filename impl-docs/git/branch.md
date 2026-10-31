# Branch

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

## Commands

### `branch.create`

Parameters: `{"name", "start"?, "track"?}`.

Creates `refs/heads/<name>` at `start` (a rev; HEAD if missing, `NotAllowed` error if HEAD has no
commits) without switching branch and, without `track`, without upstream; never over an existing ref (`NotAllowed`). New name
checked (`check_new_branch_name` and, in the `CliBackend`, `git check-ref-format --branch`, which
must return the identical name): no leading `-`, `HEAD`, `@`, full `refs/...` names →
`InvalidParams`. Result: `{"branch", "target", "upstream", "warnings", "refs", "can_undo",
"can_redo", "next_undo", "next_redo"}` (see "Undo and backup" for all `branch.*`).

`"track": true` (like `git branch --track`): `start` is required and must be a remote-tracking
branch `<remote>/<branch>` (or `refs/remotes/<remote>/<branch>`) of a configured remote
(`resolve_tracking`), otherwise `InvalidParams`: a local branch, a tag, HEAD, an id, `origin/HEAD`
(symbolic), a ref of no configured remote. The branch starts at the commit of that ref (read from
`repo.refs`, not resolved as a rev). A short `start` that git would resolve first to
`refs/tags/<start>` or `refs/heads/<start>` is ambiguous → `InvalidParams` (the full name works). A
remote whose name contains `/` is recognized; a ref that two configured remotes could own (`a` and
`a/b` for `a/b/c`) → `InvalidParams` (git refuses it too). After the ref, `branch.<name>.remote =
<remote>` and `branch.<name>.merge = refs/heads/<branch>` (`GitRefs::write_upstream`); if that
write fails the new ref is deleted again (`restore_refs` with the expected value) and the error
says so (or that the deletion failed too): nothing half-done, nothing in the journal. `upstream`
in the result: `"<remote>/<branch>"`, `null` without `track`.

### `branch.set_upstream`

Parameters: `{"name", "upstream": "<remote>/<branch>" | null}` (`upstream` **required**: a missing
field is `InvalidParams`, so a forgotten parameter never removes the upstream; `""` too).

Sets (or with `null` removes) the upstream of the existing local branch `name` (any valid ref
name, also `-x`; missing → `NotAllowed`). `upstream` is checked as for `track` (existing
remote-tracking branch of exactly one configured remote). Only `branch.<name>.remote` and
`branch.<name>.merge` of the repo file are written (`--local`); other keys (`description`,
`rebase`, `pushRemote`...) stay. Allowed with detached HEAD and during an operation (it touches no
refs, index or worktree). Same value as now → `changed: false`, nothing written, nothing in the
journal. Result: `{"branch", "upstream", "previous", "changed", "warnings", "refs", ...journal}`
(`previous`: the upstream of the repo file before, `"<remote>/<branch>"`, `"<remote>:<merge>"` if
`merge` is not a branch, or `null`).

### `branch.checkout`

Parameters: `{"name"}`.

Switches to the local branch `name` (`git switch`, never forced). Result: `{"branch", "previous":
HeadState, "changed", "warnings", "status", ...journal}`. Local changes that would be
overwritten → `CheckoutConflict { target, files, stderr }` and nothing changes; operation in progress or
conflicted files → `NotAllowed`; already on the branch: `changed: false`. Leaving a detached HEAD
that no ref reaches: `backup` (see "Commits left behind").

### `commit.checkout`

Parameters: `{"rev"}` (S55).

Checks out the commit `rev` (full or short id, tag, branch name, any rev that leads to a commit)
with **detached HEAD** (`git switch --detach`): to try an old commit. Same checks, refusals and
`CheckoutConflict` as `branch.checkout` (never forced; operation in progress or conflicted files →
`NotAllowed`). `rev` empty, or an object that is not a commit (`HEAD^{tree}`, `HEAD:file`) →
`InvalidParams`; a rev that does not exist → `UnknownRev` (the `MemoryBackend` says
`InvalidParams`). A branch name detaches at its commit (it does not switch to the branch). HEAD
already detached at that commit → `changed: false`, nothing in the journal; on a branch whose tip
is that commit it still detaches. Result: `{"head": {"detached": "<oid>"}, "previous": HeadState,
"changed", "warnings", "backup"?, "status", "status_error"?, ...journal}`.

To keep work done on a detached HEAD ("Create a branch here"): `branch.create {"name"}` without
`start` creates the branch at the detached commit (HEAD stays detached; `branch.checkout` then
switches to it without touching files).

### `branch.delete`

Parameters: `{"name", "confirm"?, "expected"?}`.

`expected`: **full** id (40 digits, otherwise `InvalidParams`) of the commit the branch was
shown at; if the branch points elsewhere → `NotAllowed` ("it moved since it was seen:
... now points to ...") before the confirmation and the deletion, nothing changes: the confirmation is valid only
for the commit seen (without `expected` no additional check). Unmerged branch (commits not
reachable from either HEAD or its upstream) → `ConfirmRequired` without `confirm`; HEAD's branch,
one open in another worktree, operation in progress → `NotAllowed`. Always backed up. Result:
`{"branch", "target", "unmerged", "backup", "removed_config": [{"key", "value"}], "warnings",
"refs", ...journal}`.

### `branch.rename`

Parameters: `{"name", "new_name"}`.

`git branch -m` (never `-M`): reflog, upstream and HEAD follow; new name checked as in
`create`, never over an existing branch (`NotAllowed`). Result: `{"branch", "previous", "target",
"head", "warnings", "refs", ...journal}`.

### `tag.create`

Parameters: `{"name", "target"?, "message"?}`.

Creates `refs/tags/<name>` on `target` (a rev: id, branch, remote branch or tag; HEAD if missing,
`NotAllowed` if HEAD has no commits). With `message` (not empty after trimming) the tag is
**annotated** (tag object with the user's identity, signed if `tag.gpgSign` says so), without it
is **lightweight**. Never over an existing tag (`NotAllowed`). New name checked by
`check_new_tag_name`: empty, full `refs/...`, leading `-`, invalid ref name → `InvalidParams`.
Not pushed: the tag stays local. Result: `{"tag", "target", "annotated", "refs", ...journal}`.

`CliBackend`: lightweight with `update-ref --no-deref -m "project: tag.create ..." refs/tags/<name>
<id> ""` (only if absent, as `branch.create`); annotated with `git tag -a --cleanup=whitespace -F -
--end-of-options <name> <id>` and the message on stdin, without `-f` (git refuses an existing tag);
`--cleanup=whitespace` keeps lines starting with `#`. Journal entry: the tag from absent to its
value as git stores it (the commit, or the **tag object** of an annotated tag); undo = deletion with
that expected value, redo = creation of the ref on the same value (the same tag object). The undo always saves the tag
object in `refs/project/backup/undo-...` (`RefRestore::abandoned` with `cat-file -t` = `tag`): its
message and author exist only there, even when the commit stays reachable. No backup on creation: nothing is
overwritten.

### `tag.delete`

Parameters: `{"name", "confirm", "expected"?}`.

Deletes the local tag `refs/tags/<name>`; the remote is not touched (`remote.delete_tag` in
[sync.md](sync.md)). Any existing name is accepted (`check_tag_name`: a valid ref name, even `-x`
or `--force`, created by other tools); empty, full `refs/...`, invalid → `InvalidParams`; a tag that
does not exist → `NotAllowed`. **Always** requires `"confirm": true`: without it `ConfirmRequired`
with the value in `expected` (the tag object of an annotated tag, otherwise the commit), saying the
commit and, for an annotated tag, the tag object (its message and author exist only there); nothing
changes. `expected` (optional, full id, otherwise `InvalidParams`): the value the user confirmed; if
the tag now has another value → `NotAllowed`, nothing changes. Result: `{"tag", "target", "annotated", "tag_object":
oid | null, "backup", "refs", ...journal}`.

`CliBackend`: the value as git stores it (the tag object of an annotated tag, the commit of a
lightweight one) first in `refs/project/backup/tag-delete-<s>-<ns>` (`create_backup_ref`: if it
fails nothing is deleted), then `update-ref --no-deref -m "project: tag.delete ..." -d
refs/tags/<name> <value>` (only if it still has that value; otherwise the backup is removed and
`NotAllowed`). The name reaches git only as `refs/tags/<name>`. Journal entry: the tag from its value
to absent, with the backup; undo = `restore_refs` that recreates the ref on the same value (the
same tag object, kept by the backup) only if it is still absent (`UndoConflict` otherwise); redo =
deletion with the expected value (the undo machinery saves the tag object again in
`refs/project/backup/undo-...`).

## Undo and backup

The `branch.*` commands receive **short** names (`feature/x`); the full name is always
`refs/heads/<name>`. An existing branch can also be indicated with a name that git would no longer
accept for a new branch (e.g. `-x`, created with `update-ref`; a valid ref name is enough), so
it can still be switched to, renamed or deleted. Names reach the `CliBackend` only as
`refs/heads/<name>` (`update-ref`) or after `--end-of-options` (`switch`, `branch -m`): a
branch called `--force` or `-d` never becomes an option (tests with such names).

- **`branch.create`**: `update-ref --no-deref -m "project: branch.create ..." refs/heads/<name>
  <id> ""` (empty old value: created only if it does not exist, even if another process
  created it an instant before; `--no-deref`: a symbolic ref `refs/heads/<name>`, even toward a
  nonexistent ref, is not followed, so no other ref is created). Name already taken, symbolic ref
  or a ref that contains it or is contained by it (`a` for `a/b`: "exists; cannot create") →
  `NotAllowed` with git's message. `update-ref -d` of `branch.delete` also uses
  `--no-deref`. No upstream (`branch.autoSetupMerge` does not apply). Journal
  entry: the branch from absent to `<id>`; undo = deletion with the expected value, redo =
  creation only if still absent. No backup: the commit already existed.
- **Upstream** (`branch.create` with `track`, `branch.set_upstream`): `GitRead::local_upstream`
  reads `git config --local -z --get-regexp '^branch\.<escaped name>\.(remote|merge)$'` (file
  order; a non-UTF-8 value → `NotAllowed`). `GitRefs::write_upstream(name, expected, entries)`
  re-reads and refuses (`NotAllowed`) if the entries are no longer `expected`, then per key
  `git config --local --replace-all --end-of-options <key> <value>` (one value; a single
  rewrite of the file under `config.lock`) or `--unset-all` (none; code 5 = nothing to remove);
  if one write fails it writes `expected` back and the error says whether it could. Afterwards a
  re-read: a different value (another process) goes in `warnings`. The configuration has no
  compare-and-swap like `update-ref`: between the re-read and the write there is a small window,
  covered by the re-read afterwards. The journal entry carries `upstream: {"branch", "before",
  "after"}` (`[{"key", "value"}]`, like `branch_config`, but only the two keys, so other entries
  of the section are never removed). `branch.create` with `track`: undo = deletion of the ref as
  above, then `before` written back only if the repo still has `after` (otherwise a warning: the
  ref has already moved, as for `branch_config`); redo = ref recreated, then `after` only if the
  repo still has `before`. `branch.set_upstream`: no refs in the entry; undo/redo refused
  (`UndoConflict`, nothing changes, the entry stays) if the branch no longer exists or its upstream
  is no longer the one the command (or the undo) left, and during an operation in progress.
- **`branch.checkout`**: `git -c core.fsmonitor=false switch --no-guess --no-overwrite-ignore
  --end-of-options <name>` (with detached HEAD `--detach <id>`, only for the undo). Never `--force`,
  `--discard-changes`, `--merge`. Git checks before writing: a modified file (staged
  or not) that differs in the destination, an untracked or ignored file that the destination contains
  (`--no-overwrite-ignore`: by itself git overwrites ignored files without saying so) make the
  command fail without changing anything → `CheckoutConflict` with the files listed by git (lines with the
  tab in stderr, `LC_ALL=C`). Recognized: "would be overwritten by" (checkout, and merge for
  submodule files with `submodule.recurse`), "would be removed by", "would lose untracked
  files" (a directory with untracked files that in the destination is a file), "would be lost". Other changes stay, as in git. Refused beforehand: merge,
  rebase, cherry-pick, revert, bisect, `git am` in progress, conflicted files, and
  `--assume-unchanged` files that differ between HEAD and the destination (git would overwrite them without
  checking). `core.fsmonitor=false` because an outdated fsmonitor would make a modified file look
  clean. Afterwards, HEAD is re-read: if it is not the one requested (`post-checkout`
  hook), `warnings`.
- **The checkout goes into the journal** (choice): GitKraken and our journal treat a branch
  switch as an operation, and without the entry the undo of a commit made before a checkout
  would be refused until one goes back to the branch by hand. The entry has no refs (no ref
  moves), only HEAD before and after. The undo does **not** use `update-ref` (`RefRestore::check`
  keeps refusing a change of HEAD's branch: it would leave the worktree of the
  previous branch), but another equally safe checkout (`GitRefs::checkout` toward HEAD before,
  branch or detached commit), only if HEAD is still the one left by the checkout
  (`UndoConflict` otherwise); if local changes would prevent it, `CheckoutConflict` and the
  entry stays. The redo likewise. A checkout that leaves a branch without
  commits does not go into the journal: going back would mean emptying index and worktree (`switch --orphan`). The
  checkout of the already active branch does nothing and does not go in.
- **`commit.checkout`**: the rev becomes a full id with `GitRead::commit_id` (`rev-parse
  --end-of-options <rev>^{commit}`), then `GitRefs::checkout(Detached(<id>))`, the same call as the
  undo: `git -c core.fsmonitor=false switch --no-guess --no-overwrite-ignore --detach
  --end-of-options <full id>` (a rev like `-x` or `--force` never reaches `switch`). Journal
  entry `commit.checkout` with HEAD before and after and `backup` (if any), undone and redone
  like `branch.checkout`.
- **Commits left behind** (`GitRefs::checkout`, so `branch.checkout`, `commit.checkout` and the
  undo/redo of both): if HEAD is detached at a commit other than the destination, `--no-replace-objects
  rev-list --count --not --branches --tags --remotes --not --end-of-options <HEAD>`; if it
  is not 0 (commits on no branch, tag or remote-tracking branch: after a switch only the reflog
  would keep them), first `refs/project/backup/detached-<seconds>-<nanoseconds>` at that commit
  (created only if it does not exist), then HEAD is re-read (another process committing on the
  detached HEAD meanwhile → `NotAllowed`, nothing switched, backup kept), then the switch. If
  the backup fails nothing is switched. In the result `backup` and a warning with the count and
  the backup name; no `confirm`: nothing is lost. If the switch fails and HEAD is still detached
  at that commit (e.g. `CheckoutConflict`, nothing changed) the backup is removed (`update-ref -d
  <backup> <id>`); otherwise (a failing `post-checkout` hook after the switch) it stays and the
  error names it. Other refs under `refs/` (backups included) do not count as keeping the commit.
- **`branch.delete`**: with `expected` (full id) the branch must still point to that commit,
  checked before computing the confirmation and before deleting (`NotAllowed` otherwise): the UI
  passes it with the commit shown, so `confirm` does not delete a branch moved by another
  process in the meantime. The backend then deletes only if the ref still has that id
  (`update-ref -d <ref> <id>`). Refused for HEAD's branch, for a branch open in another
  worktree (`git worktree list --porcelain`) and during an operation in progress. "Merged" as for
  `git branch -d`: no commits outside HEAD and the upstream, if the upstream ref exists
  (`rev-list --count <tip> ^HEAD ^<upstream>`, without replace objects); otherwise
  `confirm` is needed (`ConfirmRequired` with the number of commits). Always, even if merged, first a
  backup `refs/project/backup/delete-<seconds>-<nanoseconds>` (created only if it does not exist, like
  the others): the deletion also deletes the branch's reflog, and the backup keeps the commit
  for the undo even after a restart or a `gc`. Then `update-ref -d refs/heads/<name> <id>` (only
  if it still has `<id>`; if it fails the backup is removed) and `git config --remove-section
  branch.<name>` like `git branch -d` (upstream and description; an error other than "no such
  section" goes in `warnings`). Before removing it, it is read (`git config --local -z
  --get-regexp '^branch\.<name with escaping>\.[^.]+$'`: only the repo file, no global
  configuration or `include`; a non-UTF-8 value is not saved and a warning says so):
  the result reports it in `removed_config` and the journal entry in `branch_config`
  (`[{"key", "value"}]`, `value` `null` for a key without a value). Entry: the branch from `<id>`
  to absent, with the backup; undo = recreates it at `<id>` only if nobody has created in the meantime
  a branch with that name (`UndoConflict`), then rewrites the configuration (`git config --local
  --add --end-of-options <key> <value>`, a key without a value as `true`) only if the
  repo has no `branch.<name>.*` entries already, otherwise it leaves it and says so in `warnings`; redo =
  deletion with the expected value (no new backup: there already is one) and then the section removed only
  if it is exactly the saved one (a different one stays, with a warning). The configuration is not
  under lock like the refs: a problem there gives warnings, it does not cancel the ref move.
  The deletion also deletes the branch's **reflog** (like `git branch -d`) and the undo does not
  restore it: the backup keeps only the last commit (and therefore its history), not the
  previous positions of the branch; the entry in HEAD's reflog remains, if the branch had been active. The `CliBackend`'s `restore_refs`
  refuses (`NotAllowed`) to delete a branch open in another worktree.
  Limit: a branch stopped in a rebase or bisect of another worktree (HEAD is detached there) does not
  count as open; the commit stays in the backup anyway.
- **`branch.rename`**: `git branch -m --end-of-options <old> <new>` after checking
  that the old one has the id read and that the new one does not exist (never `-M`): unlike
  `update-ref`, `git branch -m` also moves the reflog and the `branch.<name>` section and updates HEAD
  (also of other worktrees). Entry: old from `<id>` to absent, new from absent to `<id>`, HEAD
  before and after (changes if it was the active branch). Undo and redo do not go through `restore_refs` but
  through `GitRefs::rename_branch` in reverse, after the same checks (operation in progress,
  HEAD and the two refs as the operation left them, otherwise `UndoConflict`). Limit: the undo
  of the rename of a branch with a name git no longer accepts (e.g. `-x` → `y`) is refused,
  because the undo's new name (`-x`) does not pass the new-name check.

The `MemoryBackend` makes the same refusals (the checkout is stricter than git: it refuses any
change to a file that differs between HEAD and the destination, and an untracked or ignored file that is the
file, a directory of a file or a file in place of a directory of the destination); as
configuration it has only the upstream (`branch.<name>.remote`/`merge`), and records deletion backups in
`commit_backups` (`refs/project/backup/delete-<n>`), and those of a detached HEAD left behind as
`refs/project/backup/detached-<n>`.
