# Reads: repo, log and diff

Part of [ARCHITECTURE-git.md](../ARCHITECTURE-git.md): principles (§1), rules for every call to
git (§5), slow commands (§6) and the journal (§7) are there; here are the command contract, the `CliBackend` and
undo and backup of this function. "§n" always refers to the main document.

## Commands

### `repo.open`

Parameters: `{"path"}`.

Uses the injected `RepoOpener`.

### `repo.status`

Parameters: `{}`.

### `repo.refs`.

Parameters: `{}`.

### `repo.log`.

Parameters: `{"skip"?, "limit"?, "rev"?, "all"?, "stashes"?}`.

Paginated (`limit` 200 if missing, result `{"commits", "has_more"}`). `git log --date-order` (children
before parents, then the most recent by committer date; also applies with several starting points,
and the graph lanes §3 handle several tips and several roots). From HEAD, or from `rev` (ref, `HEAD` or
id, after `--end-of-options`); a HEAD without commits gives an empty log. `"all": true`: from all local
branches, remote branches, tags and HEAD (`--branches --remotes --tags` and the HEAD id, so a detached
HEAD appears; a HEAD without commits is skipped): it shows the commits of unmerged branches and those
just downloaded on `origin/*`. `refs/stash`, backups in `refs/project/backup/` and
any other ref outside `refs/heads/`, `refs/remotes/`, `refs/tags/` (e.g. notes) are left out; a tag on a
tree or a blob is ignored. `"stashes": true` (only with `all`, otherwise `InvalidParams`): it also starts
from the stash bases (`StashEntry::base`, the first parent), so the commit the UI
attaches a stash to is always in the graph, even if no branch contains it any more; the stash commits
(and the index and untracked ones that have them as parents) are left out, the list is
`stash.list`. `rev` and `all` together → `InvalidParams`. `skip`/`limit` paginate in the same order,
so the pages are contiguous.

### `commit.show`

Parameters: `{"id"}`.

Metadata and files changed relative to the first parent.

### `diff.worktree` / `diff.index` / `diff.commit`

Parameters: `{"path"?}` / `{"path"?}` / `{"id", "path"?}`.

worktree relative to the index (without untracked files), index relative to HEAD (empty tree if there are
no commits), commit relative to the first parent (empty tree for a root commit, like
`commit.show`). Result: the `Diff` (`{"files": [...]}`). `path` (string or byte array, like
`RepoPath`) limits to the file with that exact path, old or new: a rename stays a rename (the
backend first looks for the other path with `--diff-filter=R`) and paths go to git after `--` with
`--literal-pathspecs`. No size limit
