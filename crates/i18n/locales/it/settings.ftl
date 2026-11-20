# Settings window: sections, theme, language, scale and dates (settings.rs, view/settings_view.rs).

settings-title = Impostazioni
settings-close = Chiudi
settings-general = Generale
settings-theme = Tema
settings-theme-system = Come il sistema
settings-theme-light = Chiaro
settings-theme-dark = Scuro
settings-language = Lingua
settings-language-system = Come il sistema
settings-language-hint = Applicata subito; una lingua di sistema diversa da italiano o inglese mostra l'inglese.
settings-remotes = Remoti
settings-fetch = Fetch automatico
settings-fetch-off = Disattivato
settings-fetch-minutes = { $n ->
    [one] Ogni minuto
   *[other] Ogni { $n } minuti
}
settings-fetch-hint = Ogni scheda aperta scarica dal suo remote in background (fetch, mai pull): branch locali e file non vengono toccati. Aspetta finché è aperta una finestra, gira un comando o c'è un'operazione in corso.
settings-scale = Dimensione dell'interfaccia
settings-scale-value = { $n }%
settings-scale-hint = Anche { $larger } e { $smaller }; { $reset } torna al 100%.
settings-dates = Date nell'elenco dei commit
settings-dates-absolute = Data e ora
settings-dates-relative = Relative (“3 h fa”)
settings-ask-detach = Chiedi prima di fare il checkout di un commit (HEAD staccata)
settings-ask-detach-hint = “Checkout di questo commit” spiega prima che non sarai su nessun branch.
settings-editor = Editor esterno
settings-editor-placeholder = L'applicazione predefinita del sistema
settings-editor-hint = Una riga di comando, per esempio: code -g {"{file}"}. {"{file}"} è il percorso del file (senza, il percorso va in fondo); metti tra virgolette le parole con spazi. Avviato senza shell. Vuoto: l'applicazione predefinita del sistema.

# La sezione «Git»: nome ed email di git (identity.rs, S65).
settings-git = Git
settings-identity = Nome ed email dei tuoi commit
settings-identity-hint = Git li scrive in ogni commit che crei. Fanno parte della configurazione di git, non delle impostazioni di questa app: non cambia nulla finché non premi «Salva».
settings-identity-global = Per tutti i repository
settings-identity-global-hint = La tua configurazione globale di git (git config --global): quella che usa ogni repository senza valori propri.
settings-identity-local = Solo per questo repository
settings-identity-local-hint = La configurazione di «{ $repo }» (git config --local): vince sui valori per tutti i repository.
settings-identity-name = Nome
settings-identity-email = Email
settings-identity-name-placeholder = Il tuo nome
settings-identity-email-placeholder = tu@example.com
settings-identity-inherited = { $value } (da tutti i repository)
settings-identity-save = Salva
settings-identity-nothing = Niente da salvare: cambia prima il nome o l'email.
settings-identity-saving = Salvataggio…
settings-identity-reading = Lettura della configurazione di git…
settings-identity-read-failed = Non è stato possibile leggere la configurazione di git: { $error }
settings-identity-retry = Riprova
settings-identity-save-failed = Nome ed email non salvati: { $error }
settings-identity-saved-global = Nome ed email salvati per tutti i repository. Prima: nome { $name }, email { $email }.
settings-identity-saved-local = Nome ed email salvati per questo repository. Prima: nome { $name }, email { $email }.
settings-identity-unchanged = La configurazione di git aveva già questi valori: non è stato scritto nulla.
settings-identity-quoted = «{ $value }»
settings-identity-unset = non impostato
settings-identity-used = Cosa usa git
settings-identity-used-repo = Cosa usa git in «{ $repo }»
settings-identity-not-set = non impostato
settings-identity-empty = impostato, ma vuoto
settings-identity-from-local = (impostato per questo repository)
settings-identity-from-global = (impostato per tutti i repository)
settings-identity-from-system = (dalla configurazione di sistema)
settings-identity-from-other = (da un'altra configurazione: quella del worktree o la riga di comando)
settings-identity-hidden-name = Questo repository ha un nome proprio: quello per tutti i repository («{ $global }») qui non è usato.
settings-identity-hidden-email = Questo repository ha un'email propria: quella per tutti i repository («{ $global }») qui non è usata.
settings-identity-env = { $n ->
    [one] La variabile d'ambiente { $names } è impostata dove è stata avviata l'app: git usa quella al posto di ciò che è salvato qui.
   *[other] Le variabili d'ambiente { $names } sono impostate dove è stata avviata l'app: git usa quelle al posto di ciò che è salvato qui.
}
settings-identity-env-email = Nessuna email è configurata: git la prende dalla variabile d'ambiente EMAIL.
