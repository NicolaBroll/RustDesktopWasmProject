engine-merge-other-in-progress = è in corso { $other }, non { $operation }: `{ $command }` non lo tocca, niente modificato. Concluderlo o annullarlo con i suoi comandi (o da terminale)
engine-merge-not-in-progress = non c'è { $operation } in corso: `{ $command }` non ha niente da fare, niente modificato
engine-merge-local-changes = il merge creerebbe un commit di merge ma ci sono modifiche locali ai file tracciati ({ $files }): committarle o metterle da parte (`stash.push`) e riprovare (niente `--autostash`); niente modificato
engine-merge-conflicts = ci sono file in conflitto ({ $files }): risolverli prima del merge; niente modificato
engine-merge-abort-unborn = HEAD non ha un commit: niente annullato
engine-merge-abort-mixed = git non annulla l'operazione in corso: file in stage con anche modifiche fuori stage ({ $files }). Metterle in stage (`index.stage`) o scartarle (`worktree.discard`) e riprovare; niente modificato, `{ $command }` non ha annullato niente
engine-merge-blocked = { $what } sovrascriverebbe modifiche locali o file non tracciati ({ $files }): committarli, metterli da parte o spostarli e riprovare; niente modificato
engine-merge-not-fast-forward = `{ $target_name }` non contiene `{ $branch }`: il fast-forward non si può fare, niente modificato
engine-merge-overwrite = il merge sovrascriverebbe modifiche locali o file non tracciati ({ $files }): committarli, metterli da parte o spostarli e riprovare; niente modificato
engine-merge-head-moved = HEAD è cambiata mentre si preparava il merge: niente modificato, riprovare
engine-merge-what-merge = il merge
engine-merge-what-pull = il pull
engine-merge-abort-lose-conflicts = i file in conflitto ({ $files }) tornerebbero come in HEAD, con le risoluzioni fatte finora
engine-merge-abort-lose-changes = le modifiche del { $operation } e quelle fatte dopo, in stage o no, andrebbero perse ({ $files })
engine-merge-abort-lose-operation = il { $operation } in corso verrebbe annullato
engine-merge-continue-conflicts = restano file in conflitto ({ $files }): risolverli e metterli in stage (`index.stage`), poi ripetere `merge.continue`; niente committato
engine-merge-abort-confirm = { $lost }. Prima si salverebbero indice e worktree in `refs/project/backup/{ $backup }-abort-...` (si rimettono con `git stash apply <ref>`); l'annullamento non entra nel diario
engine-merge-diverged-ff-only = `{ $source }` e `{ $branch }` sono divergenti ({ $ahead } commit solo nel branch, { $behind } solo in `{ $source }`): con `ff_only` si può solo fare fast-forward, niente modificato
engine-merge-detached = HEAD è staccata: il merge va fatto su un branch, niente modificato. Passare a un branch (`branch.checkout`) o crearne uno qui (`branch.create`) e riprovare
engine-merge-unborn = il branch `{ $name }` non ha ancora commit: niente da unire, niente modificato
engine-merge-rev-empty = `rev` deve essere un nome di branch, di tag o un id, non vuoto
engine-merge-no-ff-and-ff-only = `no_ff` (sempre un commit di merge) e `ff_only` (solo fast-forward) si escludono
engine-merge-branch-not-moved = `git merge` è terminato senza errori ma `{ $full }` non si è spostato ({ $text }): controllare indice e worktree; il branch com'era è in `{ $backup }`
engine-merge-branch-gone = dopo il merge `{ $full }` non esiste più
engine-merge-message-not-written = messaggio del merge non scritto: { $error }
# `$hint`: `mixed` (staged file with unstaged changes) or `none`; `$state`: `still` or `maybe`.
engine-merge-abort-failed = `git { $git } --abort` non è riuscito ({ $stderr }).{ $hint ->
    [mixed] {" "}Succede quando un file in stage ha anche modifiche fuori stage: metterle in stage (`index.stage`) o scartarle (`worktree.discard`) e riprovare.
   *[none] {""}
} L'operazione { $state ->
    [still] è ancora
   *[maybe] potrebbe non essere più
} in corso e alcuni file potrebbero essere già stati riscritti: rileggere lo stato. Indice e worktree di prima sono salvati in `{ $backup }` (`git stash apply { $backup }`)
engine-merge-target-missing = dopo il merge `{ $full }` ({ $after }) non contiene `{ $target }`: un hook o un altro processo ha spostato il branch durante l'operazione
engine-merge-abort-head-moved = dopo l'annullamento HEAD non è più { $head }: un hook o un altro processo l'ha spostata
engine-merge-unexpected-result = il branch è a { $after } ma non è il risultato atteso del merge (un hook o un altro processo l'ha spostato?): l'operazione non entra nel diario, il branch com'era è in `{ $backup }`
engine-merge-head-moved-after = dopo il merge HEAD non è più su `{ $full }`: un hook o un altro processo l'ha cambiata; controllare HEAD prima di continuare
# Merge squash (`merge.start` con `squash`, S58).
engine-merge-squash-with-ff = `squash` non si può combinare con `no_ff` o `ff_only`
engine-merge-squash-record-not-written = non è stato possibile salvare il record dello squash nella cartella di git ({ $error }), niente modificato
engine-merge-squash-moved = `git merge --squash` è terminato ma HEAD non è più su `{ $full }` a { $expected } ({ $text }): controllare indice e worktree; il branch com'era è in `{ $backup }`
engine-merge-squash-empty = le modifiche di { $source } sono già in `{ $branch }`: niente da committare, niente modificato
engine-merge-squash-commit-failed = lo squash è in stage ma il commit non è riuscito ({ $error }): resta in corso, concluderlo con `merge.continue` o annullarlo con `merge.abort`
