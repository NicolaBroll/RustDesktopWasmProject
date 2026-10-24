# `remote.push` errors (`push.rs`, `builtin/push.rs`, backend).
engine-push-non-fast-forward = il remoto ha commit che il branch locale non ha: fare `remote.pull` (o fetch e merge/rebase) e ripetere il push
engine-push-stale-lease = il branch sul remoto si è spostato rispetto al branch remoto locale (force-with-lease): fare `remote.fetch`, controllare i commit arrivati e decidere di nuovo
engine-push-remote-rejected = il remoto ha rifiutato l'aggiornamento
engine-push-hook = un hook `pre-push` (o un controllo locale di git) ha fermato il push prima che partisse
engine-push-other = git ha rifiutato il push
engine-push-no-tracking = (nessun branch remoto locale)
engine-push-lost-others = e altri { $n }
engine-push-lost-and-others = { $commits }, e altri { $n }
engine-push-moved = `{ $shown }` si è spostato da quando è stato visto (atteso { $expected }, ora { $now }): il force non è più quello confermato. Il remoto perderebbe { $n } commit che `{ $branch }` non ha ({ $commits }); per confermare: "confirm": true, "expected": "{ $now }"
engine-push-moved-gone = `{ $shown }` si è spostato da quando è stato visto (atteso { $expected }, ora assente): il force non è più quello confermato. Il remoto perderebbe { $n } commit che `{ $branch }` non ha ({ $commits })
engine-push-force-confirm = il force sovrascriverebbe `{ $remote }/{ $remote_branch }` ({ $shown } = { $theirs }): il remoto perderebbe { $n } commit che `{ $branch }` non ha ({ $commits }). Prima si salverebbe `{ $shown }` in `refs/project/backup/push-...`; con `--force-with-lease`, se nel frattempo il remoto si è spostato il push è rifiutato; per confermare: "confirm": true, "expected": "{ $theirs }"
engine-push-upstream-other-name = l'upstream di `{ $branch }` è `{ $remote }/{ $remote_branch }`, con un nome diverso: per sicurezza (come `push.default=simple`) non si pubblica su un altro branch, niente pubblicato. Cambiare l'upstream o rinominare il branch
engine-push-upstream-not-branch = l'upstream di `{ $branch }` non è un branch (`{ $merge }`): niente pubblicato
engine-push-mirror = il remoto `{ $remote }` è un mirror (`remote.{ $remote }.mirror=true`): git ne pubblicherebbe tutti i ref insieme, e qui si pubblica solo un branch. Niente pubblicato; usare git da terminale per il mirror
engine-push-bad-remote-config = `{ $key }` indica `{ $remote }`, che non è un remoto configurato ({ $remotes }): niente pubblicato; correggere la configurazione o indicare `remote`
engine-push-local-upstream = l'upstream di `{ $branch }` è un branch locale (`{ $merge }`): indicare `remote`, niente pubblicato
engine-push-no-branch = non esiste un branch locale `{ $branch }` con commit: niente pubblicato
engine-push-detached = HEAD è staccata: niente pubblicato. Passare a un branch (`branch.checkout`) o indicare `branch`
engine-push-expected-hex = `expected` deve essere l'id completo ({ $len } cifre esadecimali) del branch remoto locale mostrato nella conferma: `{ $value }`
engine-push-confirm-without-expected = con `"confirm": true` serve `expected`: l'id completo del branch remoto locale mostrato nella conferma (la conferma vale solo per quel commit)
engine-push-confirm-without-force = `confirm` ed `expected` servono solo con `"force": true`
engine-push-lost-commit = { $id } «{ $summary }»
