# Git client: step-by-step roadmap

Architecture in [ARCHITECTURE-git.md](ARCHITECTURE-git.md) and, per function, in [git/](git/). Each step is small, closes with
a green `cargo xtask ci` and leaves the project working. The **Goal** field can be used as is
as the goal of a session; **Done when** says how to verify it. The `§` references point to sections of the architecture.

Rules that apply to every step:

- Work only on the indicated crates; if others need touching, say so first.
- New dependencies go in the `[workspace.dependencies]` of the root `Cargo.toml`, checking the
  current version.
- Update the step's checkbox in this file when it is finished.
- Steps that write to the user's repo (from S18 on) go through `safety-reviewer` before
  delivery.
- Delivery of every step: green `cargo xtask ci` locally, pull request to `dev` and squash
  merge. For now GitHub CI only runs on pull requests to `master`, not on those to
  `dev`. The next step restarts from the updated `dev`; `dev` will go into `master` with a pull
  request.
- **Desktop only** (decision of 2026-10-24, S31d): the wasm app (`project-web`) has been removed;
  it only served as a demo on the `MemoryBackend` and as a test bench with Playwright. The UI is
  explored on the desktop app on Xvfb (`make demo-repo`, `make desktop-start`, `xdotool`; see
  `AGENTS.md`, "Seeing the app"). Steps marked done before S31d may mention the web version: it no
  longer exists.

## Phase 0 — Preparation

### S00 — CI on the main branch
- [x] **Agent:** `git-engine`
- **Goal:** the `.github/workflows/ci.yml` workflow listens to pushes on `main`, but the repo's
  main branch is `master`. Make it run on pushes to `master`.
- **Done when:** the `push` trigger of `ci.yml` names `master`.

## Phase 1 — Reading a repo

### S01 — Git types in the domain
- [x] **Agent:** `git-domain`
- **Goal:** add to `crates/domain` a `git` module with `Oid` (hexadecimal parsing,
  `Display`, `serde` as a string), `Signature`, `Commit`, `RefKind`, `RefInfo`, `Upstream`,
  `Head`. The "list of entries" example stays as it is.
- **Done when:** tests on parsing valid and invalid `Oid`s and JSON round-trip; CI green.

### S02 — Log parser
- [x] **Agent:** `git-domain`
- **Goal:** pure function `parse_log(&[u8]) -> Result<Vec<Commit>, DomainError>` for the
  fixed-field `-z` format described in §3, plus the constant with the `--format` string.
- **Done when:** tests with output captured from a real repo (merge commit, root commit,
  empty body, multi-line body, non-UTF-8 author); CI green.

### S03 — Status parser
- [x] **Agent:** `git-domain`
- **Goal:** `Status`, `StatusEntry`, `Operation` and `parse_status(&[u8])` for
  `git status --porcelain=v2 -z --branch --untracked-files=all`.
- **Done when:** tests on modified, staged, renamed, untracked, conflicted files,
  detached HEAD, branch without upstream, ahead/behind; CI green.

### S04 — Ref parser
- [x] **Agent:** `git-domain`
- **Goal:** `git for-each-ref` format (constant) and `parse_refs(&[u8])` for local branches,
  remotes, tags and stash, with upstream and ahead/behind.
- **Done when:** tests with captured output; CI green.

### S05 — Backend trait and `MemoryBackend`
- [x] **Agent:** `git-engine`
- **Goal:** in `crates/engine` the `GitBackend` trait (read-only: `head`, `refs`, `log`,
  `status`) and `RepoOpener`, plus `MemoryBackend` with a sample repo constructor (a
  dozen commits with two branches and a merge).
- **Done when:** tests that read the sample repo from the `MemoryBackend`; CI green.

### S06 — `project-git` crate with read-only `CliBackend`
- [x] **Agent:** `git-engine`
- **Goal:** new crate `crates/git` (`project-git`, L6, declared in
  `xtask/src/layers.rs`) with `CliBackend` and `CliOpener` implementing the S05 traits by running
  `git` according to the rules of §5 and using the S02–S04 parsers. Minimum version check.
