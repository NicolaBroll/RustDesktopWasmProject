reset-menu-item = Reset di «{ $branch }» a qui…
reset-menu-item-detached = Reset di HEAD a qui…
reset-menu-hint = Sposta il branch attivo (o la HEAD staccata) su { $id }: scegli soft, mixed o hard e guarda cosa cambia prima di confermare
reset-menu-unborn = Il branch «{ $branch }» non ha ancora commit: non c'è niente da resettare
reset-menu-operation = È in corso un'operazione (merge, rebase, cherry-pick, revert o bisect): prima finiscila o annullala
reset-branch-name = «{ $branch }»
reset-detached-name = HEAD (staccata)
reset-title = Reset di { $moving } a { $what }
reset-mode-soft-hint = Sposta solo il branch: le modifiche dei commit che lo lasciano restano nello stage, i file non vengono toccati.
reset-mode-mixed-hint = Sposta il branch e lo stage: quelle modifiche restano nei file, fuori dallo stage. Nei file non si perde niente.
reset-mode-hard-hint = Sposta il branch, lo stage e i file tracciati: i file diventano quelli del commit. Le modifiche locali vengono sovrascritte (prima si salva un backup).
reset-loading = Controllo di cosa farebbe questo reset…
reset-not-possible = Questo reset non è possibile: { $error }
reset-plan-move = { $moving } passa da { $before } a { $after }.
reset-plan-same = { $moving } è già su { $id }: si azzerano solo lo stage e i file.
reset-hard-clean = Non ci sono modifiche locali ai file tracciati: non si sovrascrive niente.
reset-hard-files = { $n ->
    [one] Vengono sovrascritte le modifiche locali di 1 file:
   *[other] Vengono sovrascritte le modifiche locali di { $n } file:
}
reset-hard-backup = Prima vengono salvate in un backup (refs/project/backup/reset-...): «git stash apply» le rimette. Annulla riporta indietro il branch ma non le ripristina.
reset-hard-untracked = I file non tracciati non vengono toccati.
reset-warning = Attenzione: { $warning }
reset-button = Reset ({ $mode })
reset-cancel = Annulla
reset-sending = Reset in corso…
reset-label = Reset ({ $mode }) di { $moving } a { $id }
reset-refused = Il reset è stato rifiutato, non è cambiato niente: { $error }
reset-done = Reset ({ $mode }) di { $moving } a { $id }
reset-done-backup = ; le modifiche locali di { $n ->
    [one] 1 file sono salvate
   *[other] { $n } file sono salvate
} in { $backup } (git stash apply { $stash })
reset-done-index-backup = ; lo stage precedente è salvato in { $backup }
reset-done-commit-backups = ; i commit non più raggiungibili sono salvati in { $refs }
reset-done-short = Reset fatto
