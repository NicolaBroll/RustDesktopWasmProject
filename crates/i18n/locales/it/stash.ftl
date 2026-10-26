# Stash: section, panel, dialogs and messages (stash.rs, view/stash_view.rs).
# $name: the stash name (stash@{0}); $title: its message without git's prefix.

stash-origin-branch = su { $branch } · { $base }
stash-origin-detached = HEAD staccata · { $base }
stash-tooltip-created = Creato su { $base } il { $date } (UTC)
stash-tooltip-untracked = Contiene anche file non tracciati: non si vedono nei file dello stash, ma applicandolo tornano
stash-tooltip-click = Clic: mostra i file dello stash
stash-tooltip-right = Clic destro: applica, pop, elimina
stash-push-label = Stash delle modifiche
stash-pop-label = Pop di { $name }
stash-apply-label = Applicazione di { $name }
stash-drop-check-label = Controllo per l'eliminazione di { $name }
stash-drop-label = Eliminazione di { $name }
stash-push-failed = Stash non riuscito: { $error }
stash-push-done-what = Stash creato
stash-push-done = Modifiche messe da parte in { $name } («{ $title }»). «Annulla» le rimette
stash-push-done-untracked = Modifiche messe da parte in { $name } («{ $title }», con i file non tracciati). «Annulla» le rimette
stash-op-not-allowed = { $label } non possibile: { $error }
stash-op-failed = { $label } non riuscita: { $error }
stash-op-done-what = { $label } fatta
stash-pop-not-dropped-warning = { $name } applicato, ma non tolto dall'elenco: le modifiche sono già nei file. Non ripetere il pop; per toglierlo usare Elimina…
stash-drop-not-allowed = Eliminazione di { $name } non possibile: { $error }
stash-drop-failed = Eliminazione di { $name } non riuscita: { $error }
stash-drop-done-what = Stash eliminato
stash-drop-done = { $name } («{ $title }») eliminato; backup in { $backup }. «Annulla» lo rimette in cima all'elenco
stash-reread-stashes = gli stash non si rileggono: { $error }
stash-reread-status = lo stato non si rilegge: { $error }
stash-reread-but = { $what }, ma { $problems }
stash-pop-done = { $name } applicato ed eliminato dall'elenco
stash-pop-done-backup = { $name } applicato ed eliminato dall'elenco; backup in { $backup }
stash-pop-not-dropped = { $name } applicato, ma non tolto dall'elenco: non ripetere il pop, eliminarlo con Elimina…
stash-apply-done = { $name } applicato; resta nell'elenco
stash-apply-conflicts = { $n ->
    [one] { $name } applicato con 1 file in conflitto: risolverlo e metterlo in stage; lo stash resta nell'elenco
   *[other] { $name } applicato con { $n } file in conflitto: risolverli e metterli in stage; lo stash resta nell'elenco
}
stash-not-undoable = . Non si annulla con «Annulla»
stash-section-title = Stash ({ $n })
stash-section-title-loading = Stash
stash-none = Nessuno stash
stash-loading = Lettura degli stash…
stash-list-error = Stash non letti: { $error }
stash-push-help = Mette da parte le modifiche (in stage e no; a scelta anche i file non tracciati) e riporta i file come l'ultimo commit. «Annulla» le rimette
stash-no-changes = Nessuna modifica da mettere da parte
stash-origin-untracked = { $origin } · con non tracciati
stash-apply-button = Applica
stash-apply-help = Rimette nei file le modifiche di { $name }; lo stash resta nell'elenco. Mai sopra file con modifiche locali: in quel caso niente cambia. Non si annulla con «Annulla»
stash-pop-button = Pop
stash-pop-help = Applica { $name } e, se non ci sono conflitti, lo toglie dall'elenco (il suo commit resta in un backup refs/project/backup/stash-…). Con conflitti lo stash resta. Non si annulla con «Annulla»
stash-pop-blocked = Già applicato da un pop che non è riuscito a toglierlo dall'elenco: le modifiche sono nei file, non va ripetuto. Per toglierlo, Elimina…
stash-restore-index = Rimetti anche lo stage
stash-restore-index-help = Rimette in stage quello che era in stage quando lo stash è stato creato (--index); senza, tutto torna fuori stage tranne i file nuovi
stash-drop-button = Elimina…
stash-drop-help = Toglie lo stash dall'elenco; prima chiede conferma. Il suo commit resta in un backup (refs/project/backup/stash-…) e «Annulla» lo rimette in cima all'elenco
stash-field-stash = Stash
stash-field-commit = Commit
stash-copy = Copia
stash-copy-help = Copia l'id completo
stash-field-created-on = Creato su
stash-branch = (branch { $branch })
stash-detached = (HEAD staccata)
stash-field-date = Data
stash-date-utc = { $date } UTC
stash-field-message = Messaggio
stash-untracked-note = Contiene anche file non tracciati: non sono nell'elenco qui sotto, ma applicando lo stash tornano.
stash-changed-files = File tracciati cambiati rispetto alla base:
stash-files-loading = Caricamento dei file dello stash…
stash-retry = Riprova
stash-dialog-title = Stash delle modifiche
stash-dialog-body = Le modifiche ai file tracciati (in stage e no) si mettono da parte in uno stash e i file tornano come l'ultimo commit. Si riprendono con Applica o Pop dalla sezione Stash; «Annulla» nella barra le rimette subito.
stash-dialog-message = Messaggio
stash-dialog-message-hint = facoltativo (es. a metà del parser)
stash-include-untracked = Includi i file non tracciati
stash-include-untracked-count = Includi i file non tracciati ({ $n })
stash-include-untracked-help = Mette nello stash anche i file nuovi non ancora aggiunti (e li toglie dal disco); mai quelli ignorati. Senza, restano dove sono
stash-keep-index = Tieni lo stage
stash-keep-index-help = Quello che è in stage resta in stage (e nei file), ma è salvato anche nello stash; il resto si mette da parte
stash-submit = Stash
stash-close = Chiudi
stash-sending = Stash…
stash-drop-title = Eliminare { $name }?
stash-drop-body = Lo stash «{ $title }» esce dall'elenco: le modifiche che contiene non si potranno più applicare da qui. I file del repo non cambiano.
stash-drop-what-happens = Cosa succede:
stash-drop-undo = Con «Annulla» nella barra in alto lo stash torna in cima all'elenco (stash@{"{"}0{"}"}), finché l'app resta aperta: il diario delle operazioni è in memoria. Dopo la chiusura resta il backup del suo commit in refs/project/backup/stash-….
stash-drop-confirm = Elimina lo stash
stash-drop-keep = Non eliminare
stash-conflicts-title-pop = Pop di { $name }: conflitti
stash-conflicts-title-apply = Applicazione di { $name }: conflitti
stash-conflicts-body-apply = Lo stash è stato applicato, ma dall'ultimo commit su cui era stato creato gli stessi file sono cambiati: quelli qui sotto sono in conflitto (con i marcatori di git dove git li ha messi). Gli altri file dello stash sono applicati. Niente si è perso: quei file non avevano modifiche locali e { $name } resta nell'elenco.
stash-conflicts-body-pop = Lo stash è stato applicato, ma dall'ultimo commit su cui era stato creato gli stessi file sono cambiati: quelli qui sotto sono in conflitto (con i marcatori di git dove git li ha messi). Gli altri file dello stash sono applicati. Niente si è perso: quei file non avevano modifiche locali e { $name } resta nell'elenco (il pop non lo elimina).
stash-conflicts-how = Per risolverli: correggere ogni file e metterlo in stage dall'area delle modifiche. Poi, se non serve più, eliminare lo stash. Finché restano conflitti né Stash né Applica sono possibili.
stash-conflicts-files = { $n ->
    [one] File in conflitto:
   *[other] { $n } file in conflitto:
}
stash-go-to-changes = Vai alle modifiche
stash-go-to-changes-help = Apre l'area di stage con i file in conflitto
