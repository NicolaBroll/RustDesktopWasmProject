# Settings window: sections, theme, language, scale and dates (settings.rs, view/settings_view.rs).

settings-title = Settings
settings-close = Close
settings-general = General
settings-theme = Theme
settings-theme-system = As the system
settings-theme-light = Light
settings-theme-dark = Dark
settings-language = Language
settings-language-system = As the system
settings-language-hint = Applied at once; a system language other than Italian or English shows English.
settings-remotes = Remotes
settings-fetch = Automatic fetch
settings-fetch-off = Off
settings-fetch-minutes = { $n ->
    [one] Every minute
   *[other] Every { $n } minutes
}
settings-fetch-hint = Every open tab downloads from its remote in the background (fetch, never pull): local branches and files are not touched. It waits while a dialog, a command or an operation is in progress.
settings-scale = Interface size
settings-scale-value = { $n }%
settings-scale-hint = Also { $larger } and { $smaller }; { $reset } goes back to 100%.
settings-dates = Dates in the commit list
settings-dates-absolute = Date and time
settings-dates-relative = Relative (“3 h ago”)
settings-ask-detach = Ask before checking out a commit (detached HEAD)
settings-ask-detach-hint = “Check out this commit” first explains that you will not be on a branch.
settings-editor = External editor
settings-editor-placeholder = The system's default application
settings-editor-hint = A command line, for example: code -g {"{file}"}. {"{file}"} is the file's path (without it the path goes at the end); quote words with spaces. Run without a shell. Empty: the system's default application.
