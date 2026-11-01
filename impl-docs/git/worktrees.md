# Linked worktrees

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5) and the journal (§7) are there; here are the command contract, the `CliBackend` and undo
and backup of this function. "§n" always refers to the main document.

**Not to be confused** with the commands on the files of the working tree, which share the
`worktree.` prefix: `worktree.discard`, `worktree.discard_lines` ([discard.md](discard.md)),
`worktree.clean` and `worktree.ignore` ([ignore.md](ignore.md)). The commands here manage the
*linked worktrees* of `git worktree` (more checkouts of the same repository in other folders):
`worktree.list`, `worktree.add`, `worktree.remove`.

Code: `crates/engine/src/git/worktree.rs` (types, refusals), `crates/engine/src/builtin/worktrees.rs`
(commands), `crates/engine/src/git/memory/worktree.rs` (`MemoryBackend`),
`crates/git/src/backend/worktrees.rs` (`CliBackend`); trait `GitWorktrees`.

## Commands

### `worktree.list`

Parameters: `{}`. Read-only. Result: `{"worktrees": [WorktreeInfo]}`, the main worktree first.
Each `WorktreeInfo`: `path` (absolute, as git records it; text: a non-UTF-8 path is shown with
replacement characters and cannot be removed from here), `main`, `current` (the worktree open in
this session), `bare` (the main entry of a bare repository), `head` (commit id, `null` on a branch
without commits), `branch` (full name, `null` when detached), `detached`, `locked`, `lock_reason`
(`null` without a reason), `prunable` (its folder or its `.git` is gone: `git worktree prune` would
drop the entry).

### `worktree.add`

Parameters: `{"path": string, "branch"?: string, "new_branch"?: string, "start"?: rev}`. Writes.

- `path`: an absent folder (created) or an existing empty one, absolute or **relative to the root
  of the open worktree**; never a file, a link or a folder with something in it (the checks of
  `repo.clone`, [clone.md](clone.md): `engine-clone-not-empty`, `-not-folder`, `-no-parent`). Empty,
  with NUL or a newline → `InvalidParams`.
- `branch`: an existing local branch, checked out there. Checked out in any worktree (this one
  included) → `NotAllowed` `engine-worktree-branch-in-use` (git refuses it too).
- `new_branch`: a new local branch at `start` (HEAD without it), created first with `update-ref`
  (create-only, as `branch.create`: no upstream, whatever `branch.autoSetupMerge` says), then checked
  out there. If the worktree cannot be created the branch is deleted again (only if still at its
  commit) and the error is the original one; if that deletion fails,
  `engine-worktree-branch-not-rolled-back` names the branch.
- neither: a **detached** HEAD at `start` (HEAD without it); git's default of creating a branch
  named after the folder is never used.
- `branch` with `new_branch` or with `start` → `InvalidParams`.

Result: `{"path", "branch", "created_branch", "head", "messages", "warnings", "worktrees",
"worktrees_error"?, "can_undo", "can_redo", "next_undo", "next_redo"}`. `path` is absolute (what
`repo.open` takes: the UI opens it in a new tab); `head` the commit checked out; `messages` git's
lines; `warnings`: the worktree was created but git failed afterwards (a `post-checkout` hook:
`engine-worktree-add-hook` with git's text), or it is not on the requested branch/commit (another
process). `worktrees`: the list re-read (`null` with `worktrees_error` if it cannot be).

### `worktree.remove`

Parameters: `{"path": string, "confirm": true}`. Writes. `path` as `worktree.list` gives it, or
another spelling of the same folder (relative to the root of the open worktree).

Refused (`NotAllowed`, nothing written), in this order: not a worktree of this repository
(`engine-worktree-unknown`, also a non-UTF-8 path), the main worktree (`-main`), the worktree open in
this session (`-current`), locked (`-locked` / `-locked-reason`), the folder exists but no longer holds
that worktree (`.git` missing or pointing to another repository: `-not-worktree`), an operation in
progress there (merge, rebase, cherry-pick, revert, bisect, am: `-operation`), tracked changes
(staged, unstaged, conflicted, submodules included: `-changes`), untracked files (`-untracked`),
**ignored files** (`-ignored`: `git worktree remove` deletes them without asking and a backup could be
huge, e.g. a build folder), files marked `--assume-unchanged`, or `--skip-worktree` and present
(`-hidden`: git status cannot tell whether they changed). The messages give the count and the first
path. A worktree whose folder is gone (`prunable`) has no files to check: only its entry is removed.

