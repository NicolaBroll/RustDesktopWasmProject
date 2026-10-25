# Changelog

All notable changes to BinariGit. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/) (0.x: not stable yet).

## [Unreleased]

### Changed

- **Title bar** in VS Code style: the title and a File menu (open a repository, recent
  repositories) are in the window's title bar, with the app's own minimize, maximize/restore and
  close buttons (Windows and Linux; on macOS the native buttons stay).
- **Toolbar** in GitKraken style: icon buttons with their name underneath (undo, redo, fetch,
  pull, push, stash, refresh) at the center, the branch on the left.

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

[Unreleased]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/releases/tag/v0.1.0
