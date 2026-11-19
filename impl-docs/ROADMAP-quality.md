# Code quality: step-by-step roadmap

Cleanups from the code review of 2026-10-25 (dead code, repetitions, simplifications, missed
best practices). The feature roadmap stays in [ROADMAP-git.md](ROADMAP-git.md); its rules apply
here too: each step is small, closes with a green `cargo xtask ci`, leaves the project working
and changes **no behavior** unless the step says so. The **Goal** field can be used as is as the
goal of a session; **Done when** says how to verify it.

Rules that apply to every step:

- Start from the latest `origin/dev` (see `AGENTS.md`, "Branches").
- Command ids, the JSON of the results and the texts shown do not change, unless the step
  says so: CLI, MCP and the saved journal depend on them.
- Steps that touch code writing to the user's repo go through `safety-reviewer`.
- Update the step's checkbox in this file when it is finished.

## Phase Q0 — Done (branch of the review)

### Q00 — `MemoryBackend` only in tests
- [x] **Agent:** `git-engine`
- **Goal:** `MemoryBackend`, `MemoryOpener`, the sample repo and the git emulations used only by
  them (~9k lines) were compiled into the app binaries. Feature `test-support` of
  `project-engine`, enabled only in the `[dev-dependencies]` of `project-ui` and `project-mcp`.
- **Done when:** `cargo check -p project -p project-cli` builds without the memory modules; CI
  green.

### Q01 — Dead code, stale docs, `cargo doc` in CI
- [x] **Agent:** `git-engine`
- **Goal:** remove `JobContext::cancel_token`, the unused `unic-langid` dependency and the empty
  `[features]` of `project-i18n`; fix the broken intra-doc links; `cargo xtask ci` runs
  `cargo doc --document-private-items` with `-D warnings`; `rust-version = "1.95"` (egui 0.37).
- **Done when:** `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
  --document-private-items` is clean; CI green.

### Q02 — Engine: shared result helpers, strategy as an enum
- [x] **Agent:** `git-engine`
- **Goal:** `put_reread` (value or `null` + `<key>_error`, 20 copies) and `put_journal_state`;
  `index.stage`/`index.unstage` share `index_paths`; no `to_json` inside `json!`;
  `JournalEntry::merge`/`pull` take `Integration` instead of a string re-matched with a fallback.
- **Done when:** engine tests unchanged and green; CI green.

### Q03 — UI: one decoder, shared texts, `receive` as a `match`
- [x] **Agent:** `egui-ui`
- **Goal:** `crate::decode`/`unexpected_result`/`warnings_text` replace 4 `parse<T>`, 26 inline
  `map_err` blocks and 3 `warnings_text`; the 27 i18n keys with the same text in every area
  become `common-*`; `Tab::receive` is an exhaustive `match` over `Request`.
- **Done when:** UI tests green; CI green.

### Q04 — Ref prefixes in the domain
- [x] **Agent:** `git-domain`
- **Goal:** `HEADS_PREFIX`, `REMOTES_PREFIX`, `TAGS_PREFIX`, `branch_short`, `remote_short`,
  `tag_short` in `project-domain` (re-exported by the engine); ~60 literals and
  `strip_prefix(..).unwrap_or(..)` copies use them; engine's `short(oid)` was `Oid::short`.
- **Done when:** no `"refs/heads/"`-style literal left in production code outside the domain; CI
  green.

### Q05 — Lints and a panic-safe `Registry::run_with`
- [x] **Agent:** `git-engine`
- **Goal:** workspace lints `clippy::redundant_clone` and `unused_qualifications`; match arms with
  the same body merged; `run_with` restores the empty `JobContext` also when a command panics.
- **Done when:** test of the panicking command; CI green.

### Q06 — git 2.43: undo of merge/pull/pick, `branch.create` on a dangling symref
- [x] **Agent:** `git-engine` + `safety-reviewer`
- **Goal:** bugs found while running the tests with git 2.43 (above the supported minimum 2.30):
  `reset_keep` passed `--end-of-options` to `git reset`, which that git refuses, so every
  `edit.undo` of a merge, pull, cherry-pick, revert or rebase failed; `update-ref --no-deref`
  with an empty old value overwrote a symbolic ref pointing nowhere. The target of the reset is a
  full id (no option needed); the symbolic ref is checked with `git symbolic-ref -q` first.
