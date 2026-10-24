# `remote.push` errors (`push.rs`, `builtin/push.rs`, backend).
engine-push-non-fast-forward = the remote has commits that the local branch does not have: run `remote.pull` (or fetch and merge/rebase) and push again
engine-push-stale-lease = the branch on the remote moved compared to the local remote-tracking branch (force-with-lease): run `remote.fetch`, check the incoming commits and decide again
engine-push-remote-rejected = the remote rejected the update
engine-push-hook = a `pre-push` hook (or a local git check) stopped the push before it started
engine-push-other = git rejected the push
engine-push-no-tracking = (no local remote-tracking branch)
engine-push-lost-others = and { $n } more
engine-push-lost-and-others = { $commits }, and { $n } more
engine-push-moved = `{ $shown }` moved since it was seen (expected { $expected }, now { $now }): the force is no longer the one confirmed. The remote would lose { $n ->
    [one] 1 commit
   *[other] { $n } commits
} that `{ $branch }` does not have ({ $commits }); to confirm: "confirm": true, "expected": "{ $now }"
engine-push-moved-gone = `{ $shown }` moved since it was seen (expected { $expected }, now missing): the force is no longer the one confirmed. The remote would lose { $n ->
    [one] 1 commit
   *[other] { $n } commits
} that `{ $branch }` does not have ({ $commits })
engine-push-force-confirm = the force would overwrite `{ $remote }/{ $remote_branch }` ({ $shown } = { $theirs }): the remote would lose { $n ->
    [one] 1 commit
   *[other] { $n } commits
} that `{ $branch }` does not have ({ $commits }). `{ $shown }` would first be saved in `refs/project/backup/push-...`; with `--force-with-lease`, if the remote moved in the meantime the push is rejected; to confirm: "confirm": true, "expected": "{ $theirs }"
engine-push-upstream-other-name = the upstream of `{ $branch }` is `{ $remote }/{ $remote_branch }`, with a different name: for safety (like `push.default=simple`) nothing is pushed to another branch, nothing pushed. Change the upstream or rename the branch
engine-push-upstream-not-branch = the upstream of `{ $branch }` is not a branch (`{ $merge }`): nothing pushed
engine-push-mirror = remote `{ $remote }` is a mirror (`remote.{ $remote }.mirror=true`): git would push all its refs together, and here only one branch is pushed. Nothing pushed; use git from a terminal for the mirror
engine-push-bad-remote-config = `{ $key }` names `{ $remote }`, which is not a configured remote ({ $remotes }): nothing pushed; fix the configuration or give `remote`
engine-push-local-upstream = the upstream of `{ $branch }` is a local branch (`{ $merge }`): give `remote`, nothing pushed
engine-push-no-branch = there is no local branch `{ $branch }` with commits: nothing pushed
engine-push-detached = HEAD is detached: nothing pushed. Switch to a branch (`branch.checkout`) or give `branch`
engine-push-expected-hex = `expected` must be the full id ({ $len } hexadecimal digits) of the remote-tracking branch shown in the confirmation: `{ $value }`
engine-push-confirm-without-expected = with `"confirm": true`, `expected` is required: the full id of the remote-tracking branch shown in the confirmation (the confirmation applies only to that commit)
engine-push-confirm-without-force = `confirm` and `expected` are only needed with `"force": true`
engine-push-lost-commit = { $id } “{ $summary }”