- **Done when:** integration tests on a temporary repo created by the test (see §9);
  `cargo xtask layers` passes; CI green.

### S07 — Helpers for test scenarios
- [x] **Agent:** `git-scenarios`
- **Goal:** in `crates/git/tests/common/` a helper that creates temporary repos in a
  deterministic environment (§9) with functions like `commit_file`, `branch`, `merge`, `conflict`. Rewrite
  the S06 tests with the helper.
- **Done when:** the tests use the helper and pass twice in a row with the same result;
  CI green.

### S08 — Repo session and read commands
- [x] **Agent:** `git-engine`
- **Goal:** `Session` holds the open backend and the injected `RepoOpener`. Commands
  `repo.open`, `repo.status`, `repo.refs`, `repo.log` (paginated). Remove the "list of
  entries" example (`Document`, `item.*`, `document.*`): it is an intended change of the catalog, to be noted in the
  commit. Update UI and apps just enough for them to compile (the UI shows a temporary
  message).
- **Done when:** command tests on the `MemoryBackend`; `project-cli commands` shows the new
  catalog; CI green.

### S09 — CLI and MCP on the repo
- [x] **Agent:** `git-engine`
- **Goal:** `project-cli --repo <path> run --cmd repo.status` and `eval` use
  `CliOpener`; `mcp --root <dir>` opens the repo at the root and confines paths there.
  Replace `open_document`/`save_document` with `open_repo`. Update README and `AGENTS.md`.
- **Done when:** end-to-end tests in `apps/project-cli/tests` on a temporary repo; CI green.

## Phase 2 — Seeing

### S10 — Background work
- [x] **Agent:** `egui-ui`
- **Goal:** `Platform::spawn` (§6) with a thread implementation on desktop and immediate
  execution on the web. In the UI, an executor that sends commands to the shared session and receives
  the results from a channel, with a visible "in progress" state.
- **Done when:** engine or UI test with a slow fake command; the UI stays responsive;
  CI green.

### S11 — Main layout
- [x] **Agent:** `egui-ui`
- **Goal:** top bar, left column with the refs, commit list in the center (without
  graph), details panel on the right (§8). Desktop: opening a repo with the folder dialog.
  Web: `MemoryBackend` with the sample repo.
- **Done when:** `make run` opens a real repo and shows branches and commits; screenshot of the web
  build with Playwright; CI green.

### S12 — Virtualized commit list
- [x] **Agent:** `egui-ui`
- **Goal:** `ScrollArea::show_rows` with page-by-page loading of `repo.log` while
  scrolling; selection with mouse and keyboard.
- **Done when:** a repo with at least 50,000 commits scrolls smoothly (manual check, noted in the
  commit); CI green.

### S13 — Graph lanes
- [x] **Agent:** `git-domain`
- **Goal:** lane algorithm (§3) as a pure function, with property tests
  (`proptest`) on the invariants and fixed cases (linear, branch, merge, octopus merge, several roots).
- **Done when:** tests pass; CI green.

### S14 — Drawing the graph
- [x] **Agent:** `egui-ui`
- **Goal:** draw lanes and edges next to the commit list, with one color per lane
  and the ref labels on the commit's row.
- **Done when:** web screenshot of the sample repo with a visible merge; CI green.

### S15 — Commit details
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `commit.show` command (metadata and files changed with `git diff-tree`) and details
  panel in the UI.
- **Done when:** command test on a temporary repo; the panel shows the data of the selected
  commit; CI green.

### S16 — Diff parser
- [x] **Agent:** `git-domain`
- **Goal:** `Diff`, `FileDiff`, `Hunk`, `Line` and `parse_diff(&[u8])` for unified diff,
  with renames, new and deleted files, binaries, "no newline at end of file".
- **Done when:** tests with captured output; CI green.

### S17 — Diff view
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `diff.worktree`, `diff.index`, `diff.commit` commands; view with line numbers,
  colors for additions and removals, hunk headers.
