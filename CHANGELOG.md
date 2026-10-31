# Changelog

All notable changes to BinariGit. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/) (0.x: not stable yet).

## [Unreleased]

## [0.9.0] - 2026-10-31

Three everyday operations that were missing: discarding single lines or hunks, ignoring files
from the app, and merging a branch as a single commit (squash).

### Added

- **Discard lines and hunks**: in the diff of unstaged changes, “Discard lines… (N)” next to
  “Stage selected lines” and “Discard hunk…” on each hunk put the chosen lines back as they are in
  the index; the other changes of the file stay. Always confirmed; the file as it was is saved in a
  backup and “Undo” puts it back (new `worktree.discard_lines`). Line endings, `clean`/`smudge`
  filters and the executable bit are kept; a file whose bytes would not survive git's filters is
  refused.
- **Ignore files**: an “Ignore” submenu on untracked files adds “This file”, “All *.ext files” or
  a parent folder to `.gitignore` (shared, committed) or to `.git/info/exclude` (only for this
  clone). The status bar says how many files became ignored and warns about tracked files the
  pattern matches; “Undo” removes the lines (new `worktree.ignore`).
- **Squash merge**: “Merge into … with options…” in the branch and commit menus opens a dialog
  with Merge commit / Squash into one commit / Fast-forward only. A squash brings the branch in as
  one ordinary commit, with an editable message (default: `Squashed branch 'x'` and the list of
  commits); on conflicts it stays in progress with its banner, “Continue squash…” and “Abort
  squash…” (`merge.start` with `squash`, `Squash` operation in `repo.operation`).

### Changed

- The “… with a merge commit” entry of the branch and commit menus is replaced by “Merge into …
  with options…”; the quick “Merge into …” entry is unchanged.

### Packages

- Linux: `BinariGit-0.9.0-x86_64.AppImage` (Ubuntu 22.04 or newer, glibc 2.35) and
  `BinariGit-0.9.0-x86_64.AppImage.zsync` for the updaters.
- Windows: `BinariGit-0.9.0-x86_64.msi` and `BinariGit-0.9.0-x86_64-windows.zip` (Windows 10 or
  newer, OpenGL 2).
- `SHA256SUMS.txt` to verify the downloads (`packaging/README.md`).

### Known limitations

- The Windows MSI and zip are cross-compiled and have not been tested on a Windows PC yet; they
  are not signed, so SmartScreen warns on first start.
- The ssh passphrase and host-key prompts need OpenSSH 8.4 or newer.
- No macOS build.

## [0.8.0] - 2026-10-31

Opening the repository outside the app: its folder in the file manager and a terminal in it,
from the File menu; plus updated dependencies.

### Added

- **Show in file manager** and **Open terminal here** in the File menu: they open the folder of
  the repository shown in the system's file manager or in a terminal (on Linux `$TERMINAL`, then
  `x-terminal-emulator` and the common desktop terminals; Terminal on macOS; Windows Terminal,
  otherwise `cmd`, on Windows). A program that does not start shows its error in the status bar.

### Changed

- Dependencies updated (patch and minor versions).

### Packages

- Linux: `BinariGit-0.8.0-x86_64.AppImage` (Ubuntu 22.04 or newer, glibc 2.35) and
  `BinariGit-0.8.0-x86_64.AppImage.zsync` for the updaters.
- Windows: `BinariGit-0.8.0-x86_64.msi` and `BinariGit-0.8.0-x86_64-windows.zip` (Windows 10 or
  newer, OpenGL 2).
- `SHA256SUMS.txt` to verify the downloads (`packaging/README.md`).

### Known limitations

- The Windows MSI and zip are cross-compiled and have not been tested on a Windows PC yet; they
  are not signed, so SmartScreen warns on first start.
- The ssh passphrase and host-key prompts need OpenSSH 8.4 or newer.
- No macOS build.

## [0.7.0] - 2026-10-31

The fundamental operations that were missing: local branches that follow a remote branch,
credential prompts, comparing two revisions, deleting untracked files and checking out a commit;
plus a second review of the interface.

### Added

