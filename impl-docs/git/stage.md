# Stage and unstage (files and lines)

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

## Commands

### `index.stage` / `index.unstage`

Parameters: `{"paths"}`.

`paths`: non-empty list of `RepoPath`s validated as in `diff.*`, taken literally (a directory
includes its files). Result: `{"status": Status, "backup": string | null}`, with the new status
(like `repo.status`) and the ref of the index backup saved before writing, if there were staged versions
that existed only in the index (see "Index backup"). `stage` = `git add` (modifications, new files,
deletions; a rename done in the worktree is staged with the two paths; a
deletion already staged is skipped); first a dry run (`--dry-run`), so an ignored file
requested by name (`NotAllowed`, never `-f`) or a nonexistent path leave nothing half done.
Staging a conflicted file = resolution, only if the file is requested by name: inside a requested
directory it is refused (`NotAllowed`). `unstage` = `git restore --staged --no-overlay
--source=<HEAD>` (empty tree without commits): index only, never the worktree; a conflicted file is
refused (`NotAllowed`), because it would lose the conflict stages. Index locked by another
git: `IndexLocked`, without retrying or deleting the lock.

### `index.stage_lines` / `index.unstage_lines`

Parameters: `{"path", "lines", "digest"}`.

Uses the patch builder (below) and the strict applier: `lines` is a **non-empty** list of
`LineRef`s (`{"old": n}` / `{"new": n}`) with the line numbers from `diff.worktree` (stage) or
`diff.index` (unstage) for `path`; `digest` (required) is the `digest` field of that file in the
same diff, which identifies the diff seen (see below). The backend re-reads the file's diff, chooses
the `FileDiff` with `new_path == path` (otherwise `old_path == path`: a rename requested with the
old name), checks the digest, builds the patch with `build_patch` (without `--reverse`: the
patch is always forward relative to the index), refuses (`NotAllowed`) a hunk without leading
context that does not start at the beginning of the file or without trailing context that is not the last, computes the
new content with `apply_patch` on the staged blob and writes it (`CliBackend`: `hash-object -w
--stdin --no-filters` and `update-index -z --index-info`, see "Index backup"; `MemoryBackend`: in its blobs). Errors,
always without writing: file with no more changes or different digest → `StaleDiff` (re-read the diff);
two `FileDiff`s on the same path (type change) → `NotAllowed`; domain `UnsupportedPatch`
(binaries, submodules, links, conflicts, copies, renames in the stage) → `NotAllowed`;
`InvalidLineSelection` or empty `lines` → `InvalidParams`; locked index → `IndexLocked`. Backup
like `index.stage` (see "Index backup"). Result: `{"status": Status, "backup": string | null}`.


`diff.worktree` and `diff.index` add to every file the `digest` field: 32 hex digits,
a 128-bit FNV-1a hash of the JSON serialization of the `FileDiff` (`project_engine::file_digest`;
a `FileDiff` re-read from that JSON gives the same digest). It covers paths, modes, ids and every line with
its numbers: the line numbers of a selection may coincide by chance with those of a changed
diff (same number, different content), and without the digest the patch would move into the
index lines the user did not see. The ids alone are not uniformly sufficient: in the
worktree diff the new id is computed by git, but in the `MemoryBackend` it is fake. The parameter is
required because whoever chooses the lines has in any case just read the diff the
numbers come from. It is not a cryptographic hash: it protects against a changed file, not against someone building
two diffs with the same digest on purpose.

## Patch builder (domain)

For staging and unstaging single lines: `build_patch(file: &FileDiff, lines: &[LineRef],
direction: PatchDirection) -> Result<Option<FileDiff>, DomainError>` (module
`crates/domain/src/git/patch.rs`), a pure function. The returned patch is a `FileDiff`
(`to_patch()` gives the bytes) that can be applied with `git apply` plus the `APPLY_ARGS` arguments
(`--cached --recount --whitespace=nowarn --no-ignore-whitespace`), patch on stdin; plus `--check`
to test it. The two whitespace flags neutralize `apply.whitespace=error` (it would reject
lines with trailing spaces) and `apply.ignoreWhitespace` (it would accept an index differing in whitespace).
The `index.stage_lines`/`index.unstage_lines` commands however do **not** use `git apply`: `git apply`
looks for a hunk's preimage even far from the written position and still accepts at end of file a hunk
without trailing context, so with a zero-context diff (`diff.context=0`) an insertion in the
middle of the file ended up at the bottom and the command succeeded. The engine computes the new content with
a strict applier (`project_engine::apply_patch`: every hunk only at its position,
context and removed lines byte for byte, hunks without context allowed only at the start or end
of the file) on the index blob from which the diff was computed, and writes it (see "Index backup"). `APPLY_ARGS`
stays for the tests that verify patches with git (`crates/git/tests/patch.rs`).

