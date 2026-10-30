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

### S27b — Deleting branches on the remote
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `remote.delete_branch` (always `confirm` with `expected`, the commit of the local
  remote-tracking branch; backup `refs/project/backup/push-delete-...`; `--force-with-lease`, never
  `--force`); "Delete from the remote…" in the menu of the remote branches
  ([git/sync.md](git/sync.md)).
- **Done when:** integration tests (confirmation, lease that fails because the branch moved or is
  gone, names with `-`, `pre-push` hook) and on the `MemoryBackend`; `safety-reviewer` review; CI
  green.

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
- [x] **Agent:** `egui-ui`
- **Goal:** tabs with several open repos; list of recent repos saved between one launch and
  the next.
- **Done when:** reopening the app with the previous tabs; CI green.
- Done: one `Tab` per repo with its own executor and `Session` (per-repo undo journal, results
  land in their tab even when hidden), tab strip above the top bar, "Recent" menu and list on
  the empty screen (`crates/ui/src/tabs.rs`); open tabs, the one shown and the recent repos saved
  in the eframe storage as soon as they change, reopened at startup.

### S36 — Huge repos: commit-graph (no `gix`)
- [x] **Agent:** `git-engine` (done), then `egui-ui` (offer in the UI)
- **Goal:** measured `repo.log` on the Linux kernel (1.5M commits): the slowness is all `git log
  --date-order` without a commit-graph (plain `git log` 22 ms), so `gix` is dropped (the order needs
  the whole walk anyway, whoever reads it). Instead `repo.optimize` writes the commit-graph (`git
  commit-graph write --reachable`) and `repo.open` reports `commit_graph`; the UI offers the command
  when the graph is missing and the first log page is slow. Contract and details in
  [git/reading.md](git/reading.md).

  | `repo.log` on the kernel | without commit-graph | with commit-graph |
  | --- | --- | --- |
  | `{"limit":500}` | ≈ 15.8 s (14.3 s) | 42 ms (43-46 ms) |
  | `{"all":true,"stashes":true,"limit":500}` | ≈ 15.8 s (14.2 s) | 1.36 s (1.33-1.38 s) |
  | `{"skip":1000000,"limit":500}` | ≈ 15.8 s (14.7 s) | 0.96 s (0.86-0.91 s) |
  | writing the graph, once | | ≈ 18.8 s (`repo.optimize`: 19.5 s) |

  In brackets: re-measured with the release `project-cli` and `repo.optimize`.
- **Done when:** `repo.optimize` and `commit_graph` with tests on the `MemoryBackend` and on temp
  repos (same `repo.log` before and after); the UI offers `repo.optimize` (in the background,
  cancellable, with progress) when `commit_graph` is `false` and the first page took long; CI green.
- Done (engine): `repo.optimize`, `commit_graph` in `repo.open`, `CliBackend` in
  `crates/git/src/backend/maintenance.rs`, tests in `crates/git/tests/optimize.rs`.
- Done (UI): commit-graph notice under the bar (`crates/ui/src/optimize.rs`,
  `view/optimize_view.rs`, keys in `optimize.ftl`): shown when `commit_graph` is `false` and the first
  log page took > 1 s (`SLOW_FIRST_PAGE`); "Optimize" runs `repo.optimize` with progress and "Stop" in
  the bar, then reloads; "Not now" hides it for the tab. Tests in `crates/ui/src/optimize_tests.rs`;
  checked on the kernel (first page 21 s in the debug app, notice, ~25 s optimize, reload).

