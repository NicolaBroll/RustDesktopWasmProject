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
settings-remotes = Remote
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
