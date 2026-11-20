# Git identity (user configuration)

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5) and the journal (§7) are there; here are the command contract, the `CliBackend` and why
there is no journal entry or backup. "§n" always refers to the main document.

Code: `crates/engine/src/git/config.rs` (types, checks, `Identity::from_entries`,
`is_missing_identity`), `crates/engine/src/builtin/config.rs` (commands),
`crates/engine/src/git/memory/config.rs` (`MemoryBackend`, `MemoryOpener`),
`crates/git/src/backend/config.rs` (`CliBackend`, `CliOpener`).

Without `user.name` / `user.email` git refuses to create a commit ("Please tell me who you
are"). These two commands read and write those two keys, and nothing else of the configuration.

## Commands

Both run **with or without an open repo**. With one, the reads go through the open backend
(`GitConfig::identity`, `GitConfig::identity_file`) and so does the local write
(`GitConfig::set_local_identity`); the global file is always written by the `RepoOpener` injected
by the app (`RepoOpener::set_global_identity`), which also answers the reads when no repo is open
(`RepoOpener::identity`, `RepoOpener::global_identity_file`). `Session::identity`,
`Session::identity_file` and `Session::set_identity` choose between the two.

"No repo open" means **outside any repository**, whatever the current directory of the process
is (`project-cli` run from a checkout, the app started from a terminal inside a repo): the
opener's calls are detached from the repo around that directory, so its configuration is not
read (not even to fail on a broken one) and an `includeIf "gitdir:"` that matches it does not
apply.

### `config.identity`

Parameters: `{}`. Read (`CommandSpec::reader`: it runs on a `Session::reader` beside a write;
without an open repo there is no reader and it runs on the session).

Result:

```json
{"name":  {"value": "Ada", "scope": "local"},
 "email": {"value": "ada@example.com", "scope": "global"},
 "local":  {"name": "Ada", "email": null},
 "global": {"name": "A. L.", "email": "ada@example.com"},
 "env": ["GIT_AUTHOR_NAME"]}
```

- `name`, `email`: the value git uses and the scope it comes from: `local` (the repo's file),
  `global` (the user's file), `system`, `other` (the worktree file of
  `extensions.worktreeConfig`, the command line); both `null` when no file sets the key. A file
  pulled in with `include` / `includeIf` has the scope of the file that includes it.
- `local`, `global`: the value git reads in that scope **for the open repo**, included files
  followed (`null` = not set there). With a repo open, a conditional include of the global file
  that applies to it (`includeIf "gitdir:~/work/"`) counts: `global.email` is then the work
  email, not the one written in `~/.gitconfig`. Without an open repo `local` is `{"name": null,
  "email": null}` and the rest is what git reads outside any repository.
- A value is always the **last** one git reads: a key set twice in a file, or set again by an
  included file, counts once. A key set to an empty value (`email =`, or `[user] name` with no
  value) is `""`, not `null`: it hides the scopes below it and git then has no identity; removing
  it (`"email": ""` in `config.set_identity`) brings the lower scope back.
- `env`: the environment variables, as git will inherit them from the app, that take part in the
  identity whatever the configuration says, in this order: `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`,
  `GIT_COMMITTER_NAME`, `GIT_COMMITTER_EMAIL` when present (even empty: they win over the
  configuration), and `EMAIL` when it is not empty **and no `user.email` is configured** (git
  uses it only as a fallback). The configuration values are reported as they are, not replaced
  by the environment.
- Values are text (`from_utf8_lossy`): bytes that are not UTF-8 are shown with `U+FFFD`.
- Not covered: `author.name` / `author.email` / `committer.*` (they override `user.*` for one
  role) and `user.useConfigOnly`.

### `config.set_identity`

Parameters: `{"scope": "local" | "global", "name"?: string, "email"?: string}`. Writes
(`Access::Write`). No `confirm`.

- `scope`: `local` = the open repo's file (`git config --local`; without an open repo →
  `NotAllowed` `engine-config-local-no-repo`, nothing written); `global` = the user's global file
  (`git config --global`: `~/.gitconfig`, or `$XDG_CONFIG_HOME/git/config` when that exists and
  `~/.gitconfig` does not — git's own choice). Any other scope is `InvalidParams`: the system
  file and a file given by path are never written (no `--system`, no `--file`).
- `name`, `email`: a value sets the key; an **empty string removes it** from that file (not
  there: not an error); a key **not given is not touched**. A value is written as it is (no
  trimming; a leading `-` is a value, never an option).
- `InvalidParams` (nothing read or written): neither key given; NUL, LF or CR in a value; `<` or
  `>` in a value (git would drop them from the identity); a value that is not empty but made
  only of the characters git strips from the ends of a name (spaces, control characters and
  `.,:;"'\`) — for a name git would fail with "name consists only of disallowed characters", and
  the same rule is applied to the email; unknown fields.
- The state of the repo does not matter: it works during a merge, rebase, cherry-pick or revert
  (the identity is what a stopped operation needs to be concluded) and with a detached HEAD.
  `local` writes the repository's shared file: in a linked worktree it is the one of the main
  repository, read by all its worktrees.

**The command describes and writes one file**, the one of `scope`, and never a file it includes.
Before anything is written that file is read on its own (`GitConfig::identity_file`: its values
without `include`s, and for each key whether another file decides what git reads in the scope):

- **A key another file decides is refused before writing** (`NotAllowed`
  `engine-config-decided-elsewhere`, which names that file and the value it gives; nothing is
  written, not even the other key). "Decides" = the last value git reads in that scope, seen from
  the open repo, comes from a file that is not the written one: an `include`d file that sets the
  key after the file's own value, a conditional `includeIf` that applies to the open repo, or —
  for `global` when both exist — the XDG file for a key `~/.gitconfig` does not set. Writing
  would replace the file's own value (the personal email every other repository uses) without
  changing what git reads here. Exception: the value asked for is the one that file already
  gives — nothing to do, the key is skipped.
  - So a **global write with a repo open** is judged from that repo. In a repository under an
    `includeIf` that sets the email, the global email cannot be set (the error says which file
    to edit); the main file can be changed with no repo open, or from a repository the include
    does not apply to, and there `previous` is the main file's value.
- **Equal values write nothing**: a given value equal to the file's own last value (or `""` for
  a key the file does not set) is skipped; if all keys are skipped no git process writes and
  `changed` is `false`.

Result: the object of `config.identity`, re-read after the write, plus:

- `"previous": {"name", "email"}`: the values **of the written file itself** before the write
  (both keys, also the one not given; `null` = the file did not set it). Not the value of an
  included file: that one is never overwritten.
- `"previous_all": {"name"?: [..], "email"?: [..]}`: only when a key that was written had
  **several values** in the file (`name = Me`, `name = Second`): all of them, in file order.
  The write leaves one value (`--replace-all`) or none, and `previous` has only the last, the
  one git used. Absent otherwise.
- `"changed": bool`.

Errors after the parameters are accepted:

- A read before the write fails, or the first write fails (file locked by another git,
  read-only folder, `$HOME` not set for `global`): git's error (`Git { command: "config" }`);
  nothing was written.
- Every error raised **after something was written** is a `GitWithReason` whose text lists what
  each written key held before ("`user.name` was “Me”, “Second”, `user.email` was not set"), so
  it can be put back:
  - `engine-config-partial`: the name was written and the email then failed (git's text);
  - `engine-config-reread-failed`: the file could not be read again;
  - `engine-config-still-set` / `engine-config-not-read`: the file is read again and checked
    (point 2 of the checklist in [rules.md](rules.md): verify afterwards) and git does not read
    the requested value in the scope. After the check made before writing this needs another
    process changing the files in between.
- A value of a *higher* scope (a local value over the global one just written, the worktree
  file over the local one) is not an error: the result shows it in `name.scope` / `email.scope`.

## Why no journal, no backup, no `confirm`

- No ref, index or worktree changes, so there is no `JournalEntry` (§7 records what moves refs)
  and `edit.undo` does not touch the identity: undoing a commit made with a new identity leaves
  the identity as it is.
- What a write replaces is one or two short values of one file, and they are always returned:
  `previous` (and `previous_all` when a key had several values) in the result, the same list in
  every error raised after a write. Nothing else can be replaced: a key whose value git reads
  from another file is refused before writing. The UI shows the previous values in its status
  message. Putting them back is the same command with the old values.
- git has no compare-and-swap for the configuration: `previous` is read just before the write,
  and another process writing the same key in between would be overwritten without appearing
  in `previous`. git itself serialises the writers with `<file>.lock` and replaces the file with
  a rename, so a failed or interrupted write leaves the old file whole.
- Values are text: a previous value with bytes that are not UTF-8 is reported with `U+FFFD`.

## `CliBackend` / `CliOpener`

Only options present in git 2.30 (`--show-scope` is from 2.26). The fixed options and environment
of §5 apply (`-c core.quotePath=false`, `LC_ALL=C`, `GIT_TERMINAL_PROMPT=0`; inherited
`GIT_CONFIG_PARAMETERS` / `GIT_CONFIG_COUNT` removed, so a "command line" value cannot arrive
from the environment of the app).

- Where: the backend runs `git -C <workdir> config …`. The opener (no repo open, and every
  global write) runs `git config …` with **`GIT_DIR` set to something that is not a
  repository**: the null device on Unix, a name nobody creates in the temporary folder
  elsewhere (git accepts a `GIT_DIR` that does not exist if its parent does). git then looks
  for no repo around the current directory: no local or worktree file, no `includeIf "gitdir:"`,
  no failure on a repo with a broken configuration. Verified with git 2.30.9 and 2.56 (both
  forms); as a second guard the records with scope `local` or `worktree` are dropped.
- Read (`identity`): `config --show-scope --show-origin --includes -z --get-regexp
  --end-of-options '^user\.(name|email)$'`. Records are `<scope> NUL <origin> NUL <key> LF
  <value> NUL` (a key without a value has no LF), in the order git reads the files; exit code 1
  with no message = neither key is set. It takes no lock and writes nothing.
- The written file on its own (`identity_file`): `config --local|--global --no-includes
  --show-origin -z --get-regexp --end-of-options <keys>` (for `--global` git reads exactly the
  file it would write), plus the read above: a key is decided elsewhere when the last record of
  that scope has an origin that is not the file's own (same text, or the same file once
  resolved).
- Write: `config --local|--global --replace-all --end-of-options <key> <value>` (git keeps the
  value where the last one was), or `--unset-all --end-of-options <key>` to remove (exit code
  5, nothing to remove, is success). One call per key that changes, name first. The value is one
  argument after `--end-of-options`, never through a shell.
- `env`: read from the environment the git child will get (the variables added to the opener,
  then the inherited ones), not from a git call.

## MCP

`config.identity` and `config.set_identity` with `"scope": "local"` work as everywhere. The
global file is outside the server's root and read by every repository of the user, so it is
never written over MCP:

- the guarantee is below the parameters: the session's opener is wrapped (`Confined` in
  `crates/mcp/src/lib.rs`) and its `set_global_identity` always fails with `NotAllowed`
  `engine-config-global-confined`, whatever form the parameters had;
- `execute_command` also answers `"scope": "global"` with the same refusal up front (so the
  answer does not depend on what the file holds), and refuses `params` that are not a JSON
  object (or absent) for **every** command: serde would also fill a command's parameters from an
  array in field order, which a check on parameter names would not see.

## The missing identity, for the UI

A commit that fails because git has no name or email is an `EngineError::Git` with git's text
(the other commands that create commits may wrap it in other variants).
`EngineError::is_missing_identity()` recognises it in any variant, and
`is_missing_identity(text)` in a text that contains git's message: "Please tell me who you are",
"empty ident name", "unable to auto-detect email address" / "name", "no email / name was given
and auto-detection is disabled" (`user.useConfigOnly`), "name consists only of disallowed
characters" (`LC_ALL=C`, so the texts are always these). The `MemoryBackend` fails with the same
hint when neither scope gives both a name and an email (`MemoryBackend::set_identity` = the local
values; the global ones belong to the `MemoryOpener` and are shared by the repos it opens;
`MemoryOpener::set_included_identity` makes an "included file" decide a global key,
`set_identity_env` and `fail_identity_writes` cover the environment and a failing write).

## Tests

- `crates/engine/src/git/config.rs` (pure), `crates/engine/src/builtin/config.rs` (commands on
  the `MemoryBackend`: with and without a repo, a key decided by an included file, partial
  failure, the check after the write, reader), snapshots `config.identity*`,
  `config.set_identity*`.
- `crates/git/tests/config.rs`: real git, real files. **The global file is never the real one**:
  every test runs as a user whose `HOME` and `XDG_CONFIG_HOME` are in a temporary folder (they
  work with git 2.30, which has no `GIT_CONFIG_GLOBAL`; an inherited one is removed with
  `CliOpener::env_remove`), `GIT_CONFIG_NOSYSTEM=1`, the environment injected through the opener
  and never set on the test process; at the end the real global files are compared with their
  bytes at the start. Among the cases: `include` and `includeIf "gitdir:"` (refused before
  writing, files compared byte for byte), XDG and `~/.gitconfig` together, a key with several
  values, a locked file, no `HOME`.
- `crates/git/tests/config_cwd.rs`: no repo open while the process stands inside a repo — with
  its own identity, under an `includeIf "gitdir:"`, with a configuration git cannot parse (it
  changes the current directory, hence its own file). `backend/config.rs` runs both `GIT_DIR`
  forms.
- `apps/project-cli/tests/cli.rs` (no `--repo`, temporary `HOME`), `crates/mcp` (global refused
  in object and array form, at the opener; non-object `params` refused).
