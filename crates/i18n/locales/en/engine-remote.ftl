engine-remote-push-default-changed = `remote.pushDefault` changed
engine-remote-upstreams-changed = the upstream or pushRemote of the branches that use `{ $name }` changed
engine-remote-ref-new = `{ $name }`: should not have existed, now it is { $now }
engine-remote-ref-gone = `{ $name }`: should have been { $expected }, now it is missing
engine-remote-config-changed = the `remote.{ $name }.*` configuration changed
engine-remote-head-none = nothing
engine-remote-head-changed = `refs/remotes/{ $name }/HEAD`: should have pointed to { $expected }, now to { $now }
engine-remote-changed = remote `{ $name }` changed while `{ $command }` was reading it ({ $refs }): nothing changed, try again
engine-remote-exists-again = remote `{ $name }` exists again (configuration or remote-tracking branches)
engine-remote-ref-exists = `{ $name }` already exists
engine-remote-put-back = put back as it was
engine-remote-put-back-partly = put back only in part ({ $warnings }): the removal is in the journal (`edit.undo` undoes it) and the commits of the remote-tracking branches are in the backups
engine-remote-not-put-back = NOT put back ({ $error }): the removal is in the journal (`edit.undo` undoes it) and the commits of the remote-tracking branches are in the backups
engine-remote-url-empty = the remote URL is empty
engine-remote-url-control = the URL contains control characters
engine-remote-url-spaces = the URL starts or ends with spaces
engine-remote-url-dash = a URL cannot start with `-` (it would look like an option)
engine-remote-url-transport = the `{ $transport }` transport runs commands or uses open descriptors: not allowed
engine-remote-url-authority-dash = the user or host of the URL cannot start with `-`
engine-remote-name-empty = the remote name is empty
engine-remote-name-control = the remote name contains control characters
engine-remote-name-invalid = not a valid remote name
engine-remote-none-configured = the repository has no remotes configured
engine-remote-configured = configured remotes: { $names }
engine-remote-still-listed = after the removal git still sees remote `{ $name }` (it is also defined outside the repo's file): { $how }. Remove it from where it is defined
engine-remote-removed-unreadable = remote `{ $name }` was removed, but the list of remotes cannot be read again ({ $error }): the removal is in the journal (`edit.undo` undoes it) and the commits of the remote-tracking branches are in the backups
engine-remote-name-dash = a remote name cannot start with `-`: `{ $name }`
engine-remote-unknown = there is no remote `{ $remote }` ({ $known }); nothing fetched
engine-remote-none = `{ $id }`: the repository has no remotes configured
engine-remote-which = `{ $id }`: the current branch has no upstream and there is no `origin` remote: give `remote` among { $names }
engine-remote-no-branch = remote `{ $remote }` has no branch `{ $branch }`, nothing fetched
engine-remote-missing = there is no remote `{ $remote }`
engine-remote-exists = a remote `{ $name }` already exists, nothing changed
engine-remote-remove-config = the `remote.{ $name }.*` configuration and { $n ->
    [one] 1 remote-tracking branch
   *[other] { $n } remote-tracking branches
} (`refs/remotes/{ $name }/...`)
engine-remote-remove-upstreams = the upstream of branches { $branches } (`branch.<b>.remote`/`merge`)
engine-remote-remove-push-remote = the `pushRemote` of branches { $branches }
engine-remote-remove-push-default = `remote.pushDefault`
engine-remote-remove-confirm = removing remote `{ $name }` would remove { $summary }. The actual remote does not change; the commits that are only in the remote-tracking branches are first saved in `refs/project/backup/remote-...`, and `edit.undo` puts everything back
engine-remote-remove-promisor = `{ $remote }` is the promisor remote of a partial clone (`remote.{ $name }.promisor` or `extensions.partialClone`): the objects missing locally are fetched from there, and without the remote the repo would no longer find them (checkout, log and diff would fail). Nothing changed
engine-remote-remove-partly-elsewhere = remote `{ $name }` is also defined outside the repo's file: { $elsewhere }. Removing it from here would leave part of it (git would still see it): nothing changed, remove those entries first
engine-remote-remove-elsewhere = remote `{ $name }` is configured outside the repo's file (global configuration, `include` or `.git/remotes/`): nothing changed, remove it from where it is defined
engine-remote-remove-unknown = there is no remote `{ $name }` ({ $known }); nothing changed
engine-remote-add-refs-exist = there are already remote-tracking branches `refs/remotes/{ $name }/...` ({ $n }) left over from a remote removed by hand: nothing changed (choose another name)
engine-remote-add-config-exists = the repo configuration already has `remote.{ $name }.*` entries: nothing changed (choose another name or remove them)
engine-remote-add-exists = a remote `{ $name }` already exists, nothing changed
engine-remote-no-such-branch = remote `{ $remote }` does not have the requested branch, nothing fetched: { $message }
engine-remote-git-rejects-name = git does not accept the remote name `{ $name }`: { $stderr }
engine-remote-config-not-utf8 = the value of `{ $key }` is not UTF-8: it cannot be saved for undo, nothing changed (fix it with `git config`)
engine-remote-ref-not-utf8 = remote `{ $name }` has a remote-tracking branch with a non-UTF-8 name: it cannot be saved for undo, nothing changed
engine-remote-head-not-removed = `{ $head_ref }` (to `{ $target }`) was not removed: { $error }
engine-remote-config-not-removed = the `remote.{ $name }.*` configuration was not removed: { $error }
engine-remote-key-not-removed = `{ $key }` was not removed: { $error }
engine-remote-key-not-written = `{ $key }` was not rewritten: { $error }
engine-remote-head-not-put-back = `{ $head_ref }` was not put back (to `{ $target }`): { $error }
engine-remote-refs-not-put-back = the remote branches of `{ $name }` were not put back ({ $error }): the commits are in the backups and in the journal entry
engine-remote-upstreams-not-put-back = upstream and pushRemote not put back: { $error }
engine-remote-upstream-taken = branch `{ $branch }` already has a new { $what ->
    [push-remote] pushRemote
   *[other] upstream
}: the { $what ->
    [push-remote] pushRemote
   *[other] upstream
} to `{ $name }` was not put back
engine-remote-upstream-branch-gone = branch `{ $branch }` no longer exists: the { $what ->
    [push-remote] pushRemote
   *[other] upstream
} to `{ $name }` was not put back
engine-remote-push-default-taken = `remote.pushDefault` already has another value: it was not set back to `{ $name }`
engine-remote-memory-no-merge = upstream of `{ $branch }` without `merge`: not rewritten in the in-memory repo
engine-remote-fetch-unexpected-ref = the fetch changed `{ $name }` (from { $before } to { $after }) because of a refspec configured in `remote.<name>.fetch`: the previous value is in the reflog of the ref
engine-remote-url-credentials = the URL contains credentials: git saved them in clear text in the repo configuration (`remote.{ $name }.url`); a credential helper is better
# `remote.delete_branch` (`builtin/remote_branch.rs`, backend).
engine-remote-branch-confirm = deletes `{ $branch }` from remote `{ $remote }`, for everyone who uses it (`{ $tracking }` = { $theirs }){ $only }{ $upstreams }. The commit is first saved in `refs/project/backup/push-delete-...`; with `--force-with-lease`, if the branch moved on the remote in the meantime the deletion is rejected; to confirm: "confirm": true, "expected": "{ $theirs }"
engine-remote-branch-moved = `{ $tracking }` moved since it was seen (expected { $expected }, now { $now }): the deletion is no longer the one confirmed{ $only }{ $upstreams }; to confirm: "confirm": true, "expected": "{ $now }"
engine-remote-branch-only-none = ; its commits are also in other branches or tags here
engine-remote-branch-only = ; { $n ->
    [one] 1 commit is
   *[other] { $n } commits are
} in no other branch or tag here ({ $commits }): only the backup will keep { $n ->
    [one] it
   *[other] them
}
engine-remote-branch-upstream-of = ; it is the upstream of { $n ->
    [one] the local branch
   *[other] the local branches
} { $branches } (the upstream stays configured, shown as gone)
engine-remote-branch-mirror = remote `{ $remote }` is a mirror (`remote.{ $remote }.mirror=true`): nothing deleted; use git from a terminal for the mirror
engine-remote-branch-no-tracking = no fetch refspec of remote `{ $remote }` maps `refs/heads/{ $branch }` to a local remote-tracking branch: nothing deleted (the commit to confirm and back up would not be known)
engine-remote-branch-unknown = `{ $tracking }` does not exist here: run `remote.fetch` of `{ $remote }` and try again; nothing deleted
engine-remote-branch-incomplete = `{ $branch }` was deleted from only some of the push URLs of `{ $remote }` (or git failed afterwards): it may still be on the remote, see git's messages. The commit is in `{ $backup }`
engine-remote-fetch-prune-branch = `prune` removes the branches gone from the remote: it cannot be combined with `branch`
# `remote.push_tag`, `remote.delete_tag` (`builtin/remote_tag.rs`, backend).
engine-remote-tag-mirror = remote `{ $remote }` is a mirror (`remote.{ $remote }.mirror=true`): nothing published or deleted; use git from a terminal for the mirror
engine-remote-tag-confirm-without-expected = with `"confirm": true`, `expected` is required: the full value of the tag on the remote shown in the confirmation (the confirmation applies only to that value)
engine-remote-tag-expected-hex = `expected` must be the full id ({ $len } hexadecimal digits) of the tag on the remote shown in the confirmation: `{ $value }`
engine-remote-tag-unknown = remote `{ $remote }` has no tag `{ $name }`: nothing deleted
engine-remote-tag-local-same = ; the local tag has the same value
engine-remote-tag-local-other = ; the local tag with this name has another value ({ $value }) and stays
engine-remote-tag-local-none = ; there is no local tag with this name
engine-remote-tag-confirm = deletes tag `{ $name }` from remote `{ $remote }`, for everyone who uses it (value { $theirs }){ $local }. The value is first saved in `refs/project/backup/push-delete-...`; with `--force-with-lease`, if the tag moved on the remote in the meantime the deletion is rejected; to confirm: "confirm": true, "expected": "{ $theirs }"
engine-remote-tag-moved = tag `{ $name }` on remote `{ $remote }` moved since it was seen (expected { $expected }, now { $now }): the deletion is no longer the one confirmed{ $local }; to confirm: "confirm": true, "expected": "{ $now }"
engine-remote-tag-incomplete = tag `{ $name }` was deleted from only some of the push URLs of `{ $remote }` (or git failed afterwards): it may still be on the remote, see git's messages. The value is in `{ $backup }`
