# Discarding unstaged changes (discard.rs).

discard-reason-conflict = File in conflitto: va risolto prima, non si scarta da qui
discard-reason-untracked = File non tracciato: non si scarta (vorrebbe dire cancellarlo, senza backup)
discard-label-one = Scarto delle modifiche di { $path }
discard-label-many = Scarto delle modifiche di { $n } file
discard-check-label-one = Controllo per scarto delle modifiche di { $path }
discard-check-label-many = Controllo per scarto delle modifiche di { $n } file
discard-not-allowed = { $label } non possibile: { $error }
discard-done-one = Modifiche di { $path } scartate; backup in { $backup }. «Annulla» le rimette
discard-done-many = Modifiche di { $n } file scartate; backup in { $backup }. «Annulla» le rimette
discard-status-not-reread = Modifiche scartate, ma lo stato non si rilegge: { $reason }
discard-lines-label = Scarto di { $n ->
    [one] 1 riga
   *[other] { $n } righe
} di { $path }
discard-lines-label-hunk = Scarto dell'hunk { $hunk } di { $path }
discard-lines-check-label = Controllo prima di scartare righe di { $path }
discard-lines-done = { $n ->
    [one] 1 riga
   *[other] { $n } righe
} di { $path } scartate; backup del file in { $backup }. «Annulla» le riporta
discard-lines-done-hunk = Hunk { $hunk } di { $path } scartato; backup del file in { $backup }. «Annulla» lo riporta