- **Done when:** the 25 tests of `project-git` and `project-cli` that failed with git 2.43 pass;
  CI green.

## Phase Q1 — Contracts between the layers

### Q10 — git versions in CI
- [x] **Agent:** `git-engine`
- **Goal:** Q06 shows that the integration tests only ran with one git version. A GitHub Actions
  job runs `cargo test -p project-git -p project-cli` with git 2.30 (the minimum,
  `GitVersion::MIN`) and with the latest release, on Linux (a container image or git built from
  source, cached). The existing job is unchanged.
- **Done when:** the matrix runs on pull requests; a deliberately broken flag (try locally)
  fails on the old git; CI green.
- **Done:** `.github/workflows/git-versions.yml` (`git min` = latest 2.30.x, `git latest`; git
  built by `.github/scripts/build-git.sh`, cached by version). Git 2.30 found two real flags
  it refuses: `--diff-merges=first-parent` (2.31) in `file.log`, now `-m` with the first record
  of a merge kept, and `--no-update-refs` (2.38) in `rebase.start`, now only
  `rebase.updateRefs=false`; the tests no longer rely on `GIT_CONFIG_COUNT`/`GIT_CONFIG_GLOBAL`
  alone (2.31/2.32).

### Q11 — Typed command results shared by engine and UI
- [x] **Agent:** `git-engine`, then `egui-ui`
- **Goal:** today the handlers build their result by hand (`json!`, `out["x"] = ...`) and the UI
  redefines ~32 `*Result` structs that mirror them: the schema lives twice and a mismatch shows
  only at runtime. Each command gets a `#[derive(Serialize, Deserialize)]` result type in the
  engine (in the file of its function); the handler returns it, the UI deserializes into the same
  type and its mirror struct is removed. First, `insta` snapshot tests of the JSON of every
  command on the `MemoryBackend` sample, so the output provably does not change. One function
  per pull request (branch, tag, stash, sync, merge, rebase, pick, reset, remotes, discard,
  commit, reads).
- **Done when:** for the migrated function, no `json!`/`out[...]` left in its handlers and no
  mirror struct in `crates/ui`; snapshots unchanged; CI green.
- **Done:** 94 snapshots of the 61 commands (`builtin/snapshot_tests.rs`), then one pull request
  per function. Every handler returns a typed result exported by `project-engine` (shared
  `JournalState` flattened, `reread` for the `<key>`/`<key>_error` pairs); continue/abort of
  cherry-pick, revert and rebase reuse `MergeContinueResult`/`MergeAbortResult`. The UI decodes
  into them: its only `Deserialize` structs left are settings and saved tabs; `engine_text` is
  gone. Snapshots never re-accepted.

### Q12 — Typed registry and parameters
- [x] **Agent:** `git-engine`
- **Goal:** every handler starts with `parse_params("<id>", p)?` and repeats its id (37 `const
  ID`); `WRITES` repeats the ids next to the registration. Generic registration
  `add::<P: DeserializeOwned>(id, title, writes, handler)`: the `Registry` deserializes the
  parameters with the right id for the error and the handler receives `P`; `writes` is an
  argument of the registration and `WRITES` goes away. The parameter structs become `pub` and
  `Serialize`, so the UI builds parameters with them instead of `json!` (one function per pull
  request, after Q11).
- **Done when:** no `parse_params` call left in the handlers; the catalog (`project-cli
  commands`) unchanged; CI green.
- **Done:** `Registry::add::<P>(id, title, params_help, Access, handler)` deserializes the
  parameters (error with the registered id) and the 61 handlers receive their struct;
  `register`, `parse_params` and `WRITES` are gone, the catalog is unchanged. Then one pull
  request per function: every parameter type is `pub` + `Serialize`, exported by
  `project-engine` (`BranchCreateParams`, ..., or the existing `CommitRequest`, `StashPush`,
  `LogQuery`, `CloneRequest`, `InitRequest`), and the UI builds its parameters with them
  (`crate::params`); only `{}` is left for the commands without parameters. Absent options and
  false flags are not serialized, so the JSON sent stayed the same, except `stash.push`, whose
  `StashPush` also appears in the `edit.undo`/`edit.redo` results and keeps its `null` fields.
  `MergeContinueParams`/`MergeAbortParams` also serve cherry-pick, revert and rebase.