- **Branches that follow a remote branch**: “Checkout” on a remote branch (menu or double click)
  creates the local branch with its upstream and switches to it, or switches to the local branch
  that already follows it; the new-branch dialog started from a remote branch has a “Track …”
  checkbox. “Set upstream…” and “Unset upstream” in the menu of a local branch, both undoable
  (`branch.create` with `track`, new `branch.set_upstream`).
- **Credential prompts**: when git or ssh ask for a username, password, key passphrase or the
  confirmation of an unknown host key during fetch, pull, push or clone, a dialog asks for it
  (masked field for secrets; a host key is trusted only with “Trust and continue”). Credentials
  are saved only by the user's git credential helper; an askpass the user configured
  (`GIT_ASKPASS`, `core.askPass`, `SSH_ASKPASS`) is used as before. Automatic fetch never asks: it
  pauses with “Retry”. CLI and MCP do not ask.
- **Compare two revisions**: Ctrl+click (Cmd on macOS) a second commit shows the files changed
  between the two and their diff, with “Swap”; “Compare with <current branch>” in the branch menu
  shows what the branch brings since it diverged (`diff.range`).
- **Delete untracked files**: “Delete…” on an untracked file and “Delete all N untracked
  files…” in the “⋯” menu of the unstaged list, with a confirmation listing the files. Their exact
  content is kept first in a backup (`refs/project/backup/clean-…`) and “Undo” puts them back
  (`worktree.clean`).
- **Check out a commit**: “Check out this commit…” in the commit menu (detached HEAD), with
  “Detached at <id>” and “Create branch here…” in the top bar (`commit.checkout`). Commits that
  no branch or tag reaches are saved in `refs/project/backup/detached-…` before HEAD leaves them.
- Keyboard shortcuts listed under Help → Keyboard shortcuts; UI scale and relative dates in the
  settings; “Copy commit id” and “Copy path”.

### Changed

- ↓n / ↑n badges on Pull and Push and the time of the last fetch; lane colors follow the branch.
- Refusals because of local changes show a readable text with “Stash…”; failure windows are
  centered.
- The commit menu offers only the entries that apply; “Discard all…” moves into a “⋯” menu;
  Commit is the primary button.
- Esc with a menu open closes only the menu; closing the search gives back the previous
  selection.

### Fixed

- The stage panel works at 760x520 (also in Italian) and the commit box no longer moves; narrow
  rows cut the message before the ref labels; the details panel no longer clips.
- Contrast of the light theme.

### Packages

- Linux: `BinariGit-0.7.0-x86_64.AppImage` (Ubuntu 22.04 or newer, glibc 2.35) and
  `BinariGit-0.7.0-x86_64.AppImage.zsync` for the updaters.
- Windows: `BinariGit-0.7.0-x86_64.msi` and `BinariGit-0.7.0-x86_64-windows.zip` (Windows 10 or
  newer, OpenGL 2).
- `SHA256SUMS.txt` to verify the downloads (`packaging/README.md`).

### Known limitations

- The Windows MSI and zip are cross-compiled and have not been tested on a Windows PC yet; they
  are not signed, so SmartScreen warns on first start.
- The ssh passphrase and host-key prompts need OpenSSH 8.4 or newer.
- No macOS build.

## [0.6.0] - 2026-10-30

A Windows installer, the parameter schema for CLI and MCP, and a review of the interface: the
open diff survives the window losing focus or being resized, panels that follow the window,
readable small text.

### Added

- **Windows installer**: `BinariGit-<version>-x86_64.msi`, next to the zip. It installs for the
  current user only (no administrator rights) in `%LOCALAPPDATA%\Programs\BinariGit`, adds
  BinariGit to the Start menu and is removed from “Installed apps”; a newer MSI replaces the
  installed version. It is not signed yet and has not been tested on a Windows PC yet.
- **Parameter schema for CLI and MCP**: `project-cli commands --json` and the MCP `list_commands`
  give the JSON Schema of every command's parameters (`params`) and flag the commands not
  available over MCP (`outside_repo`); the description of `execute_command` keeps only the
  general rules and no longer misses commands.

