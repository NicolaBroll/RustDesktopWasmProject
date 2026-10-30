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
- [ ] **Agent:** `git-engine`
- **Goal:** every handler starts with `parse_params("<id>", p)?` and repeats its id (37 `const
  ID`); `WRITES` repeats the ids next to the registration. Generic registration
  `add::<P: DeserializeOwned>(id, title, writes, handler)`: the `Registry` deserializes the
  parameters with the right id for the error and the handler receives `P`; `writes` is an
  argument of the registration and `WRITES` goes away. The parameter structs become `pub` and
  `Serialize`, so the UI builds parameters with them instead of `json!` (one function per pull
  request, after Q11).
- **Done when:** no `parse_params` call left in the handlers; the catalog (`project-cli
  commands`) unchanged; CI green.
- **Progress:** registry done: `Registry::add::<P>(id, title, params_help, Access, handler)`
  deserializes the parameters (error with the registered id) and the 61 handlers receive their
  struct; `register`, `parse_params` and `WRITES` are gone, the catalog is unchanged. The
  parameter structs are `pub(crate)` for now: still to do, one function per pull request, make
  them `pub` + `Serialize` (exported with a unique name, e.g. `BranchCreateParams`) and build
  the UI parameters with them (`crate::params`) instead of `json!`. Absent optional fields are
  not serialized (`skip_serializing_if`), so the JSON sent stays the same. Done: branch, tag, stash, sync, merge (with
  conflict; `MergeContinueParams`/`MergeAbortParams` also serve cherry-pick, revert and rebase), rebase, pick, reset, remotes.

### Q13 — Parameter schema for CLI and MCP
- [ ] **Agent:** `git-engine`
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

### Q14 — `GitBackend` split by area
- [ ] **Agent:** `git-engine`
- **Goal:** `GitBackend` has ~65 methods. Sub-traits by area (reading, index, refs and journal,
  remotes, stash, operations in progress, maintenance) with `GitBackend` as their union
  (supertraits plus a blanket impl), so callers and `Session` do not change. No behavior change;
  the docs of each method move with it.
- **Done when:** both backends implement the sub-traits; `cargo doc` clean; CI green.

## Phase Q2 — UI structure

### Q20 — One state for a dialog's running command
- [ ] **Agent:** `egui-ui`
- **Goal:** the dialogs keep `sending: bool`, `ticket: Option<Ticket>` and `cancelling: bool`
  side by side (remotes, branch, stash, merge, conflict editor), which allows impossible states
  (sending without a ticket), and four modules define their own `Running { ticket, cancelling }`
  (clone, fetch, sync, optimize). One generic type (e.g. `Running<T = ()> { ticket, cancelling,
  extra: T }`) held as `Option<Running<..>>`.
- **Done when:** no `sending: bool` left; UI tests green; desktop screenshot on Xvfb of a
  dialog while it runs; CI green.

### Q21 — `Tab` and `Action` by area
- [ ] **Agent:** `egui-ui`
- **Goal:** `Action` is a ~300-line enum and `Tab::apply`, `Tab::ui` are over 200 lines each.
  `Action` becomes `Action::Branch(branch::Action)`, `Action::Stash(stash::Action)`, ... (the
  `apply_*` functions already exist per area), and `Tab::ui` delegates per panel.
- **Done when:** `apply` and `ui` under 100 lines; UI tests green; CI green.

### Q22 — Long functions and long argument lists
- [ ] **Agent:** `git-engine`
- **Goal:** the longest handlers and backend functions (`remote_push` ~290 lines in
  `crates/engine/src/builtin/push.rs`, `remote_branch.rs` ~220, `pull.rs` ~190, `pull_upstream`
  ~180 in `crates/git/src/backend/sync.rs`, `backend/merge.rs`, `backend/stash.rs`) split into
  check, plan, execute and report steps; the `JournalEntry` constructors with 8 arguments take a
  struct, so the `#[allow(clippy::too_many_arguments)]` go away.
  Safety order (checks, backup, write) unchanged.
- **Done when:** no function over 150 lines in `crates/engine` and `crates/git`; `safety-reviewer`
  on the write paths; CI green.

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
