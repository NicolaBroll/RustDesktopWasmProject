engine-pull-local-changes = il pull farebbe un { $mode } ma ci sono modifiche locali ai file tracciati ({ $files }): committarle o metterle da parte e riprovare (niente `--autostash`); il branch non è stato toccato
engine-pull-conflicts = ci sono file in conflitto ({ $files }): risolverli prima del pull; il branch non è stato toccato
engine-pull-diverged-ff-only = `{ $branch }` e il suo upstream sono divergenti ({ $ahead } commit locali, { $behind } dell'upstream) e la strategia è solo fast-forward (`pull.ff=only` o `strategy` `ff_only`): il fetch è fatto, niente altro modificato. Ripetere con `strategy` `merge` o `rebase`
engine-pull-diverged-not-current = `{ $branch }` e il suo upstream sono divergenti ({ $ahead } commit locali, { $behind } dell'upstream) e `{ $branch }` non è il branch attivo: si può solo fare fast-forward. Il fetch è fatto, niente altro modificato; passare al branch (`branch.checkout`) e ripetere il pull
engine-pull-interactive = la configurazione chiede un rebase interattivo (`pull.rebase` o `branch.<nome>.rebase` = `interactive`), che vuole un editor: niente modificato. Indicare `strategy` (`merge`, `rebase` o `ff_only`)
engine-pull-config-value = valore di `{ $key }` non riconosciuto: `{ $value }`; niente modificato (correggere la configurazione o indicare `strategy`)
engine-pull-no-branch = non esiste un branch `{ $branch }`
engine-pull-undo-overwrite = `{ $command }` riporterebbe `{ $moved }` a { $target } ma sovrascriverebbe modifiche locali o file non tracciati ({ $files }): niente modificato; committarli o metterli da parte e riprovare
engine-pull-bad-request = `{ $command }`: richiesta non valida per il branch attivo, niente modificato
engine-pull-overwrite = il pull sovrascriverebbe modifiche locali o file non tracciati ({ $files }): committarli, metterli da parte o spostarli e riprovare; il branch non è stato toccato
engine-pull-rebase-untracked = il rebase passerebbe per file che ora esistono nel worktree come non tracciati o ignorati ({ $files }): git li sovrascriverebbe. Il branch non è stato toccato; spostarli o rinominarli e riprovare
engine-pull-not-current-ff = `{ $branch }` non è il branch attivo: si può solo fare fast-forward, niente modificato
engine-pull-head-moved = HEAD è cambiata mentre si preparava il pull: niente modificato, riprovare
engine-pull-tracking-missing = dopo il fetch `{ $tracking }` non c'è: niente da integrare, il branch non è stato toccato
engine-pull-no-refspec = nessun refspec di fetch del remoto `{ $remote }` scarica `{ $merge }` in un branch remoto locale (`remote.{ $remote }.fetch`): niente scaricato
engine-pull-upstream-not-branch = l'upstream di `{ $branch }` non è un branch (`{ $merge }`): niente scaricato
engine-pull-other-remote = `{ $branch }` segue il remoto `{ $upstream }`, non `{ $remote }`: il pull integra sempre l'upstream. Niente scaricato; per seguire un altro remoto cambiare l'upstream
engine-pull-local-upstream = l'upstream di `{ $branch }` è un branch locale (`{ $merge }`): il pull da un upstream locale non è supportato, niente modificato
engine-pull-no-branch-commits = non esiste un branch locale `{ $branch }` con commit: niente scaricato
engine-pull-detached = HEAD è staccata: niente scaricato. Passare a un branch (`branch.checkout`) o indicare `branch`
engine-pull-no-upstream = il branch `{ $branch }` non ha un upstream (`branch.{ $branch }.remote`/`branch.{ $branch }.merge`): niente scaricato né modificato. Impostarlo da terminale con `git branch --set-upstream-to=<remoto>/<branch> -- { $branch }`, oppure pubblicare il branch con `remote.push`, che al primo push imposta l'upstream
engine-pull-branch-gone = dopo il pull `{ $full }` non esiste più
engine-pull-overwrite-what = { $what } sovrascriverebbe modifiche locali o file non tracciati ({ $files }): committarli, metterli da parte o spostarli e riprovare; il branch non è stato toccato
engine-pull-undo-bad-entry = `{ $command }`: voce del diario non valida per il branch attivo, niente modificato
engine-pull-undo-overwrite-git = `{ $command }` riporterebbe `{ $name }` a { $to } ma sovrascriverebbe modifiche locali: niente modificato; committarle o metterle da parte e riprovare ({ $stderr })
engine-pull-undo-untracked = `{ $command }` ricreerebbe file che ora esistono nel worktree come non tracciati o ignorati ({ $files }): niente modificato; spostarli e riprovare
engine-pull-other-worktree = `{ $branch }` è il branch attivo del worktree `{ $worktree }`: il pull va fatto lì, niente modificato
engine-pull-incoming-untracked = { $what } porterebbe file che ora esistono nel worktree come non tracciati o ignorati ({ $files }): git li sovrascriverebbe. Il branch non è stato toccato; spostarli o rinominarli e riprovare
engine-pull-abort-failed = il { $operation } dell'upstream si è fermato ({ $text }) e `git { $operation } --abort` non è riuscito ({ $stderr }): il repository è ancora nel { $operation } in corso
engine-pull-aborted =
    { $text }
    (il { $operation } è stato annullato, niente modificato)
engine-pull-note-in-progress = il repository è rimasto in un'operazione in corso; il branch com'era è in `{ $backup }`
engine-pull-note-moved = `{ $full }` però si è spostato da { $from } a { $to }: git ha scritto qualcosa prima di fallire. Il pull non entra nel diario; il branch com'era è in `{ $backup }`
engine-pull-head-moved-merge = dopo il pull HEAD non è più su `{ $full }`: un hook o un altro processo l'ha cambiata; controllare HEAD prima di continuare il merge
engine-pull-branch-moved = `{ $full }` vale { $now } invece di { $expected }: un hook o un altro processo l'ha spostato; il branch com'era è in `{ $backup }`
engine-pull-merging-other = il merge in corso unisce { $incoming } invece di `{ $name }` ({ $upstream }): un hook o un altro processo l'ha cambiato; controllarlo prima di continuarlo
engine-pull-upstream-missing = dopo il pull `{ $full }` ({ $after }) non contiene `{ $name }`: un hook o un altro processo ha spostato il branch durante l'operazione
engine-pull-head-moved-after = dopo il pull HEAD non è più su `{ $full }`: un hook o un altro processo l'ha cambiata; controllare HEAD prima di continuare
