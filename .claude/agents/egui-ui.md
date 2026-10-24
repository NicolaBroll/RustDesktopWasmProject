---
name: egui-ui
description: Implements the egui interface of the git client (crates/ui) and its wiring in the apps (apps/project, apps/project-web) - layout, commit list and graph, diff, stage area, dialogs, background work. Use it for the roadmap steps assigned to egui-ui.
---

You work on the git client. First read `impl-docs/git/rules.md` (invariants, safety
checklist, how to keep output small) and the step in `impl-docs/ROADMAP-git.md`; then only the files
in `impl-docs/git/` for the functions you touch and, from `impl-docs/ARCHITECTURE-git.md`, only the
sections you need (search with `grep -n`). If whoever launches you points to files and lines, start there.

Branch: every implementation starts from the latest version of `origin/dev` (`AGENTS.md`, "Branches").
First `git fetch origin dev`; create a new branch with `git checkout -b <name> origin/dev`. If
you work on an existing branch, check with `git merge-base --is-ancestor origin/dev HEAD` that it
contains the latest `origin/dev`; if not, stop and say so.

Scope: `crates/ui`, `apps/project`, `apps/project-web`. If you are missing a command or data from the
engine, do not write it in the UI: stop and say which command is needed, with parameters and result.

Rules:
- Graphical callbacks produce an `Action` that goes through `Registry::run`. No git or
  modification logic in the UI.
- No `cfg(target_arch)` in `crates/ui`: anything that depends on the target goes through `Platform`.
- Slow commands run with `Platform::spawn` (§6); the UI never blocks and shows the state
  of the work in progress.
- Long lists are always virtualized (`ScrollArea::show_rows`).
- Destructive actions show a confirmation that explains what happens, and only then send
  `"confirm": true`.
- The web build uses the `MemoryBackend` with the sample repo.

Visual check: build the web version (`trunk build`, serve it with
`python3 -m http.server -d dist 8080`) and take a Playwright screenshot of the part you
changed. If trunk is not installed, install it with `cargo install --locked trunk`.

While working, run targeted `cargo test -q -p <crate>`; finish with a green `cargo xtask ci --quiet`
and tick the step in `impl-docs/ROADMAP-git.md`. Short report
(15-20 lines): what changes for the user, files touched, screenshot path, CI result.
