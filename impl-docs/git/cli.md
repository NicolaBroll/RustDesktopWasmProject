# Trying without the UI

`project-cli` examples for every command (the undo/redo journal only lives inside the same
invocation or MCP session). Full contracts are in the per-function files in this folder.

```sh
cargo run -p project-cli -- commands
# the same catalog as JSON, with the JSON Schema of each command's parameters ("params")
cargo run -p project-cli -- commands --search branch. --json
cargo run -p project-cli -- --repo . run --cmd repo.status --cmd 'repo.log={"limit":5}'
# graph of all branches (local and remote), tags and HEAD; without stashes and backups
cargo run -p project-cli -- --repo . run --cmd 'repo.log={"all":true,"limit":20}'
# search in the history: ids of the matching commits in repo.log order (highlight, not filter);
# literal text, case-insensitive; "in" limits the fields, "path" the commits touching it
cargo run -p project-cli -- --repo . run --cmd 'repo.search={"query":"fix","all":true,"limit":20}'
cargo run -p project-cli -- --repo . run --cmd 'repo.search={"query":"ada@","in":["author"],"path":"README.md"}'
# huge repo: does it have a commit-graph ("commit_graph" in the repo.open result)? write it
# (~19 s on the Linux kernel, then the first log page goes from ~16 s to ~40 ms)
cargo run -p project-cli -- --repo . run --cmd 'repo.open={"path":"."}' --cmd repo.optimize
cargo run -p project-cli -- --repo . run --cmd 'commit.show={"id":"HEAD"}'
cargo run -p project-cli -- --repo . run --cmd diff.worktree --cmd 'diff.commit={"id":"HEAD","path":"README.md"}'
# compare two revisions: directly, or what `dev` brings since it left `master` (with "base")
cargo run -p project-cli -- --repo . run --cmd 'diff.range={"from":"HEAD~5","to":"HEAD"}'
cargo run -p project-cli -- --repo . run --cmd 'diff.range={"from":"master","to":"dev","merge_base":true,"path":"README.md"}'
# blame of the worktree file (uncommitted lines: "commit": null) or at a commit; history of one
# file across renames ("follow":false: only under this name), paginated like repo.log
cargo run -p project-cli -- --repo . run --cmd 'file.blame={"path":"README.md"}' --cmd 'file.blame={"path":"README.md","rev":"HEAD~3"}'
cargo run -p project-cli -- --repo . run --cmd 'file.log={"path":"README.md","limit":5}' --cmd 'file.log={"path":"README.md","skip":5,"limit":5,"follow":false}'
# they write to the repo's index (marked `[writes]` in `commands`, `writes` in MCP)
cargo run -p project-cli -- --repo . run --cmd 'index.stage={"paths":["README.md"]}' --cmd 'index.unstage={"paths":["README.md"]}'
# lines: numbers and `digest` come from the file's diff, then
cargo run -p project-cli -- --repo . run --cmd 'index.stage_lines={"path":"README.md","lines":[{"new":3}],"digest":"<from diff.worktree>"}'
# commit of what is staged; amending an already published commit also needs "confirm":true
cargo run -p project-cli -- --repo . run --cmd 'commit.create={"message":"Title\n\nBody"}'
cargo run -p project-cli -- --repo . run --cmd 'commit.create={"message":"Fixed title","amend":true}'
# discards unstaged changes (worktree like the index): needs "confirm":true, saves a backup
cargo run -p project-cli -- --repo . run --cmd 'worktree.discard={"paths":["README.md"],"confirm":true}' --cmd edit.undo
# branch: create, switch, rename, delete (not merged: "confirm":true; backup in refs/project/backup/delete-...)
cargo run -p project-cli -- --repo . run --cmd 'branch.create={"name":"test"}' --cmd 'branch.checkout={"name":"test"}' --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'tag.create={"name":"v9","message":"Version 9"}' --cmd edit.undo
# delete a local tag: always "confirm":true; the value (tag object or commit) goes to refs/project/backup/tag-delete-..., edit.undo puts it back
cargo run -p project-cli -- --repo . run --cmd 'tag.delete={"name":"v9","confirm":true}' --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'branch.rename={"name":"test","new_name":"test2"}' --cmd 'branch.delete={"name":"test2"}'
# a local branch that follows a remote branch (start must be <remote>/<branch>), and the upstream of
# an existing branch set / removed (null); edit.undo puts the previous upstream back
cargo run -p project-cli -- --repo . run --cmd 'branch.create={"name":"feature","start":"origin/feature","track":true}' --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'branch.set_upstream={"name":"dev","upstream":"origin/dev"}' --cmd 'branch.set_upstream={"name":"dev","upstream":null}' --cmd edit.undo
# reset of the current branch: soft (ref only), mixed (also the index), hard (also the worktree);
# the preview lists the files a hard would overwrite; a hard with local changes needs "confirm":true
# (and "expected": the HEAD id from the preview), saves them in refs/project/backup/reset-...
cargo run -p project-cli -- --repo . run --cmd 'branch.reset_preview={"target":"HEAD~1","mode":"hard"}'
cargo run -p project-cli -- --repo . run --cmd 'branch.reset={"target":"HEAD~1","mode":"soft"}' --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'branch.reset={"target":"HEAD~1","mode":"hard","confirm":true}' --cmd edit.undo
# clone (no --repo needed: it opens the clone; destination absent or an empty folder, progress on
# stderr; an error removes the folder it created) and a new empty repository
cargo run -p project-cli -- run --cmd 'repo.clone={"url":"https://github.com/rust-lang/log.git","path":"target/log"}' --cmd 'repo.log={"limit":3}'
cargo run -p project-cli -- run --cmd 'repo.clone={"url":"file:///path/to/repo","path":"target/shallow","branch":"main","depth":1}'
cargo run -p project-cli -- run --cmd 'repo.init={"path":"target/new-repo","branch":"main"}' --cmd repo.status
# fetch from a configured remote (remote branches and tags only; progress on stderr if it is a terminal)
cargo run -p project-cli -- --repo . run --cmd remote.fetch --cmd 'remote.fetch={"remote":"origin","branch":"main"}'
cargo run -p project-cli -- --repo . run --cmd 'remote.fetch={"prune":true}'
# pull (fetch + fast-forward/merge/rebase according to pull.rebase/pull.ff, or "strategy"; undoable with edit.undo;
# merge with conflicts: stays in progress, "in_progress":true, then merge.continue or merge.abort)
cargo run -p project-cli -- --repo . run --cmd remote.pull --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'remote.pull={"branch":"main","strategy":"rebase"}'
# push of the active branch (remote: pushRemote/pushDefault/upstream/origin; first push: sets the upstream);
# force only with lease and, if the remote would lose commits, "confirm":true with "expected" (the id shown by the confirmation)
cargo run -p project-cli -- --repo . run --cmd remote.push --cmd 'remote.push={"force":true,"confirm":true,"expected":"<id from the confirmation>"}'
# delete a branch from the remote: first without confirm (the engine says what disappears and gives the id), then with "expected"
cargo run -p project-cli -- --repo . run --cmd 'remote.delete_branch={"remote":"origin","branch":"old-feature"}'
cargo run -p project-cli -- --repo . run --cmd 'remote.delete_branch={"remote":"origin","branch":"old-feature","confirm":true,"expected":"<id from the confirmation>"}'
# tags on the remote: publish one (never forced: a tag there with another value is rejected, "tag_exists");
# delete one: first without confirm (the engine reads the remote value), then with "expected"
cargo run -p project-cli -- --repo . run --cmd 'remote.push_tag={"remote":"origin","name":"v9"}'
cargo run -p project-cli -- --repo . run --cmd 'remote.delete_tag={"remote":"origin","name":"v9"}'
cargo run -p project-cli -- --repo . run --cmd 'remote.delete_tag={"remote":"origin","name":"v9","confirm":true,"expected":"<id from the confirmation>"}'
# remotes: list (credentials in URLs masked), add (with fetch), remove ("confirm":true) and undo
cargo run -p project-cli -- --repo . run --cmd remote.list
cargo run -p project-cli -- --repo . run --cmd 'remote.add={"name":"fork","url":"../fork.git","fetch":true}' --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'remote.remove={"name":"fork","confirm":true}' --cmd edit.undo
# stash: list, push (with untracked files), apply/pop (index and expected from the list), drop ("confirm":true with "expected", backup) and undo
cargo run -p project-cli -- --repo . run --cmd 'stash.push={"message":"work in progress","include_untracked":true}' --cmd stash.list --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'stash.pop={"index":0,"expected":"<id from stash.list>","restore_index":true}'
cargo run -p project-cli -- --repo . run --cmd 'stash.drop={"index":0,"confirm":true,"expected":"<id from the confirmation>"}' --cmd edit.undo
# graph that also includes the commits the stashes were created on
cargo run -p project-cli -- --repo . run --cmd 'repo.log={"all":true,"stashes":true,"limit":20}'
# merge of a branch into the active branch (fast-forward or merge commit; undoable with edit.undo)
cargo run -p project-cli -- --repo . run --cmd 'merge.start={"rev":"test"}' --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'merge.start={"rev":"origin/main","no_ff":true,"message":"Merge main"}'
# with conflicts the merge stays in progress: banner (kind, conflicted files, incoming commits), then
# resolve the files, stage them and finish; or abort ("confirm":true, backup)
cargo run -p project-cli -- --repo . run --cmd repo.operation
cargo run -p project-cli -- --repo . run --cmd 'index.stage={"paths":["a.txt"]}' --cmd merge.continue
cargo run -p project-cli -- --repo . run --cmd 'merge.abort={"confirm":true}'
# cherry-pick and revert of a commit (undoable with edit.undo; a merge commit needs "mainline");
# with conflicts they stay in progress: resolve, stage, then *.continue or *.abort ("confirm":true, backup)
cargo run -p project-cli -- --repo . run --cmd 'commit.cherry_pick={"rev":"test"}' --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'commit.revert={"rev":"HEAD"}' --cmd 'commit.revert={"rev":"<merge id>","mainline":1}'
cargo run -p project-cli -- --repo . run --cmd 'index.stage={"paths":["a.txt"]}' --cmd cherry_pick.continue
cargo run -p project-cli -- --repo . run --cmd 'revert.continue={"message":"Revert test"}'
cargo run -p project-cli -- --repo . run --cmd 'cherry_pick.abort={"confirm":true}' --cmd 'revert.abort={"confirm":true}'
# interactive rebase: the default plan of base..HEAD (pick of each commit, oldest first), then the
# plan with reordered, reworded, squashed, fixed up or dropped commits (undoable with edit.undo;
# "confirm":true to rewrite published commits); a stop stays in progress: resolve, stage, continue
cargo run -p project-cli -- --repo . run --cmd 'rebase.plan={"base":"HEAD~3"}'
cargo run -p project-cli -- --repo . run --cmd 'rebase.start={"base":"HEAD~3","steps":[{"action":"pick","commit":"<id 3>"},{"action":"reword","commit":"<id 2>","message":"New message"},{"action":"fixup","commit":"<id 1>"}]}' --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'index.stage={"paths":["a.txt"]}' --cmd rebase.continue
cargo run -p project-cli -- --repo . run --cmd 'rebase.abort={"confirm":true}'
# conflict editor: versions of a conflicted file (base, ours, theirs, worktree; text or
# binary), then the resolution with the `digest` read (text in the worktree + stage, worktree
# backup in refs/project/backup/conflict-...); "delete":true instead of "content" deletes it
cargo run -p project-cli -- --repo . run --cmd 'conflict.show={"path":"a.txt"}'
cargo run -p project-cli -- --repo . run --cmd 'conflict.resolve={"path":"a.txt","digest":"<from conflict.show>","content":"merged\n"}' --cmd repo.operation
cargo run -p project-cli -- --repo . run --cmd 'conflict.resolve={"path":"a.txt","digest":"<from conflict.show>","delete":true}'
# operation journal: only lives inside the same invocation (or MCP session)
cargo run -p project-cli -- --repo . run --cmd 'commit.create={"message":"test"}' --cmd edit.undo --cmd edit.history
echo 'repo.refs' | cargo run -p project-cli -- --repo . eval
cargo run -p project-cli -- mcp --root .   # tools: list_commands, execute_command, open_repo
```
