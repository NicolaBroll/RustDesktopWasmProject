# Scartare le modifiche fuori stage (discard.rs).

discard-reason-conflict = File in conflitto: va risolto prima, non si scarta da qui
discard-reason-untracked = File non tracciato: non si scarta (vorrebbe dire cancellarlo, senza backup)
discard-label-one = Scarto delle modifiche di { $path }
discard-label-many = Scarto delle modifiche di { $n } file
discard-check-label-one = Controllo per scarto delle modifiche di { $path }
discard-check-label-many = Controllo per scarto delle modifiche di { $n } file
discard-unexpected-response = risposta di `{ $command }` inattesa
discard-unexpected-result = { $command }: risultato inatteso: { $error }
discard-not-allowed = { $label } non possibile: { $error }
discard-failed = { $label } non riuscito: { $error }
discard-done-one = Modifiche di { $path } scartate; backup in { $backup }. «Annulla» le rimette
discard-done-many = Modifiche di { $n } file scartate; backup in { $backup }. «Annulla» le rimette
discard-status-not-reread = Modifiche scartate, ma lo stato non si rilegge: { $reason }
