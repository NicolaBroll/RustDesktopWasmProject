# Diff view and staging of hunks and lines (diff.rs, diff_view.rs, lines.rs).

diff-submodule-badge = [submodule]
diff-note-mode = mode { $old } -> { $new }
diff-note-new-file = new file, mode { $mode }
diff-note-deleted-file = deleted file, mode { $mode }
diff-note-submodule = submodule: the diff shows the commit it points to
diff-reason-unmerged = unmerged file: the diff is not available until the conflict is resolved
diff-reason-binary = binary file: the content is not shown
diff-reason-exact-rename = exact rename: content unchanged
diff-reason-exact-copy = exact copy: content unchanged
diff-reason-empty-file = empty file
diff-reason-empty-file-deleted = empty file deleted
diff-reason-mode-only = mode change only: content unchanged
diff-reason-no-change = no content changes
diff-loading = Loading the diff of { $path }…
diff-retry = Retry
diff-empty-worktree = No differences from the index. An untracked file has no diff until it is staged: after staging, its content shows up among the staged changes.
diff-empty = No differences for this file.
diff-back = < Graph
diff-back-hint = Close the diff and go back to the graph (Esc)
diff-in-commit = in commit { $id }
diff-unstaged = unstaged
diff-unstaged-hint = Changes in the working tree compared to the index
diff-staged = staged
diff-staged-hint = Changes in the index compared to the last commit (HEAD)
diff-partial-unavailable = Staging by hunk and by line is not available
diff-stage-selected = Stage selected lines ({ $n })
diff-unstage-selected = Unstage selected lines ({ $n })
diff-stage-selected-hint = Stages only the selected lines; the rest of the file stays out
diff-unstage-selected-hint = Unstages only the selected lines; the working tree does not change
diff-no-lines-selected = No lines selected: click a + or - line to select it, Shift+click for a range, drag for more lines
diff-wait-write = Wait for the current write to finish
diff-clear = Deselect
diff-clear-hint = Deselect all lines (Esc)
diff-stage-hunk = Stage hunk
diff-unstage-hunk = Unstage hunk
diff-stage-hunk-hint = Stages all the changed lines of this hunk
diff-unstage-hunk-hint = Unstages all the changed lines of this hunk; the working tree does not change
diff-line-cut = {" "}… ({ $n } more characters)
diff-lines-type-changed = the file changed type (for example from file to link): lines cannot be picked, only the whole file can be moved in or out of the staging area
diff-lines-label-stage-hunk = Stage hunk { $hunk } of { $path }
diff-lines-label-unstage-hunk = Unstage hunk { $hunk } of { $path }
diff-lines-label-stage-lines = { $n ->
    [one] Stage 1 line of { $path }
   *[other] Stage { $n } lines of { $path }
}
diff-lines-label-unstage-lines = { $n ->
    [one] Unstage 1 line of { $path }
   *[other] Unstage { $n } lines of { $path }
}
