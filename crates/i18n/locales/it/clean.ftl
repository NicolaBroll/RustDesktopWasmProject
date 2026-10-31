# Deleting untracked files (clean.rs and its confirmation dialog).

clean-label-one = Eliminazione di { $path }
clean-label-many = Eliminazione di { $n } file non tracciati
clean-check-label-one = Controllo per eliminazione di { $path }
clean-check-label-many = Controllo per eliminazione di { $n } file non tracciati
clean-not-allowed = { $label } non possibile: { $error }
clean-done-one = { $path } eliminato; backup in { $backup }. «Annulla» lo rimette
clean-done-many = { $n ->
    [one] 1 file non tracciato eliminato
   *[other] { $n } file non tracciati eliminati
}; backup in { $backup }. «Annulla» li rimette
clean-status-not-reread = File eliminati, ma lo stato non si rilegge: { $reason }
clean-size-bytes = { $n } byte
clean-size-kb = { $n } KB
clean-size-mb = { $n } MB
clean-size-gb = { $n } GB
clean-confirm-title = { $n ->
    [one] Eliminare il file non tracciato?
   *[other] Eliminare { $n } file non tracciati?
}
clean-confirm-body = { $n ->
    [one] Il file non è in git: viene tolto dal disco, e anche le cartelle che restano vuote.
   *[other] I file non sono in git: vengono tolti dal disco, e anche le cartelle che restano vuote.
}
clean-confirm-backup = Si tiene un backup con il loro contenuto esatto in refs/project/backup/clean-…: «Annulla» nella barra in alto li rimette, finché non si crea niente allo stesso percorso.
clean-confirm-files = { $n ->
    [one] 1 file:
   *[other] { $n } file:
}
clean-confirm-files-size = { $n ->
    [one] 1 file, { $size }:
   *[other] { $n } file, { $size } in tutto:
}
clean-confirm-button = { $n ->
    [one] Elimina 1 file
   *[other] Elimina { $n } file
}
