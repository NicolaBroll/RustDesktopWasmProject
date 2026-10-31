# Ignorare i file non tracciati (ignore.rs e il sottomenu "Ignora" dell'area di stage).

ignore-this-file = Questo file
ignore-extension = Tutti i file { $pattern }
ignore-folder = Cartella { $folder }/
ignore-entry-help = Aggiunge la riga «{ $pattern }» a { $file }
ignore-label = Aggiunta di «{ $pattern }» a { $file }
ignore-not-allowed = { $label } non possibile: { $error }
ignore-done = «{ $pattern }» aggiunto a { $file }; { $n ->
    [0] nessun file non tracciato corrisponde
    [one] 1 file non tracciato ora è ignorato
   *[other] { $n } file non tracciati ora sono ignorati
}. «Annulla» lo rimuove
ignore-done-already = «{ $pattern }» è già in { $file }: nessuna modifica
ignore-status-not-reread = Pattern aggiunto, ma lo stato non si può rileggere: { $reason }