### S37 — Packaging: Linux AppImage and Windows zip
- [x] **Agent:** `git-engine`
- **Goal:** `make appimage` builds `target/appimage/BinariGit-<version>-x86_64.AppImage` with a
  pinned, checksum-verified appimagetool; `make windows` cross-compiles (`x86_64-pc-windows-gnu`,
  mingw-w64) `target/windows/BinariGit-<version>-x86_64-windows.zip` (`BinariGit.exe` with the
  icon, `README.txt`). Each with `SHA256SUMS.txt`; git is not bundled. No deb; MSI and signing
  later (see also [ROADMAP.md](../ROADMAP.md)). Details in `packaging/README.md`. GitHub Actions
  (`.github/workflows/package.yml`, both on Ubuntu) uploads them as artifacts and attaches them to
  a Release on `v*` tags.
- **Done when:** the AppImage starts on a clean Ubuntu 22.04 (and 24.04) container; the Windows
  zip builds (tested by hand on a Windows PC); CI green.

## Phase 6 — Polish and distribution

Order decided on 2026-10-24 after analysing the open items of [ROADMAP.md](../ROADMAP.md) (deb left
out on purpose).

### S38 — `cargo deny` in CI
- [x] **Agent:** `git-engine`
- **Goal:** licenses, bans and sources (deterministic, no network) in `cargo xtask ci`;
  advisories (a database that changes every day) in a weekly GitHub job and on pull requests to
  `master`, so a new advisory never blocks a step at random. `deny.toml` comments in English.
- **Done when:** `cargo xtask ci` runs `cargo deny check licenses bans sources`; the advisories
  workflow exists; CI green.
- Done: `deny()` in `xtask/src/main.rs` (skipped with a warning if `cargo-deny` is missing locally,
  error with `CI` set); `make setup` installs it; `ci.yml` installs it with
  `taiki-e/install-action`; `.github/workflows/audit.yml` (Mondays, PRs to master, by hand).

### S39 — Non-blocking file dialogs
- [x] **Agent:** `egui-ui`
- **Goal:** the folder dialog (`rfd::AsyncFileDialog`) no longer blocks the UI thread: the window
  keeps repainting (progress of a fetch or `repo.optimize`) while it is open; parented to the main
  window. Remove `pick_file`/`save_file` from `Platform` (template leftovers, unused).
- **Done when:** on Xvfb the window repaints with the dialog open; CI green.
- **Done:** `NativePlatform` keeps an `AsyncFileDialog` parented to the window (built in the
  eframe creator) and waits for it with `pollster` on a job thread; one dialog at a time
  (`App::picking`); `Platform` lost `pick_file`/`save_file`/`Picked` and the unused
  `Capabilities` fields (only `open_folder` is left).

### S40 — Refresh when the repo changes
- [x] **Agent:** `egui-ui` (watcher in the desktop app), `git-engine` if the engine needs a hook
- **Goal:** the open tabs reload refs, status and log when `.git/` or the worktree change outside
  the app (terminal, an agent through MCP), debounced, ignoring the app's own writes and
  `target/`-like ignored folders. Through `Platform`, implemented in `apps/project` (crate `notify`).
- **Done when:** a `git commit` in a terminal shows up in the app without Refresh; CI green.
- **Done:** `Platform::watch(repo, on_change) -> Option<Watch>` (default `None`); desktop
  (`apps/project/src/watch.rs`): `notify` on the git directory (and `refs/` recursive; for a
  linked worktree also the common directory from `commondir`), no worktree watch; reads, `*.lock`,
  `objects/`, `logs/` ignored; 300 ms debounce (at most 2 s). UI (`crates/ui/src/refresh.rs`):
  per-tab flag, dropped during the tab's write and for 600 ms after it; re-read only for the tab
  shown, not with a dialog open or a command running (stays dirty); focus gained re-reads the tab
  shown (worktree edits). The light refresh re-reads status, history, remotes, stashes, then refs;
  the log only if refs or HEAD changed, as many commits as loaded (`CommitList::reload`), so
  selection and scroll stay. Manual Refresh unchanged.

### S41 — Windows MSI
- [ ] **Agent:** `git-engine`
- **Goal:** after the hand test of the S37 zip on Windows, an unsigned MSI built on Linux with
  `wixl` (`msitools`): per-user install, Start menu shortcut, icon, uninstall; in `package.yml`.
