# Remote management

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

## Commands

### `remote.list`

Parameters: `{}`.

Read-only. Result: `{"remotes": [{"name", "url": string | null, "push_urls": [string],
"fetch": [string], "mirror": bool, "branches": n, "default": bool, "push_default": bool,
"credentials": bool}], "default": string | null, "push_default": string | null}`, in name order
(`git remote`). `url`: the first `remote.<r>.url` as written (without `insteadOf`); `push_urls`: the
`pushurl`s, or all the `url`s if more than one; empty = the push goes to `url`. `branches`: the remote
branches `refs/remotes/<r>/...` (excluding those of a remote `<r>/...`, without symbolic ones); `default`: the
remote used by `remote.fetch` without parameters (`default_remote`; `null` if ambiguous); `push_default`:
the last `remote.pushDefault`. **Masked credentials** (`mask_url`, wherever a URL leaves the
engine: `remote.list`, `remote.add`, `remote.remove`, the journal entry in `edit.history`, error
messages about URLs): `scheme://user:password@host` → `user:***`; in `http(s)`/`ftp(s)`
a user without a password also becomes `***` (it is often a token); a user in `ssh://`/`git://` or
scp-style (`git@host:r`) stays; this applies inside a helper (`<x>::<url>`). `credentials`: the URL had some.
Ahead/behind of the branches relative to the upstream are not here: they are in `RefInfo::upstream`
(`repo.refs`, from `%(upstream:track)`), re-read in the `refs` of `remote.fetch`, `remote.pull` and
`remote.push`.

### `remote.add`

Parameters: `{"name", "url", "fetch"?}`.

Writes (`writes`), goes into the journal (§7). New name (`check_new_remote_name`: `check_remote_name` and
no leading `-`) → `InvalidParams`; name already configured (`git remote`), `remote.<name>.*` entries
in the repo file or remote branches `refs/remotes/<name>/...` left over from before (the undo would
delete them) → `NotAllowed`. **URL policy** (`check_remote_url`, before git): non-empty,
without control characters or leading or trailing spaces, does not start with `-` (`--upload-pack=...`,
`-oProxyCommand=...`), no `ext::`/`ext://`/`fd::`/`fd://` transports (they run commands or use
descriptors; also inside another helper and in any case: from here never, whatever
`protocol.allow` says), no user or host starting with `-` (`ssh://-x/r`, `ssh://-oX@h/r`,
`ssh://[-x]/r`, `u@-x:r`); other `<x>::` helpers and local paths are fine. Then `git remote
add --end-of-options <name> <url>` (URL and default refspec; a name git rejects →
`InvalidParams`, one that exists → `NotAllowed`). A URL with credentials is accepted with a warning in
`warnings` (git stores it in clear text). `"fetch": true`: then a fetch of the whole remote like
`remote.fetch` (slow, §6: progress and cancellation from the `JobContext`); a failed or
cancelled fetch does **not** undo the addition: the remote stays (and is in the journal) and the error goes in
`fetch_error` (`kind`: `auth`, `unreachable`, `cancelled`, `other`). Result: `{"remote", "url"
(masked), "credentials", "fetched", "fetch": {"updated", "messages", "warnings"} | null,
"fetch_error": {"kind", "message"} | null, "warnings", "remotes": [as remote.list], "refs",
...journal}`.

### `remote.remove`

Parameters: `{"name", "confirm"?}`.

Writes (`writes`), goes into the journal (§7). `name`: a configured remote (`check_remote_name`, leading
`-` allowed; otherwise `NotAllowed` with the list). Refused (`NotAllowed`, nothing changes): a
remote defined **even partly** outside the repo file
(`GitRemotes::remote_config_elsewhere`: global or system configuration, `include`,
`config.worktree`, `.git/remotes/`; the error lists key, scope and file), because a
part that git still sees would remain; the promisor remote of a partial clone (`remote.<name>.promisor` true or
`extensions.partialClone` = `<name>`: without it, missing objects could no longer be downloaded and
checkout, log and diff would fail); an operation in progress. If after the removal `git remote` still
lists the remote (`ensure_remote_gone`), everything is immediately put back as it was and the result is
`NotAllowed`; if the restore succeeds only partly (e.g. a remote branch recreated in the meantime: the
configuration is rewritten anyway, with a warning) or the list of remotes cannot be re-read, the
removal goes into the journal anyway, so `edit.undo` and the backups remain available (the error says
so). It **always** requires `"confirm": true`: without it, `ConfirmRequired` saying how many remote branches
disappear and which branches lose their upstream or `pushRemote`, and whether `remote.pushDefault` names it;
nothing changes. It removes (`remove_remote`, see "Undo and backup") the remote branches `refs/remotes/<name>/...` (not those of
a remote `<name>/...`; unlike `git remote remove`, refs written outside
`refs/remotes/<name>/` by a custom refspec stay), `refs/remotes/<name>/HEAD`, the
`remote.<name>` section, `branch.<b>.remote`/`merge` of the branches with that upstream, the
`branch.<b>.pushRemote` and `remote.pushDefault` that name it (all in the repo file); downloaded
tags stay, the real remote does not change. Result: `{"remote", "removed": RemoteSnapshot (masked
URLs), "upstreams_removed": [branch], "push_remotes_removed": [branch],
"push_default_removed": bool, "backups": [string], "warnings", "remotes", "refs", ...journal}`.

