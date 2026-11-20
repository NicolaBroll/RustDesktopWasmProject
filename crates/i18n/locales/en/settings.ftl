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

# The "Git" section: git's own name and email (identity.rs, S65).
settings-git = Git
settings-identity = Name and email of your commits
settings-identity-hint = Git writes them in every commit you create. They are part of git's configuration, not of this app's settings: nothing changes until you press “Save”.
settings-identity-global = For all repositories
settings-identity-global-hint = Your global git configuration (git config --global): what every repository without its own values uses.
settings-identity-local = Only for this repository
settings-identity-local-hint = The configuration of “{ $repo }” (git config --local): it wins over the values for all repositories.
settings-identity-name = Name
settings-identity-email = Email
settings-identity-name-placeholder = Your name
settings-identity-email-placeholder = you@example.com
settings-identity-inherited = { $value } (from all repositories)
settings-identity-save = Save
settings-identity-nothing = Nothing to save: change the name or the email first.
settings-identity-saving = Saving…
settings-identity-reading = Reading git's configuration…
settings-identity-read-failed = Git's configuration could not be read: { $error }
settings-identity-retry = Retry
settings-identity-save-failed = Name and email not saved: { $error }
settings-identity-saved-global = Name and email saved for all repositories. Before: name { $name }, email { $email }.
settings-identity-saved-local = Name and email saved for this repository. Before: name { $name }, email { $email }.
settings-identity-unchanged = Git's configuration already had these values: nothing was written.
settings-identity-quoted = “{ $value }”
settings-identity-unset = not set
settings-identity-used = What git uses
settings-identity-used-repo = What git uses in “{ $repo }”
settings-identity-not-set = not set
settings-identity-empty = set, but empty
settings-identity-from-local = (set for this repository)
settings-identity-from-global = (set for all repositories)
settings-identity-from-system = (from the system configuration)
settings-identity-from-other = (from another configuration: the worktree's or the command line)
settings-identity-hidden-name = This repository has its own name: the one for all repositories (“{ $global }”) is not used here.
settings-identity-hidden-email = This repository has its own email: the one for all repositories (“{ $global }”) is not used here.
settings-identity-env = { $n ->
    [one] The environment variable { $names } is set where the app was started: git uses it instead of what is saved here.
   *[other] The environment variables { $names } are set where the app was started: git uses them instead of what is saved here.
}
settings-identity-env-email = No email is configured: git takes it from the EMAIL environment variable.
