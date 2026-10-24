# Engine errors (`EngineError`): the text of each variant. `Display` is the English text (CLI,
# MCP, tests); the UI shows them in its language. git's text (`$stderr`, `$message`) is kept
# as is.
engine-unknown-command = comando sconosciuto: `{ $id }`
engine-invalid-params = parametri non validi per `{ $id }`: { $reason }
engine-repo-not-found = `{ $path }` non è un repository git: { $reason }
engine-no-repo = nessun repository aperto: aprirne uno con `repo.open`
engine-unknown-rev = revisione sconosciuta: `{ $rev }`
engine-git-not-found = impossibile eseguire `git`: { $error }
engine-git-too-old = git { $found } non è supportato: serve almeno la versione { $required }
engine-git = `git { $command }` non riuscito: { $stderr }
engine-index-locked = l'indice è bloccato: `{ $lock }` esiste. Un altro processo git sta lavorando sul repository; riprovare quando ha finito. Se nessun git è in esecuzione il file è rimasto da un crash e va rimosso a mano
engine-not-allowed = operazione rifiutata: { $reason }
engine-stale-diff = il diff di `{ $path }` non è più quello letto ({ $reason }): niente modificato, rileggere il diff e rifare la selezione
engine-nothing-to-commit = niente da committare: l'indice è uguale a HEAD. Mettere in stage le modifiche (`index.stage`), oppure usare `amend` per cambiare solo il messaggio dell'ultimo commit
engine-confirm-required = `{ $command }` chiede una conferma: { $reason }. Niente modificato; per procedere ripetere il comando con `"confirm": true`
engine-post-commit-id-backup = il commit { $id } è stato creato, ma { $reason }; il commit riscritto da `amend` è salvato in `{ $backup }`: non ripetere il commit, rileggere stato e log
engine-post-commit-id = il commit { $id } è stato creato, ma { $reason }: non ripetere il commit, rileggere stato e log
engine-post-commit-backup = il commit è stato creato, ma { $reason }; il commit riscritto da `amend` è salvato in `{ $backup }`: non ripetere il commit, rileggere stato e log
engine-post-commit = il commit è stato creato, ma { $reason }: non ripetere il commit, rileggere stato e log
engine-undo-conflict = `{ $command }` rifiutato: i ref sono cambiati dopo l'operazione ({ $refs }). Niente modificato: per non sovrascrivere lavoro fatto altrove, l'operazione va annullata a mano
engine-checkout-conflict = checkout di `{ $target }` rifiutato: sovrascriverebbe modifiche locali ({ $files }). Niente modificato: committare, mettere da parte o scartare queste modifiche e riprovare
engine-auth = autenticazione con il remoto `{ $remote }` non riuscita: { $message }. Niente modificato: controllare le credenziali (credential helper, chiave SSH caricata nell'agent, host noto in `known_hosts`)
engine-remote-unreachable = il remoto `{ $remote }` non è raggiungibile: { $message }. Niente modificato
engine-cancelled = `{ $command }` annullato
engine-pull-conflict = pull di `{ $branch }` interrotto: il { $operation } con `{ $upstream }` ha conflitti in { $files }. Il { $operation } è stato annullato e il branch è com'era (il fetch è fatto): per integrare `{ $upstream }` risolvendo i conflitti, `git { $operation } { $upstream }` da terminale, oppure usare l'altra strategia (`strategy`)
engine-push-rejected = push di `{ $branch }` su `{ $remote }` rifiutato: { $reason }. Niente modificato ({ $message })
# Errors from the layers below the engine (core, domain): their text.
engine-other = { $text }
engine-ignored = file ignorati da .gitignore (o da un altro file di esclusione), niente messo in stage: { $files }
engine-unmerged = file in conflitto, niente tolto dallo stage (si perderebbero le versioni del conflitto; per risolverlo va messo in stage): { $files }
engine-unmerged-in-folder = la cartella contiene file in conflitto, niente messo in stage (metterli in stage li segnerebbe come risolti: vanno chiesti per nome): { $files }
engine-empty-commit-message = il messaggio è vuoto dopo la pulizia di git (`commit.cleanup` toglie le righe di commento e gli spazi), niente committato: scrivere un messaggio che non sia solo commenti
# Command parameters (`builtin.rs`).
engine-params = { $error }
engine-path-empty = il percorso è vuoto: `{ $path }`
engine-path-nul = il percorso contiene un byte NUL: `{ $path }`
engine-path-not-relative = il percorso deve essere relativo alla radice del repo, con `/` come separatore e senza componenti vuoti, `.` o `..`: `{ $path }`
engine-discard-confirm = le modifiche non in stage di { $n } file andrebbero perse ({ $files }): il worktree tornerebbe come l'indice. Prima si salverebbe un backup in `refs/project/backup/discard-...`, e `edit.undo` le rimetterebbe
engine-status-unreadable = lo stato non si rilegge ({ $error })
engine-lines-empty = `lines` è vuoto: indicare almeno una riga cambiata del diff
engine-paths-empty = `paths` è vuoto: indicare almeno un percorso
engine-log-stashes-without-all = `stashes` aggiunge le basi degli stash al grafo di tutti i ref: va insieme ad `"all": true`
engine-log-rev-and-all = `rev` e `all` sono alternativi: `all` parte già da tutti i branch, i tag e HEAD
engine-memory-repos = repo in memoria disponibili: { $repos }
engine-memory-no-repo = nessun repo in memoria
# Branch names (`git/branch.rs`).
engine-with-name = { $reason }: `{ $name }`
engine-branch-name-empty = il nome del branch è vuoto
engine-branch-name-full = usare il nome breve del branch (senza `refs/heads/`)
engine-branch-name-invalid = non è un nome di branch valido
engine-branch-name-dash = un nome di branch non può iniziare con `-`
engine-branch-name-reserved = nome riservato da git
# Operation in progress (`check_no_operation`): `$what` is one of the `engine-busy-*`.
engine-operation-in-progress = è in corso { $operation ->
    [merge] un merge
    [rebase] un rebase
    [cherry-pick] un cherry-pick
    [revert] un revert
    [bisect] un bisect
   *[am] un `git am`
}: `{ $command }` { $what } a metà dell'operazione, niente modificato. Concludere o annullare l'operazione e riprovare
engine-busy-checkout = cambierebbe branch
engine-busy-branch-delete = eliminerebbe un branch
engine-busy-branch-rename = rinominerebbe un branch
engine-busy-stash-apply = applicherebbe lo stash
engine-busy-stash-push = metterebbe da parte le modifiche
engine-busy-stash-undo = cambierebbe gli stash
engine-busy-restore = sposterebbe i ref
engine-busy-pull = integrerebbe l'upstream
engine-busy-remote-remove = rimuoverebbe un remoto
engine-busy-remote-restore = rimetterebbe un remoto
engine-busy-cherry-pick = inizierebbe un cherry-pick
engine-busy-revert = inizierebbe un revert
engine-busy-worktree = riscriverebbe file
engine-busy-branch-move = sposterebbe il branch
engine-busy-merge = inizierebbe un altro merge
# Refs changed from outside (`UndoConflict`, one line per ref).
engine-head-branch = sul branch `{ $name }`
engine-head-detached = staccata su { $id }
engine-head-changed = HEAD: doveva essere { $expected }, ora è { $now }
engine-ref-changed = `{ $name }`: doveva essere { $expected }, ora è { $now }
engine-ref-value = { $id }
engine-ref-missing = assente
engine-discard-index-changed = `{ $path }`: la versione in stage è cambiata dopo lo scarto
engine-restore-head-move = `{ $command }` dovrebbe spostare HEAD { $head_expected } a { $head_target }: non ancora supportato, niente modificato
engine-journal-bad-ref = nome di ref non valido nel diario: `{ $name }`, niente modificato
# Operations in progress, with the article (`operation_name`).
engine-op-merge = un merge
engine-op-rebase = un rebase
engine-op-cherry-pick = un cherry-pick
engine-op-revert = un revert
engine-op-bisect = un bisect
engine-op-am = un `git am`
# Opening the repository (`repo.open`).
engine-open-empty-path = percorso vuoto
engine-open-bare = è un repository bare, senza cartella di lavoro: non supportato
engine-open-object-format = formato degli oggetti `{ $format }` non supportato (solo sha1)
engine-open-object-format-unknown = formato degli oggetti `{ $format }` sconosciuto
engine-index-flags-not-restored = l'indice è stato scritto correttamente, ma non si è riusciti a rimettere `{ $flag }` sui file che lo avevano (rimetterlo a mano con `git update-index { $flag } <percorso>`): { $error }
engine-backup-no-name = nessun nome libero per il backup dopo `{ $base }`
engine-git-unexpected-output = output inatteso: `{ $output }`
engine-git-no-commit = nessun commit letto per `{ $hex }`
engine-command-quoted = `{ $id }`
# An error with a note at the end (what was left changed, where the backup is).
engine-note-newline =
    { $text }
    ({ $note })
engine-note-paren = { $text } ({ $note })
engine-note-semicolon = { $text }; { $note }
engine-git-spawn = { $program }: { $error }
engine-git-exit-status = terminato con { $status }
engine-git-version-unknown = risposta di `git --version` non riconosciuta: `{ $output }`
engine-memory-not-a-commit = fatal: { $id } non è un commit del repo
engine-memory-unknown-blob = fatal: blob sconosciuto { $oid }
engine-memory-unknown-staged = error: { $target }: contenuto in stage sconosciuto
engine-memory-backup-missing = backup del worktree { $stash } non trovato
engine-nothing = niente
engine-head-moved-after = dopo `{ $command }` HEAD non è più su `{ $full }`: un hook o un altro processo l'ha cambiata; controllare HEAD prima di continuare
engine-ref-moved-after = dopo `{ $command }` `{ $name }` vale { $now } invece di { $target }: un altro processo l'ha spostato durante l'operazione