## CliBackend

**Remotes** (`remote.list`, `remote.add`, `remote.remove`, S27). Reading a remote
(`remote_snapshot`): a single `git config --local -z --get-regexp
'^(remote\.<name>\.[^.]+|remote\.pushdefault|branch\..+\.(remote|merge|pushremote))$'`
(repo file only, no `include` or global; a non-UTF-8 value → `NotAllowed`), `git
for-each-ref --format=%(refname)%00%(objectname)%00%(symref) refs/remotes/<name>/` and `git
symbolic-ref -q --end-of-options refs/remotes/<name>/HEAD` (a symbolic ref pointing to a ref
that does not exist does not appear in `for-each-ref`). `remote.list` reads with `config_values` (all
sources, like `git remote`). Entries outside the repo file (`remote_config_elsewhere`): `git
config -z --show-scope --show-origin --includes --get-regexp '^remote\.<name>\.[^.]+$'` minus
the origins of `git config -z --local --show-origin` (an included file has `local` scope but
another origin), plus the old-format files `remotes/<name>` and `branches/<name>` (`git rev-parse --git-path`), which git reads as remotes. The URL in the stderr of `git remote add` is replaced with the
masked one before making the error. Fetch and push run with `-c protocol.ext.allow=never -c
protocol.fd.allow=never`: an `ext::`/`fd::` remote configured by hand runs nothing.
Addition: `git remote add --end-of-options <name> <url>`, never
`-f`/`--mirror`/`-t`. **Removal without `git remote remove`**: the remote branches with the
`restore_refs` transaction (expected values, backup), then exactly the saved entries: `git
config --local --remove-section --end-of-options remote.<name>`, `git config --local
--fixed-value --unset-all --end-of-options <key> <value>` for upstream, `pushRemote` and
`remote.pushdefault` (code 5, nothing to remove, is fine), `git symbolic-ref --delete
--end-of-options refs/remotes/<name>/HEAD`. This way nothing disappears without being in the journal
(`git remote remove` would choose by itself the refs to remove from the refspecs, and would remove
`remote.pushDefault` from other files too). Restore: `git config --local --add
--end-of-options <key> <value>` for each entry and `git symbolic-ref -m <reflog>
refs/remotes/<name>/HEAD <ref>`.

## Undo and backup

`remote.add` and `remote.remove` go into the journal with the remote snapshotted
(`JournalEntry::remote`, a `RemoteSnapshot`: `remote.<r>.*` configuration of the repo file,
remote branches name → commit, where `refs/remotes/<r>/HEAD` points, the
`branch.<b>.remote`/`merge`/`pushRemote` entries that use it and `remote.pushDefault`; in the journal JSON
URLs are masked, in memory they are not). The entry's `refs` are the remote branches, for
display only. Two engine operations, the same for the backends:

- **`remove_remote`** (the command, undo of `remote.add`, redo of `remote.remove`): refused
  during an operation in progress; the remote must be **exactly** as in the snapshot
  (`RemoteSnapshot::differences`, configuration and upstreams as sets), otherwise
  `UndoConflict` (for the command, which has just read it, `NotAllowed`) and nothing changes. Then the
  remote branches are removed with `restore_refs` (one transaction with the expected values; HEAD is not
  touched) and the commits that would no longer be reachable from any other ref (local branches, tags,
  other remotes, backups) first go into a backup ref per tip commit,
  `refs/project/backup/remote-<s>-<ns>` (`RefRestore::backup_kind` of `remote.remove`; `undo-`
  and `redo-` for undo/redo): **choice**: no copy of all remote branches under a backup
  prefix, because their commits are almost always also in local branches and the name → commit
  is already in the journal entry; it is enough that gc does not remove those that were only there. Finally the
  configuration (`remove_remote_config`): section, symbolic `HEAD` and, only for
  `remote.remove`, saved upstream, `pushRemote` and `remote.pushDefault`; a problem here goes in
  `warnings` (the refs are already removed).
- **`restore_remote`** (undo of `remote.remove`, redo of `remote.add`): only if of the remote there
  is nothing (it is not among the configured remotes, no `remote.<r>.*` entries, no remote branches),
  otherwise `UndoConflict` (a remote recreated with the same name is not overwritten). First the
  remote branches (`restore_refs`, created only if they do not exist), then the configuration
  (`write_remote_config`). For `remote.remove` also the upstreams, but **only** for the branches that
  still exist and have not in the meantime another upstream (`GitRemotes::upstream`) or another
  `pushRemote`, and `remote.pushDefault` only if there is none now: the others stay as they are,
  with a warning in `warnings`. The journal entry is updated with what was actually
  put back, so the redo compares and removes only that (an upstream reset elsewhere stays).
  In the comparison a key without a value counts as `true` (it is rewritten that way).

The snapshot in the JSON has credentials masked in every value (also `remote.<r>.proxy`) and
for this reason it is not deserialized (`JournalEntry::remote` is `skip_deserializing`).

`remote.add` does not touch upstreams: its entry snapshots them as they were (usually none) and
the undo is refused if a branch has in the meantime taken the remote as upstream (e.g. with the first
`remote.push`), or if a fetch has changed the remote branches; the undo never removes downloaded tags
(they may have come from other remotes too). A remote added with remote branches already
present under `refs/remotes/<name>/` is refused beforehand, because the undo would delete them.
