# Changelog

All notable changes to BinariGit. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/) (0.x: not stable yet).

## [Unreleased]

### Added

- **Search in the history** (engine; `repo.search` for CLI and MCP): the commits whose message,
  author (name or email) or id contain a text, taken literally and ignoring case, in the same order
  as the history, with the number of matches; optionally only those that touch a path.

- **Delete and publish tags** (engine; `tag.delete`, `remote.push_tag`, `remote.delete_tag` for CLI
  and MCP). Deleting a local tag always asks for confirmation, keeps it in a backup
  (`refs/project/backup/tag-delete-…`) and “Undo” puts back the same tag. Publishing never
  overwrites a tag already on the remote with another value. Deleting from the remote asks for
  confirmation with the value seen there, keeps it in a backup (`refs/project/backup/push-delete-…`)
  and is rejected if the tag moved in the meantime.

## [0.4.0] - 2026-10-25

Tags created from the app, a fetch that drops the branches deleted on the remote, and a tidier
title bar.

### Added

- **Create tags**: “New tag here…” in the menu of a commit, a local branch, a remote branch or a
  tag (`tag.create` for CLI and MCP). With a message the tag is annotated, without it is
  lightweight; “Undo” deletes it.
- **Fetch removes the branches deleted on the remote** (`remote.fetch` with `"prune": true`), so
  the column no longer shows them; commits that were only there stay in a backup
  (`refs/project/backup/prune-…`).
- **Help → About** with version and license, and the app icon in the title bar.

### Changed

- Tags are listed newest version first (`v0.10.0` before `v0.9.0`).
- No line between the title bar and the tabs.

### Fixed

- After moving the window by its title bar or resizing it from its edges, buttons light up under
  the mouse again without an extra click.
- A click on the arrow of a section (Local branches, Remotes, Tags, Stashes) opens and closes it,
  like a click on its name.
- The close button of the window was cut by the right edge: its × is centered again.

### Packages

- Linux: `BinariGit-0.4.0-x86_64.AppImage` (Ubuntu 22.04 or newer, glibc 2.35) and
  `BinariGit-0.4.0-x86_64.AppImage.zsync` for the updaters.
- Windows: `BinariGit-0.4.0-x86_64-windows.zip` (Windows 10 or newer, OpenGL 2).
- `SHA256SUMS.txt` to verify the downloads (`packaging/README.md`).

### Known limitations

- Tags are created locally only: there is no push of tags from the app yet.
- The Windows build is cross-compiled and has not been tested on a Windows PC yet; it is not
  signed, so SmartScreen warns on first start.
- No macOS build; no deb or MSI packages yet.

## [0.3.0] - 2026-10-25

Branches deleted on the remote from the app, and an AppImage that updates itself from GitHub.

### Added

- **Delete a branch from the remote**: “Delete from the remote…” in the menu of a remote branch
  (`remote.delete_branch` for CLI and MCP). It always asks for confirmation, saying which commits
  are only there and which local branches track it; the commit stays in a backup
  (`refs/project/backup/push-delete-…`) and, with `--force-with-lease`, if someone pushed to the
  branch after the last fetch the deletion is refused.
- **AppImage updates from GitHub**: the AppImage embeds its update information and the Release
  also publishes `BinariGit-<version>-x86_64.AppImage.zsync`, so AppImageUpdate, Gear Lever or
  AppImageLauncher update it to the latest Release, downloading only what changed.

### Packages

- Linux: `BinariGit-0.3.0-x86_64.AppImage` (Ubuntu 22.04 or newer, glibc 2.35) and
  `BinariGit-0.3.0-x86_64.AppImage.zsync` for the updaters.
- Windows: `BinariGit-0.3.0-x86_64-windows.zip` (Windows 10 or newer, OpenGL 2).
- `SHA256SUMS.txt` to verify the downloads (`packaging/README.md`).

### Known limitations

- AppImages of 0.1.0 and 0.2.0 carry no update information: install 0.3.0 by hand once, later
  releases arrive through the updater.
- The window has no system frame on Windows and Linux: minimize, maximize, drag and resize are
  the app's own and have not been tried yet with a real window manager.
- The Windows build is cross-compiled and has not been tested on a Windows PC yet; it is not
  signed, so SmartScreen warns on first start.
