# Confirmations before writing: conflicts to mark resolved, discard, amend (view/confirm.rs).

confirm-resolve-title = { $n ->
    [one] Segnare il conflitto come risolto?
   *[other] Segnare i conflitti come risolti?
}
confirm-resolve-body = Mettere in stage un file in conflitto lo segna come risolto: nell'indice va il contenuto attuale del file nel worktree, così com'è, compresi eventuali marcatori di conflitto (<<<<<<<, =======, >>>>>>>) rimasti. Le versioni del conflitto (base, nostra e loro) escono dall'indice. Controllare prima che il file sia davvero risolto.
confirm-resolve-files = { $n ->
    [one] File in conflitto:
   *[other] { $n } file in conflitto:
}
confirm-and-more = e altri { $n }
confirm-resolve-others = { $n ->
    [one] Insieme a 1 altro file senza conflitti, messo in stage normalmente.
   *[other] Insieme a { $n } altri file senza conflitti, messi in stage normalmente.
}
confirm-resolve-button = { $n ->
    [one] Segna come risolto
   *[other] Segna come risolti
}
confirm-cancel = Annulla
confirm-discard-title = Scartare le modifiche?
confirm-discard-body = Le modifiche non in stage vengono tolte: ogni file torna alla versione in stage, o all'ultimo commit se non ha niente in stage. Le modifiche in stage e i file non tracciati non vengono toccati.
confirm-discard-backup = «Annulla» nella barra in alto rimette i file come sono adesso, finché non vengono modificati di nuovo.
confirm-discard-details = Dettagli
confirm-discard-details-body = Prima di scartare si salva un backup in refs/project/backup/discard-…; poi ogni file viene ripristinato dall'indice di git (l'area di stage).
confirm-discard-files = { $n ->
    [one] File:
   *[other] { $n } file:
}
confirm-discard-button = { $n ->
    [one] Scarta le modifiche
   *[other] Scarta le modifiche di tutti
}
confirm-discard-keep = Non scartare
confirm-amend-title = Riscrivere un commit già pubblicato?
confirm-amend-body = L'ultimo commit è già pubblicato o etichettato: lo contiene un branch remoto o un tag. Con amend viene sostituito da un commit nuovo: i branch remoti e i tag restano sul commit vecchio, il branch locale diverge da quello remoto e per pubblicare la modifica servirà un push forzato, che riscrive la storia anche per chi ha già scaricato il commit. Il commit attuale resta salvato in un ref di backup.
confirm-amend-reason = Motivo:
confirm-amend-left-behind = Restano sul commit vecchio anche:
confirm-amend-button = Riscrivi il commit