### Q13 — Parameter schema for CLI and MCP
- [x] **Agent:** `git-engine`
- **Goal:** `params_help` is free text, and the description of the MCP `execute_command` tool is
  one long hand-written paragraph that repeats it and is already incomplete (rebase,
  cherry-pick, reset, `conflict.resolve`, `branch.rename` missing). JSON Schema of the parameters
  derived from the Q12 structs (`schemars`, new workspace dependency, checked with `cargo deny`)
  in `CommandSpec` and in `list_commands`/`project-cli commands --json`; the tool description
  keeps only the general rules (`writes`, `confirm`, `expected`, network commands) and points to
  `list_commands`. The commands refused over MCP (`repo.open`, `repo.clone`, `repo.init`) are a
  flag of `CommandSpec`, not ids compared in `crates/mcp`.
- **Done when:** MCP tests check the schema of a few commands and the refusal through the flag;
  CI green.
- **Done:** `Registry::add` requires `P: JsonSchema` (`schemars` 1, workspace dependency) and
  puts the schema in `CommandSpec::params` (draft 2020-12, subschemas inline, no `$ref`, field
  descriptions from the docs); `Oid`, `RepoPath`, `LineRef`, `RebaseStep` in `project-domain`
  have their schema too. `project-cli commands --json` prints the catalog with `params`; the text
  catalog (`params_help`) is unchanged. `CommandSpec::outside_repo` (set with
  `takes_paths_outside_repo()` on `repo.open`, `repo.clone`, `repo.init`) replaces the ids
  compared in `crates/mcp`; the `execute_command` description keeps the general rules and points
  to `list_commands`.

### Q14 — `GitBackend` split by area
- [x] **Agent:** `git-engine`
- **Goal:** `GitBackend` has ~65 methods. Sub-traits by area (reading, index, refs and journal,
  remotes, stash, operations in progress, maintenance) with `GitBackend` as their union
  (supertraits plus a blanket impl), so callers and `Session` do not change. No behavior change;
  the docs of each method move with it.
- **Done when:** both backends implement the sub-traits; `cargo doc` clean; CI green.
- **Done:** `GitRead` (12 methods), `GitIndex` (6), `GitRefs` (10), `GitRemotes` (16),
  `GitStash` (5), `GitOperations` (10), `GitMaintenance` (2), exported by `project-engine`;
  `GitBackend: GitRead + ... + GitMaintenance + Send` with a blanket impl, so `Session`, the
  commands and `Box<dyn GitBackend>` do not change. `MemoryBackend` and `CliBackend` implement the
  seven traits (same bodies, moved). Callers with a concrete backend (tests, the backends' own
  helper modules) import the traits of the methods they call instead of `GitBackend`; the doc
  links `GitBackend::<method>` point to the sub-trait.

## Phase Q2 — UI structure

### Q20 — One state for a dialog's running command
- [x] **Agent:** `egui-ui`
- **Goal:** the dialogs keep `sending: bool`, `ticket: Option<Ticket>` and `cancelling: bool`
  side by side (remotes, branch, stash, merge, conflict editor), which allows impossible states
  (sending without a ticket), and four modules define their own `Running { ticket, cancelling }`
  (clone, fetch, sync, optimize). One generic type (e.g. `Running<T = ()> { ticket, cancelling,
  extra: T }`) held as `Option<Running<..>>`.
- **Done when:** no `sending: bool` left; UI tests green; desktop screenshot on Xvfb of a
  dialog while it runs; CI green.
- **Done:** `crates/ui/src/running.rs`: `Running<T = ()> { ticket, cancelling, extra }` with
  `new`/`with` and `cancel()` (the ticket only the first time). Clone, optimize and the add-remote
  dialog hold `Option<Running>`, fetch `Option<Running<Fetching>>` (`remote`, `auto`), pull/push
  `Option<Running<Request>>`. The branch-name, stash, "Continue" and conflict-editor dialogs drop
  `sending: bool` for `running: Option<Running>`: `submit_*` only returns the parameters, and
  `name_started`/`push_started`/`continue_started`/`ConflictBox::started` record the ticket that
  `Tab::submit` now returns. The add-remote `fetching` field became `AddDialog::fetching()`
  (running and "Fetch right away", the fields being read-only while it runs).

