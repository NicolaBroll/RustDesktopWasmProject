# Pull and push (sync.rs, view/sync_view.rs).
# $label: the operation description (sync-label-…); $what: sync-label-pull-branch.

sync-label-pull = Pull
sync-label-pull-branch = Pull di «{ $branch }»
sync-label-push = Push
sync-label-push-branch = Push di «{ $branch }»
sync-label-force = Push forzato
sync-label-force-branch = Push forzato di «{ $branch }»
sync-unexpected-response = risposta di pull o push inattesa
sync-pull-cancelled = { $label } interrotto: il branch non è stato toccato (i branch remoti già scaricati restano)
sync-push-cancelled = { $label } interrotto: se l'invio era già finito il remoto potrebbe essere aggiornato; fare Fetch per controllare
sync-failed-auth = { $label } non riuscito: autenticazione rifiutata o richiesta
sync-failed-unreachable = { $label } non riuscito: remoto non raggiungibile
sync-pull-conflict = { $label } annullato: il { $operation } ha conflitti, il repo è com'era
sync-rejected = { $label } rifiutato: { $reason }
sync-not-possible-title = { $label } non possibile
sync-not-possible = { $label } non possibile: { $error }
sync-failed-title = { $label } non riuscito
sync-rejection-non-fast-forward = Il remoto ha commit che non hai: fai Pull per integrarli, poi ripeti il push. Niente è cambiato, né sul remoto né in locale.
sync-rejection-stale-lease = Il branch sul remoto si è spostato dall'ultimo fetch: il push forzato (con lease) è stato rifiutato per non cancellare commit che non hai visto. Fai Fetch e controlla i commit arrivati prima di decidere di nuovo. Niente è cambiato.
sync-rejection-remote = Il remoto ha rifiutato l'aggiornamento (un branch protetto, un hook pre-receive o update sul server). Il motivo, se il server lo dà, è nel messaggio di git. Niente è cambiato.
sync-rejection-hook = Un hook pre-push (o un controllo locale di git, come i sottomoduli non pubblicati) ha fermato il push prima che partisse. Il suo output è qui sotto. Niente è cambiato.
sync-rejection-other = Git ha rifiutato il push. Niente è cambiato.
sync-rejection-short-non-fast-forward = il remoto ha commit che non hai: fai Pull
sync-rejection-short-stale-lease = il remoto si è spostato dall'ultimo fetch: fai Fetch
sync-rejection-short-remote = il remoto ha rifiutato l'aggiornamento
sync-rejection-short-hook = un hook pre-push l'ha fermato
sync-rejection-short-other = rifiutato da git
sync-pull-stopped = { $what }: merge di { $upstream } fermo prima del commit (un hook o la firma?): concluderlo con «Continua il merge…» nel riquadro in alto, o annullarlo
sync-pull-stopped-conflicts = { $what }: merge di { $upstream } fermo: { $n } file in conflitto. Risolverli e segnarli come risolti nell'area di stage, poi «Continua il merge…» nel riquadro in alto (o «Annulla il merge…», che riporta il branch com'era)
sync-pull-stopped-rebase = { $what }: rebase su { $upstream } fermo prima di un commit (un hook o la firma?): concluderlo con «Continua il rebase…» nel riquadro in alto, o annullarlo
sync-pull-stopped-conflicts-rebase = { $what }: rebase su { $upstream } fermo: { $n } file in conflitto. Risolverli e segnarli come risolti nell'area di stage, poi «Continua il rebase…» nel riquadro in alto (o «Annulla il rebase…», che riporta il branch com'era)
sync-pull-up-to-date = { $what }: già aggiornato con { $upstream }
sync-pull-up-to-date-ahead = { $what }: già aggiornato con { $upstream } ({ $n ->
    [one] 1 commit locale
   *[other] { $n } commit locali
} da pubblicare con Push)
sync-pull-fast-forward = { $what }: fast-forward a { $upstream } ({ $range }, { $n ->
    [one] 1 commit nuovo
   *[other] { $n } commit nuovi
})
sync-pull-merge = { $what }: merge di { $upstream } ({ $range })
sync-pull-rebase = { $what }: rebase di { $n ->
    [one] 1 commit locale
   *[other] { $n } commit locali
} su { $upstream } ({ $range })
sync-pull-merge-default = ; merge perché pull.rebase non è configurato (git config pull.rebase true per il rebase)
sync-pull-backup-in-progress = ; il branch com'era è in { $backup }
sync-pull-backup = ; backup in { $backup }, «Annulla» lo riporta indietro
sync-pull-status-not-reread = Pull fatto, ma lo stato non si rilegge: { $error }
sync-pull-refs-not-reread = Pull fatto, ma i ref non si rileggono: { $error }
# $summary: empty or “ (summary)” with the leading space.
sync-push-new = Push di «{ $branch }»: creato { $target } sul remoto{ $summary }
sync-push-fast-forward = Push di «{ $branch }» su { $target }{ $summary }
sync-push-forced = Push forzato di «{ $branch }» su { $target }{ $summary }: { $n ->
    [one] 1 commit tolto
   *[other] { $n } commit tolti
} dal remoto
sync-push-up-to-date = Push di «{ $branch }»: { $target } è già aggiornato
sync-push-to-push-remote = ; su «{ $remote }» (branch.{ $branch }.pushRemote)
sync-push-to-push-default = ; su «{ $remote }» (remote.pushDefault)
sync-push-backup = ; il remoto com'era è salvato in { $backup }
sync-push-set-upstream = ; upstream impostato su { $target }
sync-push-status-not-reread = Push fatto, ma lo stato non si rilegge: { $error }
sync-push-refs-not-reread = Push fatto, ma i ref non si rileggono: { $error }
sync-close = Chiudi
sync-conflict-title = Pull annullato: conflitti
sync-conflict-explanation = Il { $operation } con l'upstream si è fermato in conflitto ed è stato annullato: il repo è com'era prima del pull (branch, indice e file non sono cambiati). Il fetch invece è fatto: il branch remoto mostra i commit arrivati.
sync-conflict-hint = Si può rifare il pull con { $other } (o con la stessa strategia): un pull che va in conflitto ora resta in corso e i file si risolvono qui, poi «Continua…» o «Annulla…» nel riquadro in alto; oppure continuare a lavorare sul branch e integrare dopo, o risolvere da terminale.
sync-conflict-no-files = Git non ha indicato i file.
sync-conflict-files = { $n ->
    [one] File in conflitto:
   *[other] { $n } file in conflitto:
}
sync-engine-message = Messaggio del motore
sync-pull-with = Pull con { $other }
sync-pull-with-hint = Riprova il pull con { $other } invece di { $operation }, solo per questa volta (la configurazione non cambia)
sync-rejected-title = Push rifiutato
sync-rejected-branch = Branch: «{ $branch }»
sync-pull-button = Pull
sync-pull-button-hint = Scarica e integra i commit del remoto, poi si potrà ripetere il push
sync-fetch-button = Fetch
sync-fetch-button-hint = Scarica i commit arrivati sul remoto per controllarli
sync-force-button = Forza (con lease)…
sync-force-button-hint = Sovrascrive il branch sul remoto con il tuo (force-with-lease: rifiutato se il remoto si è spostato dall'ultimo fetch). Se il remoto perderebbe commit chiede conferma e li elenca
sync-force-confirm-title = Sovrascrivere il branch sul remoto?
sync-force-confirm-title-branch = Push forzato di «{ $branch }»: sovrascrivere il branch sul remoto?
sync-force-confirm-explanation = Il push forzato sostituisce il branch sul remoto con il tuo: i commit qui sotto spariscono dal remoto, anche per chi li ha già scaricati o ci sta lavorando. Un push non si annulla da qui.
sync-force-confirm-backup = Prima del push il branch remoto com'era si salva in un backup (refs/project/backup/push-…), da cui si può ripubblicare a mano. Con il lease, se nel frattempo il remoto si è spostato il push è rifiutato.
sync-force-confirm-changed = Il branch remoto è cambiato da quando hai visto questa conferma: niente è stato pubblicato. Controlla di nuovo i commit qui sotto.
sync-force-confirm-lost = Cosa si perde sul remoto:
sync-force-confirm-button = Forza il push
sync-force-confirm-gone = Il branch remoto locale non c'è più: chiudere, fare Fetch e riprovare
sync-force-confirm-cancel = Non forzare
sync-label-push-tag = Push del tag «{ $tag }» su «{ $remote }»
sync-push-tag-new = Tag «{ $tag }» pubblicato su «{ $remote }» (su { $id })
sync-push-tag-new-annotated = Tag annotato «{ $tag }» pubblicato su «{ $remote }» (su { $id })
sync-push-tag-up-to-date = Il tag «{ $tag }» è già su «{ $remote }» con lo stesso valore
sync-rejection-tag-exists = Il remoto ha già un tag con questo nome, su un altro valore: un tag non viene mai sovrascritto. Non è cambiato niente. Per sostituirlo, prima «Elimina dal remoto…» (con la sua conferma), poi di nuovo il push.
sync-rejection-short-tag-exists = il remoto ha già questo tag su un altro valore