- **Done when:** command tests; the diff opens from the details panel; CI green.

## Phase 3 — Changing locally

### S18 — Stage and unstage of files
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `index.stage`, `index.unstage` and stage area in the UI ("uncommitted changes" row
  at the top of the list).
- **Done when:** integration tests; `safety-reviewer` review; CI green.

### S19 — Patch builder
- [x] **Agent:** `git-domain`
- **Goal:** patch builder for selected lines (§3), in both directions
  (stage and unstage).
- **Done when:** `insta` snapshots and tests that apply the patch with `git apply --check` on a
  temporary repo; CI green.

### S20 — Stage by hunk and by line
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `index.stage_lines`, `index.unstage_lines`: builder patch applied
  by a strict applier in the engine (each hunk only at its position) and written with
  `hash-object --no-filters` / `update-index --index-info`; hunk and line selection in the
  diff view.
- **Done when:** integration tests on files with several hunks; `safety-reviewer` review;
  CI green.

### S21 — Commit
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `commit.create` (message via stdin, `amend` option), with hooks and signing
  according to the user's configuration; message box in the stage area.
- **Done when:** integration tests, also with a failing `pre-commit` hook; CI green.

### S22 — Operation journal and undo
- [x] **Agent:** `git-engine`
- **Goal:** journal (§7) for the commands that move refs; `edit.undo` and `edit.redo` with
  a check that the refs have not changed in the meantime.
- **Done when:** tests: commit then undo brings the branch back to the previous commit; undo refused
  if the ref was moved from outside; `safety-reviewer` review; CI green.

### S23 — Discarding changes with backup
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `worktree.discard` with `confirm`, `git stash create` backup and backup ref
  (§7), undoable with `edit.undo`; confirmation in the UI.
- **Done when:** tests: discard, then undo restores the file identical; without `confirm` nothing
  happens; `safety-reviewer` review; CI green.

### S24 — Branches
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `branch.create`, `branch.checkout` (clear error with conflicting changes),
  `branch.delete` (`confirm` if not merged), `branch.rename`; context menu in the refs
  column.
- **Done when:** integration tests, also with names starting with `-`; `safety-reviewer` review;
  CI green.

## Phase 4 — Remotes

### S25 — Fetch
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `remote.fetch` in the background with progress from stderr (`--progress`) and recognized
  authentication errors; button in the bar.
- **Done when:** tests with a local remote (`git init --bare` in a temporary folder); CI green.

### S25b — Graph of all refs
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** the graph also shows the commits of unmerged branches and those just downloaded
  on `origin/*`: `repo.log {"all": true}` starts from local and remote branches, tags and HEAD (also
  detached), without stashes or backups; the UI uses it for the graph.
- **Done when:** integration tests (unmerged branch, remote branch after a fetch, stash and
  backups excluded, detached HEAD, contiguous pages) and on the `MemoryBackend`; CI green.

### S26 — Pull and push
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `remote.pull` (strategy according to the configuration: merge or rebase) and
  `remote.push` (setting the upstream on the first push; `--force-with-lease` only with
  `confirm`).
- **Done when:** tests with a local remote, also a push rejected because it is not fast-forward;
  `safety-reviewer` review; CI green.

### S27 — Remote management
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `remote.list`, `remote.add`, `remote.remove`; ahead/behind visible next to the
  branches.
- **Done when:** integration tests; CI green.

## Phase 5 — Advanced features

### S28 — Stash
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `stash.push`, `stash.apply`, `stash.pop`, `stash.drop` (`confirm`); stashes in the
  left column and in the graph.
- **Done when:** integration tests; `safety-reviewer` review; CI green.

### S29 — Merge and the "in progress" state
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `merge.start`, `merge.abort`, `merge.continue`; banner in the UI when the repo is
  in merge, rebase, cherry-pick or revert, with the conflicted files.
