# Remotes and upstream (remotes.rs).

remotes-ahead = { $n ->
    [one] 1 commit to push
   *[other] { $n } commits to push
}
remotes-behind = { $n ->
    [one] 1 commit to pull
   *[other] { $n } commits to pull
}
remotes-in-sync = Up to date with { $name }
remotes-relative = { $counts } compared to { $name }
remotes-upstream-gone = upstream gone
remotes-upstream-gone-hint = The upstream { $name } no longer exists (deleted on the remote, or the remote no longer has it): a Fetch won't bring it back; Push recreates it
