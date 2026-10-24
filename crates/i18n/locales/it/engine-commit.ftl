engine-commit-amend-published = `amend` riscriverebbe un commit già pubblicato o con un tag (raggiungibile da { $refs }): chi lo ha già scaricato vedrebbe una storia diversa, il push andrebbe forzato e i tag resterebbero sul commit vecchio. Il commit riscritto resterebbe in un ref di backup
engine-commit-amend-unborn = `amend` senza un commit da riscrivere: il branch non ha ancora commit, niente committato
engine-commit-amend-merge = è in corso un merge: non si può fare `amend` dell'ultimo commit, niente committato. Il commit senza `amend` conclude il merge
engine-commit-conflicts = ci sono file in conflitto, niente committato: risolverli e metterli in stage (`index.stage`) prima del commit: { $files }
engine-commit-message-empty = il messaggio è vuoto: scrivere almeno l'oggetto del commit
engine-commit-message-nul = il messaggio contiene un byte NUL
engine-commit-operation = è in corso { $operation ->
    [rebase] un rebase
    [cherry-pick] un cherry-pick
    [am] un `git am`
   *[revert] un revert
}: un commit da qui lo cambierebbe in modo poco visibile, niente committato. Concludere o annullare l'operazione (anche da terminale) e riprovare
engine-commit-amend-empty = `amend` renderebbe vuoto l'ultimo commit (stesso contenuto del genitore), niente committato
engine-commit-memory-merge-head = il repo in memoria non ha `MERGE_HEAD` per questo merge (impostato con `set_operation`): il commit di merge non è supportato, niente committato
engine-commit-head-unreadable = HEAD non si rilegge ({ $error })
engine-commit-head-no-commit = HEAD non punta a un commit
engine-commit-unreadable = il commit non si legge ({ $error })
engine-commit-rewritten-unreadable = il commit riscritto non si legge ({ $error })
engine-commit-reflog-unreadable = il reflog di `{ $name }` non si legge ({ $error })
engine-commit-head-moved = HEAD ({ $head }) non è il commit appena creato sopra { $previous }: un altro processo git o un hook (es. `post-commit`) ha spostato HEAD prima o dopo il commit
engine-commit-head-moved-unborn = HEAD ({ $head }) non è il commit appena creato sopra nessun commit: un altro processo git o un hook (es. `post-commit`) ha spostato HEAD prima o dopo il commit
engine-commit-silent-failure = terminato con { $status } senza messaggi (un hook che rifiuta senza scrivere niente?)
engine-commit-backup-kept = HEAD non è più { $old }: il commit da riscrivere è salvato in `{ $name }`
engine-commit-backup-note =
    { $text }
    
    { $note }
