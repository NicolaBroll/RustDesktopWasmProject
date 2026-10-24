# Conflict editor

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

Marker parser in `crates/domain/src/git/conflict.rs`; types and checks shared by the backends in
`crates/engine/src/git/conflict.rs`; commands in `crates/engine/src/builtin/conflict.rs`;
`CliBackend` in `crates/git/src/backend/conflict.rs`; UI in `crates/ui/src/conflict.rs` and
`crates/ui/src/view/conflict_view.rs`.

## Marker parser (domain)

Pure functions, on the bytes of the worktree file:

- `parse_conflicts(text: &[u8], marker_size: usize) -> Result<ConflictText, DomainError>`:
  `ConflictText { segments: Vec<Segment> }`, `Segment::Common(Vec<u8>)` (bytes outside the blocks,
  newlines included) or `Segment::Conflict(ConflictBlock)` with `ours`, `base: Option<Vec<u8>>`,
  `theirs` (bytes of the lines, newlines included), the labels after the markers (`ours_label`,
  `base_label`, `theirs_label`, `Vec<u8>` without the leading space, empty if missing) and `range`,
  the byte interval of the block in the text, markers and their newlines included.
- Markers: at the start of a line, exactly `marker_size` characters (default 7, attribute
  `conflict-marker-size`) and then end of line or a space: `<<<<<<<`, `|||||||` (only with
  `merge.conflictStyle=diff3`/`zdiff3`), `=======`, `>>>>>>>`. A line with more characters
  (`<<<<<<<<`) is text. Newline `\n` or `\r\n`, preserved as they were; the last line may have no
  newline.
- Text outside a block that looks like a middle marker (`=======`, `|||||||`, `>>>>>>>` without an open
  `<<<<<<<`) is common text (it may be a setext heading or a line of a real file). A
  block that is opened and not closed, or with the parts in the wrong order → `DomainError::InvalidConflict`
  (the editor shows the three versions and the file as is, without blocks).
- `Choice`: `Ours`, `Theirs`, `Base` (only if the block has the base; otherwise error),
  `OursThenTheirs`, `TheirsThenOurs`. `ConflictBlock::resolved(&self, choice) -> Result<Vec<u8>, _>`
  the bytes that replace the block; `resolve_block(text, &ConflictText, index, choice) ->
  Result<Vec<u8>, _>` the text with only that block replaced (the others, resolved or not, stay
  as they are); `resolve_all(text, &ConflictText, choice)` all blocks with the same choice.
- Bytes, not strings: no loss for non-UTF-8 text (the UI however only edits UTF-8, below).

Tests: merge and diff3 style, labels, CRLF, last line without newline, blocks empty on one side,
longer markers, isolated `=======`, unclosed block, different `marker_size`,
`resolve_block` and `resolve_all` for every `Choice`, `range` delimiting exactly the block, and that
concatenating the segments with the blocks rebuilt from the `range`s gives back the file.

## Commands

### `conflict.show`

Parameters: `{"path"}`. Read-only.

`path`: `RepoPath` validated as in `diff.*`; must be a conflicted file in the status (`u`
entry), otherwise `NotAllowed`. Result:

```json
{"path": "...", "kind": "BothModified" | "BothAdded" | "DeletedByUs" | "DeletedByThem" | ...,
 "base": Version | null, "ours": Version | null, "theirs": Version | null,
 "worktree": Version | null, "marker_size": 7, "digest": "<32 hex>"}
```

`Version`: `{"text": string | null, "binary": bool, "size": n}`; `text` is the content if it is
valid UTF-8 and without NUL, otherwise `null` with `binary: true`. `base`/`ours`/`theirs` from stages
1/2/3 of the index (`ls-files -s -z` for the ids, then `cat-file blob <id>`: index bytes, without
filters), `null` if the stage is missing (e.g. `DeletedByUs` has no `ours`); `worktree` the bytes of the file
(`null` if it does not exist; a link or a directory → `NotAllowed`). `marker_size` from `git check-attr
conflict-marker-size -z --stdin` (7 if unset or invalid). `digest`: 128-bit FNV-1a
(like `file_digest`) of the worktree bytes, with a fixed value for "file absent": it identifies the
version shown for `conflict.resolve`. Conflicted submodule (mode 160000) → `NotAllowed`.

### `conflict.resolve`

Parameters: `{"path", "digest", "content"?: string, "delete"?: bool}`. Writes (`writes`).

Exactly one of `content` and `delete: true` (otherwise `InvalidParams`). Resolves the conflict
of `path`: writes `content` to the worktree file (UTF-8 bytes exactly as they arrive, without
newline conversion: the UI starts from the worktree bytes) or deletes it with `delete`, and stages it
like `index.stage` of that path only (`git add` or, with `delete`, `git rm --cached`
of the path, `--literal-pathspecs`). The content is **not** checked for leftover
markers: it is free text from the user; warning about it is the UI's job.

Checks, before writing, always `NotAllowed`/`StaleDiff` without changing anything:

- `path` not conflicted (`u` entry), submodule, link or directory → `NotAllowed`.
- `digest` different from the one computed now on the worktree → `StaleDiff` (the file changed after
  `conflict.show`: re-read).
- Locked index → `IndexLocked`.

