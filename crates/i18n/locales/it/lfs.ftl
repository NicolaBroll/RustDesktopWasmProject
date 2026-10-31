# Git LFS nella vista (S63).

# Il badge di un file salvato in Git LFS, nell'area di stage e nella barra del diff.
lfs-badge = LFS
lfs-badge-hint = Salvato in Git LFS: git tiene un piccolo file puntatore, il contenuto è nell'archivio LFS

# Nel diff di un file LFS, al posto del testo del puntatore; $size: la dimensione del file.
lfs-stored = Salvato in Git LFS ({ $size })
lfs-stored-unknown = Salvato in Git LFS
lfs-size-bytes = { $n ->
    [one] 1 byte
   *[other] { $n } byte
}
lfs-size-kb = { $n } KB
lfs-size-mb = { $n } MB
lfs-size-gb = { $n } GB

# L'avviso quando il repository usa Git LFS e git-lfs non è installato.
lfs-missing = Questo repository usa Git LFS, che non è installato: i file restano puntatori
lfs-missing-hint = I file salvati in Git LFS sono piccoli file puntatore nella working tree, non il loro contenuto. Installa git-lfs (https://git-lfs.com), esegui «git lfs install» e fai di nuovo il checkout per avere il contenuto; fare commit di un puntatore modificato a mano salverebbe quel testo.
lfs-missing-dismiss = Chiudi
lfs-missing-dismiss-hint = Chiudi l'avviso: per questo repository non viene più mostrato fino al riavvio dell'app
