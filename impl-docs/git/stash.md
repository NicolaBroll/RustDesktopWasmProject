# Stash

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

## Commands

### `stash.list`

Parameters: `{}`.

Read-only. Result: `{"stashes": [{"index": n, "name": "stash@{n}", "id", "base", "message",
"branch": string | null, "time", "untracked": bool}]}`, from the most recent, from the reflog of `refs/stash`
(empty if it does not exist). `base`: the first parent, the HEAD commit when the stash was created (the
UI draws the stash next to this commit; with `repo.log {"all": true, "stashes": true}` it is always
in the graph); `message`: the reflog message as in `git stash list` (`On <branch>: <msg>` or `WIP
on <branch>: <id> <subject>`); `branch` read from there (`null` with detached HEAD or another format);
`time`: date of the stash commit; `untracked`: it has the third parent with the untracked files. The tracked
files of a stash are read with `commit.show`/`diff.commit` on the `id` (comparison with `base`:
the saved worktree, staged or not); the untracked ones are not there. `refs/stash` also remains in
`repo.refs` (`kind: "Stash"`, the most recent stash).

### `stash.push`

Parameters: `{"message"?, "include_untracked"?, "keep_index"?, "paths"?}`.

Writes (`writes`), goes into the journal (§7). `git stash push` (see "CliBackend"): saves changes to tracked
files (staged or not) and, with `include_untracked`, untracked files (never ignored ones: no
`--all`), then returns index and worktree to HEAD (with `keep_index` the index stays and the worktree
becomes like the index). `paths`: non-empty list of literal paths, like `index.stage` (**git
still puts the whole index in the stash**: staged changes of other files stay in the
worktree but are also in the stash, and applying it brings them back). `message`: one line, without control
characters (`InvalidParams`); empty = absent (git writes `WIP on ...`). Refused before git
(`check_stash_push`, `NotAllowed`, nothing changes): operation in progress, conflicted files, branch
without commits, `intent-to-add` files (git cannot save them), `--assume-unchanged` files among those
chosen (`CliBackend` only, see below: git would not save their changes and would then delete them with the
worktree reset), nothing to set aside among the chosen files (even if git does not create the stash:
`refs/stash` unchanged). Result: `{"stash": StashEntry, "stashes": [StashEntry], "status",
...journal}`.

### `stash.apply` / `stash.pop`

Parameters: `{"index"?, "expected"?, "restore_index"?}`.