**Backup** (§1, rules: the only copy of something is rewritten without git asking for confirmation): the
worktree file with the markers and the user's manual edits exists only there. Before
writing, its bytes are saved (`hash-object -w --no-filters --stdin`) in a commit (fixed backup
identity, no parent) with a tree containing only that file at its path, in
`refs/project/backup/conflict-<seconds>-<nanoseconds>`. If the file does not exist (e.g. `DeletedByUs`
with the file deleted) no worktree backup. The versions of the three stages are not needed: they come
from the merge commits and `git checkout -m <path>` regenerates the file with the markers. If the
backup fails, nothing is written. If the write or the stage fails after the backup, the
backup stays and the error names it.

The write to the worktree: temporary file in the same directory and `rename` over the file (with the
permissions of the current file); then the digest is re-read to check that in the meantime
nobody changed it (if before the rename the file is no longer the one of the digest, `StaleDiff`
and the temporary file is deleted). Known limit: between this last check and the `rename` there remains a
window of a few system calls in which a save from an editor would be lost without a
backup. The stage uses the same path as `index.stage` (with its index backup,
if needed).

Result: `{"status": Status, "backup": string | null, "index_backup": string | null}`
(`backup`: the ref of the saved worktree; `index_backup`: that of `index.stage`'s index).

Undo: it moves no refs, so no journal entry, like the stage; recovery is the backup (`git show
<backup>:<path>`) or `git checkout -m <path>` while the merge is in progress.

Implementation choices (where the contract was silent):

- The stage ids come from the `u` record of `git status --porcelain=v2` (they are the same as
  `ls-files -s`), read anyway for the "conflicted" check; the bytes with `cat-file blob`.
- `digest`: 128-bit FNV-1a of a type byte (`0` file absent, `1` present) followed by the bytes
  of the worktree (`worktree_digest`), so no content has the digest of the absent file.
- A symbolic link even among the **directories** of the path → `NotAllowed` (it would write outside
  the repo or into another file); with the file absent the missing directories are created.
- The temporary file (`.project-conflict-<pid>-<nanos>.tmp` in the file's directory) takes all the
  permissions of the file it replaces (a 0600 file stays 0600); a file that did not exist is created with the
  permissions of the `umask`, executable if one of the `ours`/`theirs` stages is (e.g. `DeletedByUs`
  with their side 100755). Missing directories are created only after re-checking the digest. The backup records the file mode (`100644`/`100755`).
- An error after the backup (write, re-checked digest, stage) keeps the type when it is
  `Git`, `NotAllowed` or `StaleDiff` and adds the backup name to the text; the others become
  `Git` with the original text and the backup.
- `marker_size`: a positive number from the attribute, otherwise 7; the `MemoryBackend` always gives 7.
- `delete` also goes through `index.stage`'s stage (`git add` of a deleted path
  records the deletion and removes the stages), without a separate `git rm --cached`.

`MemoryBackend`: the `u` entries of `merge_three_way` with the three stages; `conflict.show` reads them,
`conflict.resolve` writes the file and stages it like `stage`; backup in its ref store.

## UI

From the in-progress operation banner (`repo.operation`), a conflicted file opens in the conflict
editor (in place of the diff view), which reads `conflict.show` and parses `worktree.text`
with `parse_conflicts(.., marker_size)`:

- **Three columns on top**, read-only: base, ours (`ours_label`, or "HEAD"), theirs
  (`theirs_label`, or the `label` of `incoming`), each with the text of its stage; `null` →
  "(absent)", binary → "(binary)".
- **Result at the bottom**, editable (multiline `TextEdit`, monospace), starting from the worktree
  text. For each parser block a row of buttons: "Ours", "Theirs", "Both",
  "Base" (if present). The blocks are those of the **current** result text (re-parsed at every
  edit): the choice replaces that block with `resolve_block`, the rest (including manual
  edits) stays. Global buttons "All ours", "All theirs" (`resolve_all`) and "Start over" (re-reads
  `conflict.show`). If the current text does not parse (markers broken by hand) no per-block
  buttons, the text stays editable.
- **"Mark as resolved"** → `conflict.resolve {path, digest, content}`. If the result still contains
  markers (`parse_conflicts` finds blocks) it asks for confirmation in two steps ("It still contains
  markers: resolve anyway?"). Afterwards: re-reads `repo.operation` and the status; the file disappears from
  `conflicts`; with `ready` the "Conclude merge" button becomes active. `StaleDiff` → message and
  re-read.
- Binary file, `worktree.text` `null` or parser that fails: no blocks and no editable
  result; only "Use ours" / "Use theirs" (with `content` = text of that side; only if it is
  text) and, for `DeletedByUs`/`DeletedByThem`, "Delete the file" (`delete: true`).
- No modification logic in callbacks: the buttons prepare `content` (pure, `resolve_block`) and
  invoke `Registry::run`.

Done when: parser tests; a UI test (`egui_kittest` or the view model, like the
others) that opens a `MemoryBackend` conflict, chooses per block, marks as resolved and finds
`ready`; `CliBackend` integration scenario (`crates/git/tests`) with a conflicted merge,
`conflict.show`, `conflict.resolve`, `merge.continue`; CI green.