Without `confirm` (after the refusals): `engine-worktree-remove-confirm`, which says whether a backup
would be saved. Result: `{"path", "backup", "messages", "worktrees", "worktrees_error"?}`.

## Steps and guarantees (`CliBackend`)

Every option exists in git 2.30 (`GitVersion::MIN`): no `list -z` (2.36), no `--orphan` (2.42), no
`--path-format`; `locked` and `prunable` in the porcelain list exist only from 2.31.

1. List: `git worktree list --porcelain` (records separated by an empty line; a path with a newline
   is not supported, `worktree.add` refuses one). `locked` from the porcelain line or, for older
   git, from `<common dir>/worktrees/<id>/locked` (the administrative folder whose `gitdir` file
   holds `<path>/.git`; common dir from `rev-parse --git-common-dir`); `prunable` from the
   porcelain line or when `<path>/.git` does not exist (not for a locked worktree); `current` by
   comparing canonical paths.
2. Add: the destination as for `repo.clone` (`Destination::prepare_in`, relative to the worktree
   root; an absent folder is created with `create_dir`), then `git worktree add --no-guess-remote
   --end-of-options <abs path> <branch>` or `git worktree add --detach --end-of-options <abs path>
   <commit id>`. Never `--force`, `-b`/`-B`, `--lock`, `--orphan`. Failure: if the worktree is not
   registered the destination goes back to how it was (a created folder removed, an existing empty
   one emptied); if git says the path already exists, nothing is cleaned (someone wrote there); if
   it is registered (a hook failed after the checkout), success with a warning. Afterwards the
   worktree is looked up again and must be on the requested branch / commit, otherwise a warning.
3. Remove: the entry is found (same bytes, or same canonical folder; not UTF-8 → unknown), the
   refusals of `removal_refusal` (shared with the `MemoryBackend`) are applied with the contents
   read **inside the worktree** after checking that git there has its top level in that folder and
   the same common dir: `git -c core.fsmonitor=false status --porcelain -z --untracked-files=normal
   --ignored --ignore-submodules=none`, `ls-files -v -z` (lowercase tag = assume-unchanged, `S` =
   skip-worktree), the operation files of its git directory.
4. A detached HEAD with commits on no branch, tag or remote branch (`rev-list --max-count=1 <head>
   --not --branches --tags --remotes`, also for a worktree whose folder is gone, from the porcelain
   `HEAD`) is saved with `update-ref refs/project/backup/worktree-<s>-<ns> <head> ""` in the shared
   repository; if the backup fails nothing is removed. The entry is then re-read: HEAD or branch
   different → `engine-worktree-changed` (the backup stays and is named).
5. `git worktree remove --end-of-options <path>`: without `--force` git checks again changes,
   untracked files and the lock, and refuses a worktree with populated submodules. A failure after
   the backup (git's own refusal, a folder it cannot empty midway) → `engine-worktree-remove-failed`
   with git's text and the backup, which is never removed.

Known limits: between the checks and `git worktree remove` another process can create an ignored
file, which git deletes; a worktree whose folder is gone loses its index (staged changes there were
never in a commit), as with `git worktree prune`.

The `MemoryBackend`: the main worktree is the repo (`/repo`, `MEMORY_MAIN_WORKTREE`); linked ones
are set by the tests (`MemoryBackend::add_linked_worktree` with their contents, `unreferenced`) or
created by `worktree.add` (a relative path is joined to `/repo` without normalization;
`MemoryBackend::occupy_path` makes a path not empty). Remove applies the same refusals and saves
the backup in `refs/project/backup/worktree-<n>`. A branch checked out in a linked worktree is not
deleted by `branch.delete` nor by undo, as with git.

## Undo and backup

Only the branch `worktree.add` creates goes into the journal, as a `branch.create` entry (HEAD of
this worktree unchanged): undo deletes the branch, refused (`engine-undo-delete-worktree-branch`)
while a worktree has it checked out; after `worktree.remove` it works. Adding or removing a worktree
moves no ref and is not undone: a removed worktree is added again with `worktree.add`; its detached
HEAD, when it had commits on no ref, is in `refs/project/backup/worktree-...` (`worktree.add
{"path", "start": "<ref>"}` checks it out again).
