# Clone and new repository

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the
`CliBackend` and undo and backup of this function. "§n" always refers to the main document.

Code: `crates/engine/src/git/clone.rs` (requests and shared checks), `crates/engine/src/builtin/clone.rs`
(commands), `crates/engine/src/git/memory/clone.rs` (`MemoryOpener`),
`crates/git/src/backend/clone.rs` (`CliOpener`).

## Commands

Both run **without an open repo**, like `repo.open`, and at the end open the new repo in the
`Session` (the previously open repo, and its journal, are replaced; if opening fails the
previously open repo stays). They go through the `RepoOpener` injected by the app
(`RepoOpener::clone_repo`, `RepoOpener::init_repo`), then `Session::open` on the same `path`.

### `repo.clone`

Parameters: `{"url", "path", "branch"?, "depth"?}`. Writes. Slow: progress and cancellation from
`Session::job` (`Registry::run_with`).

- `url`: anything `git clone` accepts (`https://`, `ssh://`, `user@host:path`, `file://`, a local
  path). A URL starting with `-` is a URL, never an option (`--end-of-options`). A relative local
  path is relative to the current directory of the process.
- `path`: the destination folder. It must **not exist** or be an **empty folder**; anything else
  (a non-empty folder, a file, a symbolic link) → `NotAllowed` `engine-clone-not-empty` /
  `engine-clone-not-folder`, nothing is written. Its parent folder must exist
  (`engine-clone-no-parent`): the command never creates more than one folder.
- `branch`: the branch (or tag) to check out, `git clone --branch`; one the remote does not have →
  `NotAllowed` `engine-clone-no-such-branch`, nothing left behind.
- `depth`: shallow clone, `--depth <n>`, `n > 0` (`0` → `InvalidParams`). For a plain local path
  git ignores it (it warns: "use file:// instead"); the warning is in `messages`.

Empty `url`/`path`, an invalid `branch` name, unknown fields → `InvalidParams`. Credentials are
never asked for (`GIT_TERMINAL_PROMPT=0`, no terminal): they come from the user's git configuration
(credential helper, `ssh-agent`); rejected or missing → `Auth { remote: <url> }`, unreachable →
`RemoteUnreachable { remote: <url> }`, as for `remote.fetch` ([sync.md](sync.md)).

Result: `{"path", "head", "commit_graph", "messages": [string]}`: the first three as in
`repo.open`; `messages`, git's non-progress lines (e.g. "warning: You appear to have cloned an
empty repository."). An empty remote clones fine: `head` is the unborn branch.

### `repo.init`

Parameters: `{"path", "branch"?}`. Writes.

`git init` in `path`, with the same destination rules as `repo.clone` (absent or empty folder,
existing parent). `branch`: the initial branch (`--initial-branch`, a valid new branch name, not
starting with `-`); without it git's `init.defaultBranch` (or git's own default). Result: `{"path",
"head", "commit_graph"}` as in `repo.open` (`head`: the unborn branch).

## Steps and guarantees (`CliOpener`)

1. Git version check (as `open_cli`), then `path` made absolute (`std::path::absolute`, against
   the current directory). `symlink_metadata`: a link (even to an empty folder) or a file →
   refused; a folder must be empty (`read_dir`).
2. An absent destination is created by the command with `create_dir` (atomic: it fails if
   something appeared in the meantime, so the command knows the folder is its own). git then always
   receives an existing, empty folder.
3. `git -C <cwd> -c protocol.ext.allow=never -c protocol.fd.allow=never clone --progress
   [--branch=<b>] [--depth=<n>] --end-of-options <url> <abs path>` with `Git::run_streaming`
   (stdin closed, `GIT_TERMINAL_PROMPT=0`, no controlling terminal, `LC_ALL=C`): progress lines
   go to the `JobContext`, the other lines into `messages` or the error. `ext::`/`fd::` URLs never
   run commands. Init: `git init --quiet [--initial-branch=<b>] -- <abs path>`.
4. On cancellation or error the destination goes back to how it was: a folder created by the
   command is removed whole (`remove_dir_all`); a pre-existing empty folder stays and only what is
   now inside it is removed (it was empty before, so it is all git's). git already does the same
   on failure and on SIGTERM; this covers SIGKILL after the cancel grace. Exception: if git's
   error says the destination "already exists and is not an empty directory" (someone wrote into
   it between the check and git), nothing is removed. A failed cleanup is appended to the error
   text (`engine-clone-cleanup-failed`), naming the folder to check by hand.
5. Success: `Session::open(path)`. If the clone cannot be opened (e.g. a sha256 repository) the
   error is the open error and the clone **stays** on disk: it is complete, not junk.

The `MemoryOpener` simulates the same contract: the remote is a repo registered at `url`
(otherwise `RemoteUnreachable`), `MemoryOpener::deny(url, message)` gives `Auth`,
`MemoryOpener::occupy(path)` marks a non-empty folder, a cancelled job gives `Cancelled` and
registers nothing. The clone has the remote's commits and tags, `origin/<b>` for each remote
branch, and the local branch (`branch` or the remote's HEAD branch) tracking `origin/<b>`; `depth`
is accepted but ignored.

## Undo and backup

Nothing to back up: the destination was absent or empty, so no existing file is touched. No
`JournalEntry`: the operation does not move refs of an open repo, and the new repo starts with an
empty journal (`edit.undo` right after says there is nothing to undo). Removing a clone or an
init is left to the user.

## MCP

`repo.clone` and `repo.init` take arbitrary paths and URLs (a local path outside the root is a
URL): through `execute_command` they are refused, like `repo.open` (the server is confined to its
root). The CLI runs them as any command: `project-cli run --cmd 'repo.clone={...}'` without
`--repo`, with progress on stderr.