- **Done when:** tests with the S07 conflict scenario; CI green.
- [x] `git-engine` part: commands `merge.start`, `merge.continue`, `merge.abort` and the
  `repo.operation` read for the banner ([git/merge.md](git/merge.md)).
- [x] `egui-ui` part: in-progress operation panel with the conflicted files, "Continue the
  merge…" and "Abort the merge…" (with confirmation), "Merge into «branch»" from the branch, tag and
  commit menus; web demo with a conflicting branch (§8 of the architecture).
- [x] Follow-up (`git-engine`): `remote.pull` in a merge that stops on conflict leaves the merge in
  progress like `merge.start` (`in_progress`, `conflicts`; you leave with `merge.continue` or
  `merge.abort`) instead of aborting it; in a rebase it stays aborted until S32
  ([git/sync.md](git/sync.md)).

### S30 — Conflict editor
- [x] **Agent:** `egui-ui`, with `git-domain` for the marker parser
- **Goal:** three-part view (base, ours, theirs) and editable result; choice per
  block; marks as resolved with `index.stage`.
- **Done when:** conflict marker parser tests; complete resolution of a
  conflict from the UI; CI green.

### S30b — Name and visual identity
- [x] **Agent:** none (small, done in session)
- **Goal:** the app is called **BinariGit**: name visible in the desktop window, in the web
  `<title>`, in the CLI and in the README; icon, logo and banner in `assets/` (source SVGs, PNGs
  generated with `assets/render.sh`); window icon (`viewport.icon`) and web favicon.
  The technical names stay: crates and binaries `project-*`, refs `refs/project/backup/...`, backup
  identity, command ids.
- **Done when:** the desktop and web apps show name and icon; CI green.

### S31 — Cherry-pick and revert
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `commit.cherry_pick`, `commit.revert` from the commit's context menu, with
  the conflict handling of S29.
- **Done when:** integration tests; CI green.
- [x] `git-engine` part: commands `commit.cherry_pick`, `commit.revert`, `cherry_pick.continue`,
  `cherry_pick.abort`, `revert.continue`, `revert.abort` and `continue`/`abort` of
  `repo.operation` for cherry-pick and revert ([git/cherry-pick.md](git/cherry-pick.md)).
- [x] `egui-ui` part: "Cherry-pick onto «branch»" and "Revert" in the commit menu (parent
  dialog for a merge commit); operation panel with the `continue`/`abort` commands of
  `repo.operation` for merge, cherry-pick and revert (`crates/ui/src/pick.rs`).

### S31b — Languages (i18n): infrastructure and UI
- [x] **Agent:** `egui-ui` (infrastructure and first modules), then several `egui-ui` in parallel on the
  remaining modules
- **Goal:** the app in **Italian and English** with [Fluent](https://projectfluent.org/).
  New crate `project-i18n` (L0, no dependency on project crates): `.ftl` files per
  language and per area (`crates/i18n/locales/{it,en}/<area>.ftl`, included with `include_str!`, so
  it also works in wasm), `Lang` (`It`, `En`), translation with key and parameters (Fluent
  plurals: "1 file" / "3 files"). App language: **English**, not selectable; Italian stays
  as a second Fluent language. All the egui UI texts go through keys.
  These stay as they are: CLI, MCP, descriptions of the catalog commands, documents, git text.
- **Done when:** tests that the two languages have the same keys and that every key used in
  code exists; no visible text written in the UI code; screenshot in English; CI
  green.
- [x] UI migrated: crate `project-i18n` (Fluent, `tr!`), one `.ftl` area per function in
  `crates/i18n/locales/{it,en}/` (including `app` and `common`), app always in English, Italian
  only as a second Fluent language ([i18n.md](i18n.md)). The engine errors remained to do (S31c).

### S31c — Languages (i18n): engine error messages
- [x] **Agent:** `git-engine` — `Message` (key + parameters, `msg!` macro) in `EngineError`,
  `engine*.ftl` areas in it/en, `GitWithReason`, key and parameter tests; the UI uses
  `EngineError::text()`. `warnings` and journal descriptions are also `Message`s (in JSON
  `{"text", "key", "args"}`). All the program's text (UI, CLI, MCP, catalog, core,
  domain, `MemoryBackend`) is in English; Italian exists only as a second Fluent language
  (`crates/i18n/locales/it`). The whole repository (program text, comments, tests, docs) is now
  in English; the `test-italiano` feature is gone and the UI tests run in English.
