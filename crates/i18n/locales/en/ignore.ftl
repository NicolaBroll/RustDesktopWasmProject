# Ignoring untracked files (ignore.rs and the "Ignore" submenu of the stage area).

ignore-this-file = This file
ignore-extension = All { $pattern } files
ignore-folder = Folder { $folder }/
ignore-entry-help = Adds the line “{ $pattern }” to { $file }
ignore-label = Ignoring “{ $pattern }” in { $file }
ignore-not-allowed = { $label } not possible: { $error }
ignore-done = Added “{ $pattern }” to { $file }; { $n ->
    [0] no untracked file matched it
    [one] 1 untracked file is now ignored
   *[other] { $n } untracked files are now ignored
}. “Undo” removes it
ignore-done-already = “{ $pattern }” is already in { $file }: nothing changed
ignore-status-not-reread = Pattern added, but the status can't be reread: { $reason }
