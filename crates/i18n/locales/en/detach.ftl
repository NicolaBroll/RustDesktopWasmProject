# Checking out a commit with detached HEAD (detach.rs, view/branches.rs).

detach-heading = Check out commit { $id }?
detach-text = You will not be on a branch (detached HEAD): the files become those of this commit. New commits need a branch to be kept: “Create branch here…” in the top bar.
detach-dont-ask = Don't ask again
detach-dont-ask-hint = Can be turned back on in Settings
detach-button = Check out
detach-checkout-label = Checking out commit { $id }
detach-checked-out = HEAD detached at { $id }: you are not on a branch. “Create branch here…” keeps new commits; “Undo” goes back
detach-already = HEAD is already detached at { $id }
detach-saved = { $n ->
    [one] 1 commit not on any branch was saved as { $backup }
   *[other] { $n } commits not on any branch were saved as { $backup }
}
detach-saved-unknown = the commits not on any branch were saved as { $backup }