### Q21 — `Tab` and `Action` by area
- [x] **Agent:** `egui-ui`
- **Goal:** `Action` is a ~300-line enum and `Tab::apply`, `Tab::ui` are over 200 lines each.
  `Action` becomes `Action::Branch(branch::Action)`, `Action::Stash(stash::Action)`, ... (the
  `apply_*` functions already exist per area), and `Tab::ui` delegates per panel.
- **Done when:** `apply` and `ui` under 100 lines; UI tests green; CI green.

### Q22 — Long functions and long argument lists
- [x] **Agent:** `git-engine`
- **Goal:** the longest handlers and backend functions (`remote_push` ~290 lines in
  `crates/engine/src/builtin/push.rs`, `remote_branch.rs` ~220, `pull.rs` ~190, `pull_upstream`
  ~180 in `crates/git/src/backend/sync.rs`, `backend/merge.rs`, `backend/stash.rs`) split into
  check, plan, execute and report steps; the `JournalEntry` constructors with 8 arguments take a
  struct, so the `#[allow(clippy::too_many_arguments)]` go away.
  Safety order (checks, backup, write) unchanged.
- **Done when:** no function over 150 lines in `crates/engine` and `crates/git`; `safety-reviewer`
  on the write paths; CI green.

### Q23 — Long UI functions
- [x] **Agent:** `egui-ui`
- **Goal:** the UI functions over 150 lines (review of 2026-11-01: `view/commits.rs` `show` ~185,
  `view/clone_view.rs` ~180, `view/refs.rs` ~180, `merge.rs` `receive` ~175, `remotes.rs` ~165,
  `branch.rs` ~160 twice, `view/stage_view.rs` ~155 twice, `sync.rs` ~155) split by section
  of the panel or by outcome; the `#[allow(clippy::too_many_arguments)]` of `stage_view.rs` and
  `view/refs.rs` go away with a context struct.
- **Done when:** no function over 150 lines in `crates/ui`; UI tests unchanged and green; CI green.

### Q24 — Shared test helpers
- [x] **Agent:** `git-engine`
- **Goal:** `same<T>` (typed result read back as the same JSON) was defined in 15 test modules
  of `crates/engine/src/builtin*`, and `oid(n)` in ~50 test modules of `crates/domain`,
  `crates/engine` and `crates/ui`. Now `crate::test_util::{oid, same}` in the engine,
  `crate::test_util::oid` in the UI (both `MemoryBackend::fake_oid`) and `tests::oid` (hex) in
  the domain. The `oid(n: u8)` (`[n; 20]`) and `oid(c: char)` variants stay in their tests: they
  give other ids, and the tests check them (short ids, order).
- **Done when:** one definition each per crate and id scheme; tests unchanged and green; CI green.

## Phase Q3 — Optional

### Q30 — More pedantic lints
- [ ] **Agent:** `git-engine`
- **Goal:** evaluate `clippy::cast_possible_truncation`, `match_same_arms`,
  `needless_pass_by_value` and `significant_drop_tightening` one at a time: enable the ones whose
  findings are real, with `#[expect(..., reason = "...")]` where a case is deliberate.
- **Done when:** each enabled lint passes with `-D warnings`; CI green.

### Q31 — Non-UTF-8 paths outside the repo
- [ ] **Agent:** `git-engine`
- **Goal:** paths inside the repo are bytes (`RepoPath`), but the repo folder is a `String`
  (`Session::open`, `RepoOpener`, `Platform::pick_folder`): a non-UTF-8 folder is dropped with a
  log line. Either carry it like `RepoPath` (string or byte array in JSON) or document the limit
  in `impl-docs/git/reading.md` and show a message instead of the log line.
- **Done when:** the decision is written down; test of the chosen behavior; CI green.

## Phase Q4 — UI performance

From the review of the GUI of 2026-11-18: work that grows with the size of the repo and runs on
the UI thread when a result arrives.

### Q40 — Graph lanes and colors a page at a time
- [x] **Agent:** `git-domain`, then `egui-ui`
- **Goal:** every page of `repo.log` (500 commits) recomputed the lanes and their colors of all
  the loaded commits on the UI thread: measured on a synthetic history of 100 000 commits (release
  build), 16 ms of `layout_graph` plus 45 ms of `lane_colors` per page, growing with the rows
  loaded. Both become incremental; the result does not change.