- **Directions.** `git apply --cached` (with which the builder tests verify the patches; the
  commands use the strict applier, which behaves the same way but only at the written
  position) compares the patch preimage (context and `-` lines) with the index. The patch is always built *forward* relative to the index, so the two
  directions apply the same way, **without `--reverse`**.
  `stage` works on the `diff.worktree` diff, whose old side is the index: selected additions
  `+`, unselected additions dropped, selected removals `-`, unselected
  removals context. `unstage` works on the `diff.index` diff, in which the index is the
  **new** side: the diff is inverted and the same rule applied, which relative to the displayed lines
  becomes: selected additions `-` (they leave the index), unselected additions context
  (they stay), selected removals `+` (they return), unselected removals dropped.
- **Order in the result.** In every group of changes between two context lines, removed and
  added lines are paired by position (first removed with first added, ...): of `-x -y +x' +y'`
  the stage of `x → x'` gives `x' y`. The unstage of a selection gives the same content as the stage
  of the unselected lines starting from HEAD.
- **Selection** (`LineRef`, in JSON `{"old": n}` / `{"new": n}`): the line number in the side
  where the changed line exists, as in the diff's `Line` (`old_line` for removed,
  `new_line` for added); `Line::line_ref`, `Hunk::line_refs` and `FileDiff::line_refs` give the references
  of a line, of a whole hunk and of the file. Empty selection: `Ok(None)`. A reference that is not
  a changed line of the diff (context, nonexistent number, wrong side):
  `DomainError::InvalidLineSelection`, never a patch on other lines.
- **End of file without newline**: a line without newline that in the result is no longer the last takes
  the newline (if it was in the preimage it becomes `-line` + `\ No newline` and `+line`).
- **New file**: `new file mode` patch with only the chosen lines. In the worktree diff it appears only
  with `git add -N` (in the builder tests `git apply --cached` accepts creating an
  `intent-to-add` file; the commands write the entry with `update-index --index-info`, which replaces
  the placeholder with a normal entry); an untracked
  file is not in the diff and must be staged whole. In the unstage it is the case of a file
  deleted in the stage: it returns to the index with the chosen lines.
- **Deleted file** (from the worktree in the stage, new staged file in the unstage): all lines
  ⇒ `deleted file mode`, the file leaves the index; partial selection ⇒ modification, the file stays
  with the unselected lines.
- **Renames**: line patches do not rename. In the unstage (staged rename) the patch
  modifies the new path, the one in the index; the old one does not return (`index.unstage`
  of the file does that). In the stage (rename seen with `git add -N`) it is refused.
- **Mode changes**: never in the patch, which has no `old mode`/`new mode` or `index` lines and leaves
  the index mode; they go with staging the whole file.
- **Headers**: written by `FileDiff::write_header` (names in quotes where needed, exact
  bytes), without `index`, `similarity`, `rename`; hunk counts recomputed, so the
  patch is valid even without `--recount`. Content (CRLF, non-UTF-8 bytes) copied byte for byte.
- **Refused** with `DomainError::UnsupportedPatch` (before looking at the selection): binaries,
  submodules, symbolic links, conflicts, copies, renames in the stage.

Tests: `insta` snapshots of patches on real git output (`testdata/patch.sh`), property tests with
a pure applier (chosen lines applied and the others not, total selection = original diff,
unstage = stage of the complement, stage then unstage = initial content) and tests in
`crates/git/tests/patch.rs` that apply the patches with `git apply --check` and then for real.

## Index backup

`index.stage` and `index.unstage` rewrite the staged version of files. If a touched file
(even inside a requested directory) has in the index a version different from HEAD and from the worktree
(XY code of `git status` with X ∉ {`.`, `?`} and Y ≠ `.`: `MM`, `AM`, `AD`, `MD`, `RM`, ...), after
the write that version would remain only in an unreachable blob. Before writing, the
backend saves it:

1. a temporary index in the git directory (`GIT_INDEX_FILE`) with only those versions
   (`update-index --index-info`), so it works even with conflicts in the real index, and
   `write-tree`;
2. `commit-tree` with parent HEAD (if any), message with the operation and the paths, fixed
   identity (the backup succeeds even without `user.name`) and `--no-gpg-sign`;
3. `update-ref refs/project/backup/index-<seconds>-<nanoseconds> <commit> ""`: the ref is created only
   if it does not exist, a backup never overwrites another.