- **Done when:** the MSI installs and uninstalls on a Windows PC (by hand); CI green.
- **Status:** built, waiting for the hand test on Windows (install, Start menu, upgrade over an
  older MSI, uninstall). `make windows` also builds `BinariGit-<version>-x86_64.msi` from
  `packaging/binarigit.wxs` (per-user in `%LOCALAPPDATA%\Programs\BinariGit`, no dialogs,
  `MajorUpgrade`); `package.yml` uploads it and attaches it to the Release. Details in
  `packaging/README.md`.

### S42 — macOS build (unsigned DMG)
- [ ] **Agent:** `git-engine`
- **Goal:** only if a Mac is available for testing: `.app` bundle and DMG built on a
  `macos-latest` runner (free for the public repo), universal or aarch64.
- **Done when:** the DMG opens and the app starts on a Mac (by hand); CI green.

### S43 — Code signing (Windows and macOS)
- [ ] **Agent:** `git-engine`
- **Goal:** when the costs are decided: Windows signing from Linux (`jsign`/`osslsigncode` with a
  cloud certificate, e.g. Certum Open Source or Azure Trusted Signing); macOS signing and
  notarization (`rcodesign`, Apple Developer Program). Secrets only in GitHub, never in the repo.
- **Done when:** signed packages from `package.yml`; SmartScreen/Gatekeeper checked by hand.

### S44 — UI control for agents (only if still needed after S40)
- [ ] **Agent:** to be decided
- **Goal:** as in `ARCHITECTURE-rust.md` §6: off by default, loopback only, token in a file
  readable only by the user; drive the open app (tabs, selection, screenshots). Writes keep the
  same confirmations as the UI.
- **Done when:** to be defined with the design.

## Phase 7 — Repository management

Gaps compared with the big clients (GitKraken & co.), in order of daily use. Each command goes in
the file of its function in [git/](git/) before the code (contract, `CliBackend`, undo and backup).

### S45 — Reset of the current branch
- [x] **Agent:** `git-engine`, then `egui-ui`; `safety-reviewer`
- **Goal:** `branch.reset {"target", "mode": "soft"|"mixed"|"hard", "confirm"?}` moves the current
  branch (or a detached HEAD) to `target` (a rev) in the [git/rules.md](git/rules.md) style; new file
  `git/reset.md`. `soft` touches only the ref; `mixed` also the index, with the index backup of
  stage/unstage saved on its own; `hard` with changes in the worktree or index asks for `confirm`
  and first saves them as in `worktree.discard` (`stash create` + byte-for-byte backup,
  `refs/project/backup/reset-...`; no backup → nothing is written; untracked files are not
  touched). Refused during an operation in progress (merge, rebase, cherry-pick, revert, bisect),
  on a branch without commits and on a `target` that is not a commit. A `JournalEntry` with HEAD
  and the branch before and after, so `edit.undo` puts the ref back (the worktree of a `hard`
  stays in the backup, and the undo says so). Result with `warnings` if the commits that leave
  the branch are already on its upstream (the next push will need `--force-with-lease`).
  UI: "Reset <branch> to here" in the commit menu with the three modes and the confirmation
  dialog listing the files that a `hard` overwrites.
- **Status:** engine and backend done (`branch.reset`, `branch.reset_preview`, [git/reset.md](git/reset.md));
  UI done ("Reset … to here…" in the commit menu, dialog with the three modes and the plan of
  `branch.reset_preview`: `crates/ui/src/reset.rs`, `view/reset_view.rs`); `safety-reviewer` review done (hidden files and
  replaced tracked folders checked before the ref moves).
- **Done when:** integration tests (three modes, `hard` with changed, staged, CRLF and conflicted
  files, detached HEAD, `target` that is not an ancestor, undo/redo, refs changed from outside
  between confirmation and write) and on the `MemoryBackend`; `safety-reviewer` review; CI green.