- **Done when:** a page costs its own commits; property test "pages give the same layout"; UI
  test "pages give the same colors"; CI green.
- **Done:** `project_domain::GraphBuilder` (`extend`, `extend_commits`, `layout`) keeps lanes and
  pending segments between pages, `layout_graph`/`layout_graph_nodes` feed it once.
  `CommitList` holds the builder; `graph_version` now grows only when the graph starts again
  from the first page. `graph::LaneColors` keeps the run of each row and segment (the color is
  read from the run, because a branch further down colors the rows above) and `extend`s by the
  new rows; the cache of `view/commits.rs` extends it in place and rebuilds it only when the
  graph version, the refs or the remotes change.

### Q41 — Large diffs off the UI thread
- [x] **Agent:** `egui-ui`
- **Goal:** `decode::<Diff>` and `DiffDoc::build` (rows, word diff of every changed pair, side by
  side rows) ran on the UI thread when `diff.commit` answered, with no limit on the number of
  lines. Measured (release build): 200 000 added lines 320 ms, 100 000 changed pairs 835 ms,
  10 000 pairs of long lines (~390 words) 5.6 s of frozen window, almost all of it the word diff.
- **Done when:** opening those diffs costs the UI thread less than a frame; UI tests; CI green.
- **Done:** `Executor::submit_prepared(command, params, prepare)` runs `prepare` on the result
  in the job, after the session is released; `take_prepared::<T>(ticket)` gives it to whoever
  handles the `Done`, and what nobody takes is dropped at the next `poll`. `Tab::request_diff`
  uses it to decode and build the `DiffDoc`; `DiffState::receive` takes the document. The word
  diff is no longer in the build: `DiffDoc::emphasis(f, h, l)` compares a pair of lines the first
  time one is drawn and keeps the answer (the pairs are found at build time, which is cheap).
  The line counts of the bar and of the file titles, recounted on every frame, are kept in the
  document. Measured after: `receive` under 1 µs for every case above (the job takes 35 ms for
  20 000 lines, 330 ms for 200 000); the first screen of long changed lines 25-30 ms once, then
  nothing. Still on the UI thread, at 200 000 lines only: 20-30 ms when a reload brings the same
  diff (comparing it and freeing the copy) and 12-26 ms to free the document on close.

### Q42 — Recent folders checked in a job, commit rows by id, Help windows in the app state
- [x] **Agent:** `egui-ui`
- **Goal:** the leftovers of the review that need no decision. `App::check_missing` called
  `Path::is_dir` on every recent repo once a second on the UI thread: a folder on a network drive
  that does not answer froze the window. `CommitList::position` scanned the loaded commits, and
  the frame asks it for HEAD (changes row, stage panel). "About" and "Keyboard shortcuts" kept
  their open flag in egui's temporary data, outside `App`, so `App::shortcuts` had to ask egui
  whether a modal layer was open.
- **Done when:** no disk access on the UI thread for the recent list; the Help windows open and
  close with an `Action`; UI tests; CI green.
- **Done:** the check runs with `Platform::spawn`, one at a time (`MissingCheck`: a flag cleared
  when the job ends or is dropped, the answer taken on a later frame). `CommitList` keeps the
  row of every loaded commit by id (`commit(id)`, `index_of`, `seek`, `anchor`).
  `tabs::Action::Help(Option<view::Help>)` and `App::help`: the window is drawn by `App::ui` as
  the settings and clone ones, and `top_modal_layer` is gone from `App::shortcuts`.
- **Checked and left as they are:** the windows of a failed fetch, pull or push are deliberately
  not modal (`failure_window`: "It does not block the rest of the window") and
  `RepoView::modal_open` agrees with that: whether they should be is a UX question. The colors
  written outside `theme.rs` are grouped in one place per area (the diff `Style`, the graph
  palette). The per-frame `recent_view()` and `labels()` allocate a few short strings.

### Q43 — Reads beside a write
- [x] **Agent:** `git-engine`, then `egui-ui`; `safety-reviewer` (reads now run during the writes)
- **Goal:** point 3 of the review. A job holds the session for the whole command, so during a
  fetch, a pull, a push or a commit with a slow hook every read waited: a clicked commit showed
  no details and no diff until the end, also during the automatic fetch.
