# Git LFS in the view (S63).

# The badge of a file stored in Git LFS, in the stage area and in the diff bar.
lfs-badge = LFS
lfs-badge-hint = Stored in Git LFS: git keeps a small pointer file, the content is in the LFS store

# In the diff of an LFS file, in place of the pointer text; $size: the file's size.
lfs-stored = Stored in Git LFS ({ $size })
lfs-stored-unknown = Stored in Git LFS
lfs-size-bytes = { $n ->
    [one] 1 byte
   *[other] { $n } bytes
}
lfs-size-kb = { $n } KB
lfs-size-mb = { $n } MB
lfs-size-gb = { $n } GB

# The notice when the repository uses Git LFS and git-lfs is not installed.
lfs-missing = This repository uses Git LFS, which is not installed: files stay as pointers
lfs-missing-hint = The files stored in Git LFS are small pointer files in the working tree, not their content. Install git-lfs (https://git-lfs.com), run “git lfs install” and check out again to get the content; committing a pointer changed by hand would store that text.
lfs-missing-dismiss = Close
lfs-missing-dismiss-hint = Close the notice: it is not shown again for this repository until the app restarts