- **Goal:** the `EngineError` errors shown to the user (`NotAllowed`,
  `ConfirmRequired`, `InvalidParams`... ) carry a key and parameters instead of the already
  composed text; `Display` is the English text (CLI, MCP, tests), the UI shows them in its
  language. Git's text (`stderr`) stays as it is.
- **Done when:** no error message composed with `format!` in the engine and backends;
  tests that every key exists in the two languages; CI green.

### S31d — Desktop only: wasm removed
- [x] **Agent:** none (small, done in session)
- **Goal:** remove `apps/project-web`, the wasm clippy in `cargo xtask ci`, trunk and the web
  targets of the `Makefile`; explore the UI on the desktop app instead: `scripts/demo-repo.sh`
  (`make demo-repo`: real repo with merge, tag, remote ahead, stash, the branch `experiment`
  in conflict and local changes), `make desktop-start`/`desktop-stop` (app on Xvfb `:99`
  with focus, driven with `xdotool` and photographed with `import`).
- **Done when:** CI green; the app explored with clicks and typing on the demo repo.

### S32 — Interactive rebase
- [x] **Agent:** `git-domain` (plan), `git-engine` (execution)
- **Goal:** rebase plan (pick, reword, squash, fixup, drop, reordering) as pure data;
  execution with `GIT_SEQUENCE_EDITOR` writing the plan; `rebase.abort` and `rebase.continue`.
  With these, `remote.pull` in a rebase stopped on conflict will also stay in progress instead of
  aborting (like the merge already does, [git/sync.md](git/sync.md)).
- **Done when:** integration tests for every plan action; `safety-reviewer` review;
  CI green.
- [x] `egui-ui` addition: "Continue rebase…"/"Abort rebase…" in the operation panel (`Kind::Rebase`
  in `crates/ui/src/merge.rs`) and the pull notices for a stopped rebase, so a pull rebase left in
  progress can be concluded from the app.

### S33 — Rebase by dragging
- [x] **Agent:** `egui-ui`
- **Goal:** reorder, combine and delete commits by dragging them in the list, with a plan
  preview before running.
- **Done when:** web screenshot with the plan in preview; the rebase run from the UI gives the
  same result as the S32 test; CI green.

### S34 — Blame and file history
- [x] **Agent:** `git-domain` (`--porcelain` parser), then `git-engine` and `egui-ui`
- **Goal:** `file.blame`, `file.log` (with `--follow`) and their views.
- **Done when:** parser and command tests; CI green.
- Done: parsers, commands and `CliBackend`; UI with "Blame"/"File history" in the file menus of
  the commit details and the stage area, a view in place of the graph (`crates/ui/src/file.rs`,
  `view/file_view.rs`), requests in the background cancelled when replaced.

### S35 — Multiple repos and recent repos
- [ ] **Agent:** `egui-ui`
- **Goal:** tabs with several open repos; list of recent repos saved between one launch and
  the next.
- **Done when:** reopening the app with the previous tabs; CI green.

### S36 — Huge repos with `gix`
- [ ] **Agent:** `git-engine`
- **Goal:** measure `repo.log` on the Linux kernel; if slow, read the log with `gix` in the
  `CliBackend` keeping the same result.
- **Done when:** timing comparison noted in the commit; the integration tests give the
  same result with the two readings; CI green.

### S37 — Linux packaging
- [ ] **Agent:** `git-engine`
- **Goal:** AppImage and deb package (see also [ROADMAP.md](../ROADMAP.md)) with
  `SHA256SUMS.txt`.
- **Done when:** the package installs and starts on a clean Ubuntu; CI green.