- No macOS build; no deb or MSI packages yet.

## [0.2.0] - 2026-10-25

A new look, VS Code and GitKraken style, and a left column that stays fluid with thousands of
branches and tags.

### Changed

- **Title bar** in VS Code style: the title and a File menu (open a repository, recent
  repositories) are in the window's title bar, with the app's own minimize, maximize/restore and
  close buttons (Windows and Linux; on macOS the native buttons stay).
- **Toolbar** in GitKraken style: icon buttons with their name underneath (undo, redo, fetch,
  pull, push, stash, refresh) at the center, the branch on the left.
- **Tabs** in VS Code style: flat tabs side by side, the active one joined to the toolbar below
  with a blue line on top instead of the blue fill; the "×" shows on the active tab and under the
  mouse, the middle button closes a tab too.
- **Left column**: a filter on top; branch names with `/` grouped in folders; each remote heads
  its branches; section headers with icons and "+" (add remote, new stash); the current branch
  has a check. Only the rows in view are drawn: thousands of branches or tags stay fluid.
- **Diff**: the changed words of a modified line are highlighted; a side by side view next to the
  unified one.
- **Theme**: File → Theme (as the system, light, dark); remote branch labels readable on light.
- Ahead/behind arrows (↑ ↓) no longer show as boxes; in a narrow window the commit list hides
  author and date before cutting the subject, and the panel headers no longer overlap their
  buttons; the stage area shows a file's button only under the mouse; the status bar shows the
  repo path and the changed files; the commit id in the details is short with a copy icon.

### Packages

- Linux: `BinariGit-0.2.0-x86_64.AppImage` (Ubuntu 22.04 or newer, glibc 2.35).
- Windows: `BinariGit-0.2.0-x86_64-windows.zip` (Windows 10 or newer, OpenGL 2).
- `SHA256SUMS.txt` to verify the downloads (`packaging/README.md`).

### Known limitations

- The window has no system frame on Windows and Linux: minimize, maximize, drag and resize are
  the app's own and have not been tried yet with a real window manager.
- The Windows build is cross-compiled and has not been tested on a Windows PC yet; it is not
  signed, so SmartScreen warns on first start.
- No macOS build; no deb or MSI packages yet.
- The side by side diff cuts long lines at the middle and stages whole hunks only (single lines
  in the unified view).
- Edits to worktree files appear when the window regains focus (the refs and the index are
  watched).

## [0.1.0] - 2026-10-24

First release: a graphical git client for the desktop, written in Rust with egui. It runs the
system `git`, which must be installed (Git for Windows on Windows).

### Features

- **History:** virtualized commit list with the graph of all branches, tags and stashes;
  commit details and diffs; blame and file history (with renames).
- **Changes:** stage and unstage by file, hunk or line; commit and amend; discard with a backup.
- **Undo/redo** of the operations that move refs (commit, branch, checkout, merge, rebase, pull, …).
- **Branches:** create, rename, delete, checkout; merge with a conflict editor; cherry-pick and
  revert; interactive rebase, also by dragging commits in the list.
- **Remotes:** fetch, pull and push (never forced without asking), add and remove remotes; long
  operations run in the background with progress and Stop.
- **Stash:** save, apply, pop and drop.
- **Several repositories** in tabs, a list of recent repositories, both restored at the next
  start; the open tabs refresh when the repository changes outside the app.
- **Huge repositories:** offers to write git's commit-graph when the history loads slowly
  (Linux kernel: first page from ~14 s to ~45 ms).
- Interface in English and Italian.
- CLI (`project-cli`) and an MCP server over stdio for agents, on the same commands as the UI.

### Packages

- Linux: `BinariGit-0.1.0-x86_64.AppImage` (Ubuntu 22.04 or newer, glibc 2.35).
- Windows: `BinariGit-0.1.0-x86_64-windows.zip` (Windows 10 or newer, OpenGL 2).
- `SHA256SUMS.txt` to verify the downloads (`packaging/README.md`).

### Known limitations

- The Windows build is cross-compiled and has not been tested on a Windows PC yet; it is not
  signed, so SmartScreen warns on first start.
- No macOS build; no deb or MSI packages yet.
- Edits to worktree files appear when the window regains focus (the refs and the index are
  watched).

[Unreleased]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/releases/tag/v0.1.0