### S46 — Clone and new repository
- [x] **Agent:** `git-engine`, then `egui-ui`; `git-scenarios` for the edge cases
- **Goal:** `repo.clone {"url", "path", "branch"?, "depth"?}` and `repo.init {"path",
  "branch"?}`, commands without an open repo (as `repo.open`) that open the new repo at the end;
  new file `git/clone.md`. Destination absent or an empty directory, never over existing files
  (`NotAllowed`). Clone in the background with progress from stderr (`--progress`), cancellable,
  the same recognized authentication errors as `remote.fetch` (`GIT_TERMINAL_PROMPT=0`: the
  credentials come from the user's git configuration); `--end-of-options` before the URL; with a
  cancel or an error the folder is removed only if the command created it. `repo.init` with
  `git init` (initial branch from `branch` or from `init.defaultBranch`). UI: "Clone…" and
  "New repository…" in the File menu and on the empty screen (URL, folder with the non-blocking
  dialog of S39, optional branch), the repo opens in a new tab and goes in the recent ones.
- **Status:** engine and backend done (`repo.clone`, `repo.init`, [git/clone.md](git/clone.md):
  `RepoOpener::clone_repo`/`init_repo`, `Session::clone_repo`/`init_repo`, refused over MCP); UI
  done (`crates/ui/src/clone.rs`, `view/clone_view.rs`: File menu and empty screen, dialog with
  "Browse…", progress and "Cancel" in the dialog, the clone runs in a tab's session that joins the
  tabs and the recent list when the repo is open).
- **Done when:** tests with a local remote (`file://` and a path), a URL starting with `-`, a
  non-empty destination, a cancel midway (no folder left), `depth`, an empty remote; init with
  and without `branch`; CI green.

### S47 — Tags: delete and push
- [x] **Agent:** `git-engine`, then `egui-ui`; `safety-reviewer`
- **Goal:** next to `tag.create` ([git/branch.md](git/branch.md)): `tag.delete {"name",
  "confirm"}` (always `confirm`; the tag object of an annotated tag saved in
  `refs/project/backup/tag-delete-...` before deleting, `JournalEntry` so undo recreates it on the
  same value). In [git/sync.md](git/sync.md): `remote.push_tag {"remote", "name"}` (only
  `refs/tags/<name>`, never `--force`: a tag already on the remote with another value is a clear
  error) and `remote.delete_tag {"remote", "name", "expected", "confirm"}` as
  `remote.delete_branch` (`--force-with-lease` with the expected value, backup
  `refs/project/backup/push-delete-...`). UI: tag menu in the refs column ("Push to <remote>",
  "Delete…", "Delete from the remote…").
- **Done when:** integration tests with a local remote (lightweight and annotated tag, tag already
  on the remote with the same or another value, lease that fails, names with `-`, undo/redo of the
  local deletion) and on the `MemoryBackend`; `safety-reviewer` review; CI green.

### S48 — Search in the history
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `repo.search {"query", "in"?: ["message", "author", "id"], "path"?, "all"?, "skip"?,
  "limit"?}` (read-only, in [git/reading.md](git/reading.md)) returns the ids of the matching
  commits in the same `--date-order` as `repo.log`, so the graph stays whole: the search
  highlights, it does not filter. Literal text (`--fixed-strings`, `-i`), never a regex from the
  user; `id` = prefix of the hash; `path` with `--literal-pathspecs`. UI: search field (Ctrl+F)
  above the list, matches highlighted, "next"/"previous" with `CommitList::seek` (also on pages
  not loaded yet), number of matches; Escape closes.
- **Done when:** tests (regex characters taken literally, non-UTF-8 message, author by name and
  by email, short hash, `path` with a rename, `all` with an unmerged branch, contiguous pages) and
  on the `MemoryBackend`; desktop screenshot on Xvfb with highlighted matches; CI green.

### S49 — Settings window
- [x] **Agent:** `egui-ui`
- **Goal:** today the preferences are scattered (theme in a title bar menu, unified/side by side
  in the diff view) and the language cannot be chosen (`project_i18n::set_lang` exists, the app
  always starts in English). A "Settings…" window (File menu in the title bar, Ctrl+,) with the
  sections **General** (theme: as the system / light / dark; language: as the system / Italiano /
  English, applied at once) and room for the next ones. One `Settings` struct in `crates/ui`, saved in
  the eframe storage under its own key as the tabs (`crates/ui/src/tabs.rs`), written only when it
  changes, unknown or missing fields → defaults (`#[serde(default)]`, so older saves keep working).
  The theme menu stays as a shortcut to the same setting; the system language comes through
  `Platform` (implemented in `apps/project`), `None` → English. Settings never go through the
  `Registry`: they do not touch the repo. Texts in `settings.ftl`.
- **Done when:** UI tests (round trip of the storage, an old save without the new fields, language
  applied without restarting); desktop screenshot on Xvfb of the window in Italian; CI green.

### S50 — Auto fetch
- [x] **Agent:** `egui-ui` (`git-engine` only if `remote.fetch` needs a change)
- **Goal:** each open tab runs `remote.fetch` on a timer, interval in the S49 settings
  (**Remotes** section: off / 1 / 5 / 15 minutes, default 5). Only fetch, never pull: local
  branches and files are not touched. Skipped (and retried at the next tick) with a dialog open, a
  command running or an operation in progress (merge, rebase, cherry-pick, revert); hidden tabs
  fetch when shown again if their interval has passed; never two fetches of the same tab at once.
  Authentication error (`GIT_TERMINAL_PROMPT=0`, the credentials come from the git
  configuration): auto fetch suspended for that tab, one notice under the bar with "Retry",
  no dialog at every tick. Network error: doubled wait up to 30 minutes, then back to the
  interval; only an icon in the bar. After a fetch the S40 light refresh (selection and scroll
  stay) and, if remote branches moved, a discreet notice ("3 new commits on origin/main"). A
  manual Fetch restarts the timer.
- **Done when:** UI tests with a fake clock (interval, skips, backoff, suspension on an
  authentication error and Retry, hidden tab); by hand on Xvfb: a commit pushed to a local
  remote from a terminal shows up in the graph without pressing Fetch; CI green.

### S53 — Compare two revisions
- [ ] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** `diff.commit` only compares a commit with its parent. Add the read-only `diff.range
  {"from", "to", "merge_base"?: bool, "path"?}`: the diff of `to` against `from` (`git diff from
  to`), or with `merge_base: true` what `to` brings since it diverged from `from` (`from...to`, i.e.
  against the merge base; no merge base → clear error). Same result shape, renames handling, size
  limits and binary handling as `diff.commit` (same parser and code path), plus `{"from", "to",
  "base"?}` as full oids. Without `path`: every changed file with its hunks, as `diff.commit`. Revs
  after `--end-of-options`, paths after `--` with `--literal-pathspecs`; a rev that is not a commit
  → `InvalidParams`. Docs in `impl-docs/git/reading.md`, examples in `impl-docs/git/cli.md`, README
  command list. UI: Ctrl+click a second commit in the list → details panel "Compare A ↔ B" with
  files and diff; "Compare with the current branch" in the branch menu (`merge_base: true`).
- **Done when:** integration tests (two unrelated commits, merge_base with diverged branches, no
  common base, rename, binary, path filter, rev starting with `-`, tag and branch names) and on the
  `MemoryBackend`; desktop screenshot; CI green.
- **Status:** engine part done (`diff.range`, `GitRead::commit_id`/`common_base`,
  `DiffSpec::Range`; tests in `crates/git/tests/range.rs` and `crates/engine/src/builtin/range_tests.rs`);
  UI part (`egui-ui`) to do.
