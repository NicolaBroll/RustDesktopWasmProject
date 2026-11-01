# Ricerca nella cronologia (search.rs, view/commits.rs).

search-hint = Cerca in messaggi, autori, id (Ctrl+F)
search-searching = Ricerca in corso…
search-no-matches = Nessun risultato
search-position = { $n } di { $total }
search-count = { $total ->
    [one] 1 risultato
   *[other] { $total } risultati
}
search-failed = Ricerca non riuscita
search-previous = Risultato precedente (Maiusc+Invio)
search-next = Risultato successivo (Invio)
search-close = Chiudi la ricerca (Esc)
search-hint-content = Testo aggiunto o rimosso dai commit, Invio per cercare (lento)
search-content = Contenuto
search-content-tip = Cerca in ciò che i commit hanno cambiato invece che in messaggi, autori e id (più lento: premi Invio per cercare)
search-regex = Regex
search-regex-tip = Il testo è un'espressione regolare (POSIX estesa) confrontata con le righe aggiunte e rimosse
search-ignore-case-tip = Ignora maiuscole e minuscole
search-press-enter = Premi Invio per cercare
search-stop = Ferma
search-stop-tip = Ferma la ricerca nel contenuto
search-stopped = Interrotta
search-found = { $count ->
    [one] 1 trovato
   *[other] { $count } trovati
}
search-found-more = { $count }+ trovati
search-position-more = { $n } di { $total }+
