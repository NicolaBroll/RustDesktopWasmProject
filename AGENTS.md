# Instructions for agents

Reference architecture: [impl-docs/ARCHITECTURE-rust.md](impl-docs/ARCHITECTURE-rust.md).

## Layers

| Layer | Crate | Folder |
| --- | --- | --- |
| L0 | `project-core` | `crates/core` — errors, parsing of `id=<json>` |
| L0 | `project-i18n` | `crates/i18n` — UI and engine error texts in Italian and English (Fluent), `tr!` macro |
| L2 | `project-domain` | `crates/domain` — git types and git output parsers |
| L5 | `project-engine` | `crates/engine` — `Session` (open repo and journal for undo/redo), `Registry`, `GitBackend`/`RepoOpener`, `MemoryBackend`, `Platform` trait |
| L6 | `project-ui` | `crates/ui` — egui `App`, no `cfg(target_arch)` |
| L6 | `project-mcp` | `crates/mcp` — MCP server on stdio |
| L6 | `project-git` | `crates/git` — `CliBackend`/`CliOpener`: runs `git`; only the L7 apps import it |
| L7 | `project`, `project-cli` | `apps/*` — startup and wiring only |

A crate imports only from **strictly lower** layers; `cargo xtask layers` checks this.
A new crate must be declared in `xtask/src/layers.rs`.

## Rules

- Every operation on the repository is a `Registry` command (`crates/engine/src/builtin.rs`).
  UI, CLI, MCP and tests invoke it with `Registry::run`; no modification logic in graphical callbacks.
- Command ids are stable: do not rename them.
- A command that modifies the repo goes in `WRITES` (`builtin.rs`): `CommandSpec::writes` flags it to
  UI, CLI and MCP. If it can lose data it asks for `"confirm": true` and saves a backup; if it rewrites
  the only copy of something without git asking for confirmation (stage/unstage) it saves a backup on its own
  and does not write if the backup fails (§1 and §7
  of the git architecture). Whoever rewrites a commit (`commit.create` with `amend`) always saves a
  backup ref of the rewritten commit and asks for `confirm` if that commit is already published.
- The rules of individual commands (stash, pull/push, merge...) are in the function's file in
  [impl-docs/git/](impl-docs/git/); the common ones and the safety checklist in
  [impl-docs/git/rules.md](impl-docs/git/rules.md).
- A command that moves refs (commit, and then branch, checkout, merge, reset...) records a
  `JournalEntry` in the `Session` journal with HEAD and the refs before and after, so `edit.undo` and
  `edit.redo` cover it (§7 of the git architecture).
- Platform I/O goes through the `Platform` trait; the implementations live in the L7 apps.
- Visible UI texts go through `tr!("<area>-<what>")` with the keys in
  `crates/i18n/locales/{it,en}/`: guide in [impl-docs/i18n.md](impl-docs/i18n.md).

## Branches

- Always work on `dev`, not on `master`: pull requests go to `dev`.
- **Every implementation starts from the latest version of `origin/dev`**: first `git fetch origin
  dev`, then the new branch with `git checkout -b <name> origin/dev` (never from `master`, from an
  outdated local `dev` or from another step branch). Whoever continues on an already created
  branch checks that it contains the latest `origin/dev` (`git merge-base --is-ancestor origin/dev
  HEAD`); if not, they stop and say so.
- Before starting a step, check the roadmap status on `origin/dev`.

## Git client (in development)

The project is becoming a graphical git client: architecture in
[impl-docs/ARCHITECTURE-git.md](impl-docs/ARCHITECTURE-git.md) (common rules) and in
[impl-docs/git/](impl-docs/git/) (one file per function: command contract, `CliBackend`, undo
and backup; a new command goes in the file of its function), steps in
[impl-docs/ROADMAP-git.md](impl-docs/ROADMAP-git.md). Each step names the agent to use
(`.claude/agents/`): `git-domain`, `git-engine`, `egui-ui`, `git-scenarios`; steps that
write to the user's repo also go through `safety-reviewer`.

## Before delivering

```sh
cargo xtask ci           # fmt, clippy -D warnings, test, layering
cargo xtask ci --quiet   # the same, with output only from the step that fails
```

## Trying without the UI

```sh
cargo run -p project-cli -- commands                      # catalog, `[writes]` = writes
cargo run -p project-cli -- --repo . run --cmd repo.status --cmd 'repo.log={"limit":5}'
cargo run -p project-cli -- mcp --root .                  # list_commands, execute_command, open_repo
```

Examples for every command: [impl-docs/git/cli.md](impl-docs/git/cli.md).

## Seeing the app

```sh
make desktop-shot                      # desktop app on Xvfb -> target/desktop.png (REPO=, WAIT=, SHOT=)
make demo-repo                         # demo repo -> target/demo-repo (merge, tag, stash, conflict, changes)
make desktop-start REPO=target/demo-repo   # app left running on Xvfb :99, with keyboard focus
DISPLAY=:99 xdotool mousemove 300 44 click 1   # click (screen 1280x800, window at 0,0)
DISPLAY=:99 xdotool type 'text'; DISPLAY=:99 xdotool key ctrl+Return
DISPLAY=:99 import -window root target/shot.png   # screenshot, then look at it
make desktop-stop
```

Tools (installed by `.devcontainer/post-create.sh`): `xvfb` + `xauth` (`xvfb-run`),
`x11-utils` (`xwininfo`, `xprop`), `imagemagick` (`import`), `xdotool`; Chrome for
`assets/render.sh`. VS Code's `WAYLAND_DISPLAY` must be unset, otherwise winit uses Wayland and the
screen stays black (the target does this on its own).