### Changed

- One scale of text sizes across the app; the small text (status bar, hints, counters, sub-lines)
  grows from 9 to 11 points.
- In a narrow stage panel the section titles stay whole and their buttons move to the row below.
- The diff of a single file shows its path once, in the bar above.
- The settings shortcut reads “Ctrl+,” in the File menu; clearer Italian texts (“Togli tutto
  dallo stage”, “Scrivi il messaggio del commit”).

### Fixed

- The open diff no longer reloads when the window gets focus back or is resized: the scroll and
  the selected lines stay, and the status bar no longer flashes for the automatic refresh.
- The side panels get their width back when the window grows again after being made smaller; in a
  narrow window the center keeps at least 40% of the width.
- In a narrow window the diff bar, the “Stage hunk” button and the split-view hint no longer
  overlap other text, and the stage lists no longer cover the commit box.
- The commit box no longer moves when a dialog opens.

### Packages

- Linux: `BinariGit-0.6.0-x86_64.AppImage` (Ubuntu 22.04 or newer, glibc 2.35) and
  `BinariGit-0.6.0-x86_64.AppImage.zsync` for the updaters.
- Windows: `BinariGit-0.6.0-x86_64.msi` and `BinariGit-0.6.0-x86_64-windows.zip` (Windows 10 or
  newer, OpenGL 2).
- `SHA256SUMS.txt` to verify the downloads (`packaging/README.md`).

### Known limitations

- The Windows MSI and zip are cross-compiled and have not been tested on a Windows PC yet; they
  are not signed, so SmartScreen warns on first start.
- No macOS build.

## [0.5.0] - 2026-10-25

Repository management: clone and new repository, reset of the branch, tags deleted and published,
search in the history, a settings window with the language, and automatic fetch.

### Added

- **Clone and new repository**: “Clone…” and “New repository…” in the File menu and on the empty
  screen (`repo.clone`, `repo.init` for the CLI). The clone runs in the background with progress
  and “Cancel”, never over existing files; a cancelled clone leaves no folder behind. The new repo
  opens in a tab and goes in the recent ones.
- **Reset of the current branch**: “Reset … to here…” in the menu of a commit, with the three
  modes (soft, mixed, hard) and the list of the files a hard reset overwrites
  (`branch.reset` for CLI and MCP). Changes overwritten by a hard reset are kept first in a backup
  (`refs/project/backup/reset-…`); “Undo” puts the branch back.
- **Automatic fetch**: every open tab downloads from its remote in the background (fetch, never
  pull: local branches and files are not touched), every 5 minutes by default; Settings → Remotes
  sets 1, 5 or 15 minutes or turns it off. It waits while a dialog, a command or an operation
  (merge, rebase, ...) is in progress; a hidden tab catches up when shown. The graph updates
  keeping selection and scroll, with a notice like “2 new commits on origin/main”. Rejected
  credentials pause it with a notice and “Retry”; an unreachable remote shows an icon in the bar
  and waits longer between attempts (up to 30 minutes).
- **Settings window** (File → “Settings…”, Ctrl+,): a General section with the theme (as the
  system, light, dark) and the language (as the system, Italiano, English), applied at once
  without restarting. The settings are kept between launches; the theme menu and the
  unified/side by side toggle of the diff change the same settings.
- **Search in the history** (Ctrl+F above the commit list; `repo.search` for CLI and MCP): the
  commits whose message, author (name or email) or id contain a text, taken literally and ignoring
  case, are highlighted in the graph, with “n of total” and next/previous; Escape closes the
  search.
- **Delete and publish tags**: “Push to …”, “Delete…” and “Delete from …” in the menu of a tag
  (`tag.delete`, `remote.push_tag`, `remote.delete_tag` for CLI and MCP). Deleting a local tag
  always asks for confirmation, keeps it in a backup (`refs/project/backup/tag-delete-…`) and
  “Undo” puts back the same tag. Publishing never
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

[Unreleased]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.9.0...HEAD
[0.9.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/NicolaBroll/RustDesktopWasmProject/releases/tag/v0.1.0
