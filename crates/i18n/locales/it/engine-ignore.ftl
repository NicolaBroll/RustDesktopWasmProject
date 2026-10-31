engine-ignore-no-patterns = nessun pattern: niente da ignorare
engine-ignore-pattern-empty = un pattern vuoto
engine-ignore-pattern-blank = un pattern di soli spazi
engine-ignore-pattern-line-break = il pattern `{ $pattern }` contiene un a capo o un byte NUL: un pattern per riga
engine-ignore-pattern-comment = il pattern `{ $pattern }` inizia con `#`, che git legge come commento: scriverlo come `\#`
engine-ignore-conflicted = `.gitignore` ha un conflitto: risolverlo prima
engine-ignore-not-checked-out = `.gitignore` è nel repository ma non nella cartella di lavoro (sparse checkout o `--skip-worktree`): niente scritto, ripristinarlo prima
engine-ignore-not-a-file = `{ $file }` non è un file normale (un link, una cartella o un file speciale): niente scritto, modificarlo fuori dall'app
engine-ignore-dir-not-real = la cartella di `{ $file }` non è una vera cartella (un link o un file): niente scritto
engine-ignore-unreadable = `{ $file }` non si può leggere: { $error }
engine-ignore-changed = `{ $file }` è cambiato mentre si aggiungevano i pattern: niente scritto, riprovare
engine-ignore-backup-incomplete = il backup (`{ $backup }`) non ha i byte di `{ $file }`: niente scritto, riprovare
engine-ignore-write-failed = `{ $file }` non si è potuto scrivere ({ $error }): il file è come prima; il backup è in `{ $backup }`
engine-ignore-not-as-expected = dopo `{ $command }` `{ $file }` non ha i byte scritti (un altro programma l'ha cambiato?); i byte sono in `{ $backup }`
engine-ignore-file-changed-undo = `{ $file }` è cambiato dopo l'operazione: niente ripristinato
engine-ignore-tracked = { $n ->
    [one] 1 file tracciato corrisponde ai nuovi pattern
   *[other] { $n } file tracciati corrispondono ai nuovi pattern
}: git { $n ->
    [one] continua a tracciarlo
   *[other] continua a tracciarli
} (`git rm --cached` lo fermerebbe): { $files }