For `--assume-unchanged` or `--skip-worktree` files (letters from `git ls-files -v`) `git status`
gives Y = `.` even if the worktree differs: for them X ∉ {`.`, `?`} is enough. The status read before
writing does not use `core.fsmonitor` (an outdated fsmonitor would hide the changes).
The temporary index is written with `core.splitIndex=false`, `core.fsmonitor=false` and
`index.sparse=false`, so it leaves no files in `.git`.

Between reading the status and writing there is a short window in which another git process
could change the index. Just before writing, the backend re-reads the index
(`ls-files -s`) and checks that the touched entries agree with the status and with the first read;
if not, error without writing. Only the instant between that check and the write remains, and only with
concurrent external git writes. The `MemoryBackend` has neither `assume-unchanged` nor
`skip-worktree` flags nor concurrent processes: it uses only the XY code.

`index.stage_lines` and `index.unstage_lines` also replace the staged version of the
file and use the same criterion (`versions_at_risk*`, X ∉ {`.`, `?`} and Y ≠ `.`) on the paths
touched (the requested one and the two of the `FileDiff`): lines of an `MM`, `AM`, `MD`, `AD`, `RM` file
→ first the backup of the staged version; with `.M`, `M.`, `A.`, `.A` (intent-to-add) no,
because the replaced version is also in HEAD or in the worktree. The `CliBackend`, in this order: reads
status (without fsmonitor) and index (`ls-files -s`), then the diff, and checks that the index
entry is the one the diff is computed on (id of the index side: the old one for the stage,
the new one for the unstage; an `intent-to-add` must be there, with the empty blob, a file deleted in
the stage not); reads that blob (`cat-file blob <id>`; absent or `intent-to-add` file: nothing) and
computes the new content with the strict applier, which writes nothing (a patch that does not
apply does not even leave a backup); if `index.lock` exists it gives `IndexLocked` immediately without
making the backup; saves the content as a blob with `hash-object -w --stdin --no-filters` (it is already
in the index form: no `clean` filters or line-ending conversions; it writes only to the
object database); saves the backup; re-checks the index as above; writes the entry with
`update-index -z --index-info` (index mode, or the patch's for a new or
`intent-to-add` file, which becomes a normal entry; mode `0` removes the entry if the file leaves the
index); restores the `--skip-worktree`/`--assume-unchanged` flags the entry had (see
below); finally re-reads the entry and checks it is the one written (if another git changed it
right after, error pointing to the backup). Between the re-check and the write only
`update-index` remains: with the git CLI there is no real compare-and-swap on the index (git takes the
lock only inside `update-index`), so the instant between the re-check and that lock stays uncovered,
as for stage and unstage. Why it is
safe: the index entry is exactly the side of the diff the line numbers come from (same
id), the context is fixed (`--unified=3`), the digest guarantees the diff is the one seen and
the applier never looks for the preimage elsewhere. The worktree is never read or written; the
configuration that could change the result does not come into play (`diff.context` is
neutralized in `DIFF_ARGS`; `apply.*` and `core.whitespace` are no longer needed, because `git apply`
is not used). An `intent-to-add` file (`.A`) has in `git status` mode and id zero but in the index an
entry with the empty blob (in the repo's format): the index re-check accepts it only with
that id (also applies to `index.stage`). Today `Oid` and the parsers support only SHA-1 and a
SHA-256 repo does not open: the SHA-256 id of the empty blob (like that of the empty tree) is there only so as
not to take the format for granted, and is not used.

**`--skip-worktree` and `--assume-unchanged` flags.** `update-index --index-info` and `restore
--staged` recreate the index entry and lose its flags (verified: `S d/f` becomes
`H d/f`). In a sparse checkout a file outside the cone with a staged change would go from
`M ` to `MD`, and a following `git add -A` or `commit -a` would stage a deletion never
made. After writing, `index.stage_lines`, `index.unstage_lines` and `index.unstage` restore the
flags (`update-index --skip-worktree` / `--assume-unchanged`, paths on stdin) on the touched entries
that had them in the initial read (`ls-files -v`: `S`/`s` skip-worktree, lowercase
assume-unchanged) and that are still in the index; if it fails, an error that says so (the entry
written is correct anyway). `index.stage` (`git add`) does not need it: it keeps
`--assume-unchanged` and refuses `--skip-worktree` files (outside the sparse checkout) with an
error, writing nothing.

If a step fails the index is not touched and the command returns the error. The command's result
reports the ref (`"backup"`), the UI shows it in the status bar; the content is
recovered with `git show <ref>:<path>`. Staging a conflicted file makes no backup: git
keeps the conflict versions in the REUC extension of the index (`git checkout -m` redoes the
conflict), and unstaging a conflict is refused. The `MemoryBackend` records backups in
memory (`MemoryBackend::backups`).