They write (`writes`), do **not** go into the journal (§7). The stash is `stash@{index}` (0 if missing; not
found → `NotAllowed` saying how many there are); `expected`: the **full** id of the stash seen
(otherwise `InvalidParams`), if the list has changed → `NotAllowed` saying where it is now (recommended:
the UI always passes it). The id reaches the backend. `restore_index`: also restores the split between
staged and not (`--index`); without it, everything comes back unstaged except new files, which git leaves staged.
**Never over local changes** (checks before git, `NotAllowed` and nothing changes): operation in
progress, conflicted files, a change (staged or not) to a file the stash changes (relative to the
base, in the saved worktree or index), an untracked or ignored file in place of a file the
stash adds or of an untracked file it restores (even in place of one of its directories),
`--assume-unchanged` files the stash changes. **Exception**: a file without unstaged changes whose
index entry is the same in the index and in the worktree saved by the stash (`<id>^2` and `<id>`; mode
0: absent from both) already has the stash's version and does not block: this is the case of files left
staged after a push with `keep_index`, or with `paths` (git saves the whole index anyway); without it,
the undo of those pushes and the pop after a `keep_index` would always be refused. An `MM` file left
with `keep_index` stays blocked (the stash's worktree is different: git would conflict).
They are needed because `git stash apply` does not check everything before writing: with an untracked file of the
stash already present it restores the tracked files and then fails, and refusing because of a local change it has
already restored the untracked files. `restore_index` that does not apply over the current index (HEAD
changed from the base) → `NotAllowed` suggesting to retry without it. **Conflicts** (choice): if
HEAD has changed the same files since the base the merge stops and the files stay conflicted as git
leaves them (`conflicts`; no operation in progress, only `U` entries in the index); nothing is lost,
because those files had no local changes (check above) and the stash stays in the list: `pop`
does not delete it (`dropped: false`, with a warning). They are resolved with `index.stage` of the file (then
the conflict editor, S30); while they exist, apply and push are refused. `pop` without conflicts:
drop like `stash.drop` (backup `refs/project/backup/stash-...` first). A drop that fails **after**
the apply is not an error of the pop (`StashPopped::drop_error`): the apply is done and must not be repeated, the
result has `dropped: false` and a warning "applied but not removed from the list". Result:
`{"stash": StashEntry, "conflicts": [path], "dropped": bool, "backup": string | null,
"warnings", "stashes", "status"}`.

### `stash.drop`

Parameters: `{"index"?, "expected", "confirm"}`.

Writes (`writes`), goes into the journal (§7). Stash chosen like `stash.apply`. It **always** requires
`"confirm": true` **with `"expected"`**, the full id of the stash: without `confirm`,
`ConfirmRequired` with position, id, message, number of tracked files and whether it has untracked ones, and
the id in `expected` (as for the force of `remote.push`); `confirm` without `expected` →
`InvalidParams`; nothing changes. The confirmation is valid only for the stash seen (`expected` different from
`stash@{index}` → `NotAllowed`). First a backup of the commit in `refs/project/backup/stash-<s>-<ns>`
(if it fails nothing is deleted). Result: `{"stash": StashEntry, "backup": string,
"warnings", "stashes", ...journal}`.

## CliBackend

**Stash** (`stash.*`, S28). List: `git log --walk-reflogs -z --no-show-signature
--no-color --format=tformat:%H%x00%P%x00%ct%x00%gs --end-of-options refs/stash --` (4 fields
per record, each terminated by NUL; only if `refs/stash` exists). Push: first `ls-files -v`: an
`--assume-unchanged` file (lowercase letter) among those chosen → `NotAllowed`, always, without
looking at the content (`git stash push` does not see its changes and then resets the worktree to
HEAD: it would lose them), before the other checks. Then `git -c
core.fsmonitor=false [--literal-pathspecs] stash push [--include-untracked]
--keep-index|--no-keep-index [--message=<msg>] [--pathspec-from-file=- --pathspec-file-nul]`
(paths on stdin). **Exception to the messages-via-stdin rule**: `git stash` does not read the
message from stdin or from a file, so it goes in a single `--message=<text>` argument (one line
without control characters, checked by the engine), which git cannot read as another
option. The new stash is recognized from `refs/stash` before and after (git exits with 0 even with
"No local changes to save"); if git fails after creating the stash, the error names it.
Apply: the files the stash touches (`diff-tree -r -z --name-only --no-renames <base> <id>` and
`<base> <id>^2`) and the untracked files it restores (`ls-tree -r -z --name-only --full-tree
--end-of-options <id>^3`, **without a trailing `--`**: `ls-tree` would take it for a path)
compared with the status and with the file system (`symlink_metadata`: a file the stash
adds must not exist, not even ignored, and its directories must be real directories),
with the exception of files that already have the stash's version (entries of `ls-tree -r -z
--full-tree <id>^2` and `<id>` equal to the index entry), then `git -c core.fsmonitor=false
-c merge.directoryRenames=false stash apply [--index] --end-of-options <id>` (the id, never
`stash@{n}`). `merge.directoryRenames=false`: with directory-rename detection a
file the stash adds in `old/` would end up in `new/` if HEAD renamed the directory, and
would overwrite an ignored file there, which the checks do not see (they look at `old/`). Non-zero exit with `U` entries in the status or `CONFLICT` in the output: the conflicts
(a result, not an error); "conflicts in index" (`--index`): `NotAllowed`, nothing modified.
Only if the repo changes between the checks and git, which then has already written part: "already
exists, no checkout" → `NotAllowed` saying the apply was **left half done** (git has already done
the merge of the tracked files, the untracked ones listed were not restored); "would be
overwritten" → `NotAllowed` with only the files of the block right after the message (not the
status summary git prints afterwards), "nothing modified" only if the stash has no untracked
files, otherwise it warns that git may already have restored them. The untracked files to restore
are always checked on the file system, even if the status shows them (e.g. `D.` with the ignored
file in the worktree), and the exception for files that already have the stash's version applies only
to tracked ones. Drop: backup
`update-ref <ref> <id> ""`, re-check that `stash@{n}` is still the id, `git stash drop
--quiet --end-of-options stash@{n}` (if it fails the backup is removed); git has no drop with
the expected value, so the instant between the re-check and `drop` remains, and afterwards the list is
compared with the expected one (different → `warnings`). Store (undo of a drop): `git stash store
--quiet --message=<msg> <id>` (`store` rejects `--end-of-options`; the id is hexadecimal), then the
stash must be at the top.

## Undo and backup

`stash.push` and `stash.drop` go into the journal with the stash (`JournalEntry::stash`, a
`StashRecord`: `{"id", "message", "base", "push": StashPush | null}`); the entry has no `refs`
(the stash list is the reflog of `refs/stash`, and moving that ref would not be enough).

- **Undo of `stash.push` = `pop` of that stash with `restore_index`** (like `git stash pop
  --index`): only if there is no operation in progress, HEAD is the one of the entry **and on the commit
  `base`** (so the merge cannot conflict: the stash is reapplied on the same
  commit) and the stash is still in the list (at any position), otherwise `UndoConflict` and
  nothing changes. The `stash.apply` checks apply: if in the meantime a file of the stash has
  local changes, `NotAllowed` and the entry stays. Before the drop the stash commit goes into
  `refs/project/backup/undo-...` (in `backups`). If the apply succeeds but the drop does not, the undo gives an
  error saying the stash was applied (it must not be repeated; the entry stays in the journal and the
  stash must be deleted with `stash.drop`). **Limit**: after a push with `paths` while other
  files were also staged, git put the whole index in the stash, and `pop --index` does not apply
  over the index that still has those files ("conflicts in index"): the undo is refused (`NotAllowed`,
  nothing changes, the entry stays) with its own message pointing to the way out, `stash.pop`
  without `restore_index`. Redo = `stash.push` again with the same
  parameters, with the same HEAD: the new stash usually has another id (the entry is updated) and
  contains what there is to set aside at the time of the redo (usually what the undo
  restored; if in the meantime other files changed, those too, which stay in the
  stash anyway).
- **Undo of `stash.drop` = `git stash store`** of the same commit (kept by the drop's backup),
  only if it is not already in the list (`UndoConflict`): it returns **to the top** (`stash@{0}`), not to the
  previous position, because `git stash store` cannot insert elsewhere (choice: rewriting the
  reflog of `refs/stash` by hand would be more fragile). The message is the reflog's. Redo =
  the drop again, wherever the stash is now; the drop's backup is usually already there, but if it does not
  point to the stash any more (removed or moved by hand) a new one is made, `refs/project/backup/redo-...`,
  before deleting it.
- **`stash.apply` and `stash.pop` do not go into the journal** (choice): they change only index and
  worktree, and undoing them would mean removing from the files what the stash added,
  mixed with the changes that were already there (apply and pop are allowed with changes to other files)
  or made afterwards: there is no safe way. Nothing is lost anyway: after `apply` the stash stays
  in the list, after `pop` its commit is in `refs/project/backup/stash-...` (and the undo of a
  previous `stash.push`, with the stash now removed, is refused with `UndoConflict`). `can_undo`
  after an apply or a pop therefore refers to the previous entry, and the UI must not present undo
  as undoing the apply.
- A conflict left by apply/pop has no automatic way out for now: it is resolved file by file
  with `index.stage` (then with the conflict editor, S30). Thanks to the `stash.apply` checks
  the conflicted files had no local changes: the "ours" version is that of HEAD.
- `refs/stash` is a **volatile** protection of commits (see §7 "Backup of undone
  commits"): the `refs/project/backup/stash-...` and `undo-...` backups of drop and pop are instead
  real refs, which a future backup cleanup must treat like the others.

The `MemoryBackend` records stash backups in `commit_backups`
(`refs/project/backup/stash-<n>`, `undo-<n>`, `redo-<n>`) and resolves them as refs.

## UI (`crates/ui/src/stash.rs`)

**Stash (S28, `crates/ui/src/stash.rs`)**. The "Stash" section of the left column comes from
`stash.list` (read with refs and status, re-read after undo/redo and taken from command responses),
not from `refs/stash`: one row per stash (`stash@{n}`, message without the `On <branch>:` prefix,
branch and base, "with untracked"). Click: selects the stash (by id, the position changes) and its
base in the graph; on the right, in place of the commit details, the stash panel with the
actions and the tracked files (`commit.show` on the id; a file opens `diff.commit` on the id, i.e. the
comparison with the base); untracked files are only flagged, because `commit.show` does not see them.
Context menu and panel: Apply, Pop, the "Also restore the index" checkbox
(`restore_index`), Delete…. Apply, pop and drop always send `index` and `expected` from the list
seen; a `NotAllowed` goes in the status bar and the list is re-read. **Apply and pop do not go into
the journal**: the notice says "Cannot be undone with “Undo”" and the Undo button stays that
of the previous entry (its tooltip shows its description). A non-empty `conflicts` opens
a (non-modal) window with the files and "Go to changes": they are resolved by staging the file. A successful pop with `dropped: false` and without conflicts (applied, but the drop did not
succeed) is flagged in red with the engine's warning, and for that stash Pop stays disabled (the
changes are already in the files): Delete remains.
Delete goes in two steps like removing a remote: without `confirm` the engine responds with
`ConfirmRequired` and the reason, the red dialog shows it, and only the click sends `"confirm":
true`. "Stash…" (top bar, head of the stage area, Stash section; active only with
changes) opens a dialog with an optional message (one line: newlines become spaces),
"Include untracked files" and "Keep index"; a refusal stays in the dialog. In the graph each
stash is a label on the base row, after branches and tags, with only a gray border: the list's
log asks for `"all": true, "stashes": true`, and after push, drop and a successful pop it is re-read.