- **Done when:** a read submitted during a write answers before the write ends; an answer read
  before a write ended never reaches the UI after that write's own; the reads take no lock in
  the repository; engine, UI and real-git tests; CI green.
- **Done:** `Session::reader()`: a second session on the same repo that only reads
  (`GitRead::reader`: the same repository for the `CliBackend`, a copy for the `MemoryBackend`);
  `repo_mut`, `journal`, `repo_and_journal` and the opens refuse on it. `CommandSpec::reader`
  marks the commands that may run there: the `Access::Read` ones except `repo.open` and
  `edit.history` (`needs_session`). The `Executor` runs them on the reader, takes a new reader
  after every command that writes or opens, and reads again an answer overtaken by a write
  (`executor::Reading`; architecture §4 "Session" and §6 "Reads beside a write"). CLI and MCP
  are unchanged. Tests: the catalog of the readers with parameters that reach every handler
  (`the_pure_reads_run_on_a_reader_and_the_rest_is_refused_there`), executor tests on threads,
  and `crates/git/tests/reader.rs` (every read leaves `.git/index` byte for byte as it was, free
  to lock it or with `index.lock` held; a reader writes nothing). Tried in the app: with a
  commit held 15 s by a `pre-commit` hook, the details and the diff of another commit load at
  once, and the new commit appears when the hook ends.
- **From the safety review** (no way to lose data found; reads and writes run side by side in
  120 rounds of add, commit, stash, checkout, gc, rebase with no failed write): a read answered
  during a write marks the tab for the automatic refresh, because a write that fails re-reads
  nothing (`Tab::receive`); a read on which git fails is asked again on the session, after the
  write (a file or `refs/stash` missing for a moment); a write that panics still counts as a
  change (`Changing`); an answer about a repo no longer open is not read again.
- **Left as they are:** the reads still run one at a time among themselves (a content search
  holds the others; it is cancelled when the query or the repo changes). A read overtaken by a
  write starts again from the beginning, so a long content search or blame takes longer if a
  stage or a fetch ends meanwhile. "Refresh" is off during a write. In a Git LFS repo a read can
  make `git-lfs` write `.git/config` and its hooks once per clone, as before this step, now
  possibly beside a command that writes the configuration (`config.lock`; not reproduced in 40
  tries).
- **Not verified:** Windows, where the app's reading and writing git processes now overlap.

## Phase Q5 — UI tests

### Q50 — Smoke tests of the drawing code
- [x] **Agent:** `egui-ui`
- **Goal:** the ~17 000 lines that draw (`view/`, `diff_view.rs`, `merge_view.rs`) ran in no
  test: the 378 tests of `crates/ui` drive the state and reach 1.5% of those lines. A view that
  panics, that is not drawn in its state or that reuses an id showed only by hand on the desktop
  app.
- **Done when:** every panel, dialog and menu is drawn by a test that checks what it shows; the
  measure is repeatable; CI green.
- **Done:** `crates/ui/src/view_tests.rs` (module `tabs::view_tests`, it needs the private state
  of `App`): `Screen` draws the whole `App` with `Context::run_ui` and
  `eframe::Frame::_new_kittest()` on a `MemoryBackend`, applies `Action`s as the end of a frame
  does, simulates the pointer (`click_on` a painted text) and returns the texts painted; every
  frame fails on egui's id-clash warning (a test checks that the warning is seen). 21 tests,
  0.2 s: welcome, the panels at several sizes down to 160x90, the custom window frame, stage
  area and its confirmations, diffs (unified, side by side, line selection, renamed file), the
  dialogs of branches, tags, remotes, stashes, merge, cherry-pick, reset, rebase, worktrees,
  clone, settings and credentials, a merge in conflict with its editor, rejected push and forced
  push, failed fetch and pull, blame, history, search, compare, two tabs, the light theme, the
  notices under the bar, and the context menus of commits, refs, remotes, files, submodules and
  of the menu bar. `make view-coverage` (llvm-tools of the toolchain, installed by
  `post-create.sh`): 81% of the functions and 75% of the lines of the drawing code, from 3.5%
  and 1.5%.
- **Not reached:** what a click does inside the dialogs (the handlers that push an `Action`:
  the state they lead to is tested by the area's own tests), a command in progress with its
  progress bar and "Stop" (the test platform answers at once), the stash-conflict window, the
  drag of rows in the rebase editor and of the window edges.
