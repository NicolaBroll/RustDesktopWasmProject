# Commit and amend

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

## Commands

### `commit.create`

Parameters: `{"message", "amend"?, "confirm"?}`.

`git commit -F -` (see "CliBackend"). Checks shared by the backends (`check_commit_state`,
`CommitRequest::check_message`, `check_amend_published`), always before writing: empty or
whitespace-only message (or with NUL) → `InvalidParams`; conflicted files → `NotAllowed`; rebase,
cherry-pick, revert or `git am` in progress (also a multi-commit cherry-pick/revert stopped between one
commit and the next: `.git/sequencer`) → `NotAllowed` (a commit from here would conclude them or change them
in a hardly visible way: a cherry-pick would take the original author); nothing staged (index
equal to HEAD, `intent-to-add` files excluded) without `amend` and without a merge → `NothingToCommit`, never an
empty commit. Merge in progress: the commit is the merge commit (parents HEAD and `MERGE_HEAD`), even
without changes; to conclude a merge the command is `merge.continue` ([merge.md](merge.md)),
which uses the same commit but removes the `# Conflicts:` block from the `MERGE_MSG` message and goes into the
journal as a merge (with `commit.create` the entry is that of a commit). Bisect: no check. First commit of a branch and detached HEAD work (with
detached HEAD only HEAD moves). Missing identity: git's error, no default. `amend`:
same parents and author, tree from the index, new message (even just the message); refused
without commits, during a merge and if it would make the commit empty (git's error); a merge is rewritten
keeping its two parents. If the rewritten commit is reachable from a remote branch or a tag
(`for-each-ref --contains <HEAD> refs/heads/ refs/remotes/ refs/tags/`, without symbolic refs;
split in `amend_impact`) `"confirm": true` is required, otherwise `ConfirmRequired` listing the refs
(a tag would stay on the old commit); an unpublished commit is rewritten without confirmation (it is in the
reflog and in the backup). "Published" is seen only from tracking refs: a push by URL, without
`refs/remotes/...`, is not visible and the commit counts as unpublished. The other local branches and
tags that contain the rewritten commit end up in `also_on`: after the `amend` they stay on the old
history. An `amend` that produces the same commit (same tree, message, author and dates: e.g. two
identical `amend`s in the same second) is a success with no changes: `commit.id == previous`,
`backup` `null`. Before writing, with `amend`, a backup ref on the rewritten commit (below).
Result: `{"commit": Commit, "previous": oid | null, "branch": string | null, "backup": string |
null, "also_on": [string], "status": Status}`: the commit as in `repo.log`, HEAD before, the branch
moved with its full name (`refs/heads/main`; `null` with detached HEAD), the backup ref (only
`amend`), the other branches and tags left on the rewritten commit (only `amend`, otherwise empty) and the
new status. A problem **after** a successful commit (HEAD moved by a `post-commit` hook or by another
process, reflog that does not add up, status unreadable) is `PostCommit { commit, backup, reason
}`: the commit must not be repeated, `commit` is the commit created if known (`null` if HEAD is no longer
that one), `backup` the ref of the rewritten commit, which remains.

## CliBackend

`git -c i18n.commitEncoding=UTF-8 commit --no-edit -F - [--amend]` with `GIT_EDITOR=:`:
- hooks (`pre-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit`, `post-rewrite`) and
  signing (`commit.gpgSign`, `gpg.format`, `user.signingKey`) follow the configuration: never
  `--no-verify`, `--no-gpg-sign`, `--allow-empty` or `--allow-empty-message`;
- no editor (`--no-edit` and `GIT_EDITOR=:`, the value with which git does not start one): so
  `core.editor`, `commit.verbose`, `commit.template` (ignored anyway with `-F`) and
  `commit.status` cannot open an editor that would wait for a terminal;
- message cleanup is that of `commit.cleanup`; without configuration, with `-F` git
  uses `whitespace` (`#` lines stay: `#123 fixes` is a valid message). If the
  cleanup makes it empty (e.g. `strip` and only comments) git refuses: `InvalidParams`;
- the message comes from JSON and is UTF-8: `i18n.commitEncoding=UTF-8` prevents a different
  configuration from labeling it with another encoding;
- a hook that rejects or a signature that fails: `EngineError::Git` with git's stderr and stdout
  (git also sends hook output to stderr), no commit, index unchanged
  (except what the hook itself changed: a `pre-commit` that runs `git add` changes
  the index, and if it succeeds what gets committed is that, as with `git commit`). Locked
  index: `IndexLocked`;
- hooks and signing can be slow: the command stays synchronous (§6), stdout and stderr are
  read together and stdin is written from another thread, so a hook that prints a lot does not
  block. No timeout: a graphical gpg pinentry or an ssh key confirmation
  legitimately wait for the user; without a terminal or a graphical pinentry gpg fails immediately.
  If the app is started from a terminal, a text pinentry may use that terminal;
- a hook that leaves background processes with stdout or stderr open makes the
  command wait until those processes close them (git and the backend read to the end);
- an ssh signature with a passphrase-protected key (without an agent) and a hook that reads
  `/dev/tty` may wait on the controlling terminal, if there is one (CLI or MCP server started
  from a terminal); the desktop app usually has none. No timeout, by choice (see
  above);
- environment: besides the variables of §5, the inherited `GIT_AUTHOR_DATE` and `GIT_COMMITTER_DATE`
  are removed (an app started from a hook would give every commit the date of that moment); name and
  email (`GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_*`, `EMAIL`) stay, as for
  git from the terminal: they are the identity chosen by the user;
- after the commit the backend re-reads HEAD and checks that the last reflog entry of the updated
  ref (the branch, or HEAD if detached) goes from `previous` to the new commit
  (`<ref>@{1}` and `<ref>@{0}`): this also reveals another process that did an `amend`
  between reading `previous` and `git commit` (git would have rewritten that other commit, not
  the one saved in the backup). It then checks the parents (with `amend` the same as the rewritten
  commit). If something does not add up: `PostCommit` (the commit stays). Limit: without reflog
  (`core.logAllRefUpdates=false` and no log file for the ref) only the parents check
  remains. Inside `git commit` the ref is updated with the old value read by git,
  so a move during the hooks makes the commit fail.

## Backup of the rewritten commit (`amend`)

`commit.create` with `amend` replaces HEAD with a new commit: the old one stays only in the
reflog, which expires. Before running `git commit --amend` the backend creates
`refs/project/backup/amend-<seconds>-<nanoseconds>` on the rewritten commit (`update-ref <ref>
<id> ""`, only if it does not exist, as for the index) and returns it in `"backup"`; if the ref
cannot be created, no commit is made. If `git commit` then fails and HEAD is still the commit read before, the
ref is removed (`update-ref -d <ref> <id>`, which removes it only if it still points to that commit):
there is nothing to recover. A short window remains between re-reading HEAD and
`update-ref -d` in which another process could move HEAD: the backup would disappear, but the
commit would remain in the reflog. If instead HEAD changed (a hook or another process) the
backup stays and the error names it; `PostCommit` also names it after a successful commit. An
`amend` that produces the same commit removes the backup (it would point to the current commit). A new commit
saves no backup: the previous commit is its parent. The `MemoryBackend` records backups in
`MemoryBackend::commit_backups`.

For the journal (S22) `commit.create` already returns the before and after values of the moved ref:
`branch` (or `null` with detached HEAD), `previous` and `commit.id`.
