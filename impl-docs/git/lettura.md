# Letture: repo, log e diff

Parte di [ARCHITETTURA-git.md](../ARCHITETTURA-git.md): principi (§1), regole per ogni chiamata a
git (§5), comandi lenti (§6) e diario (§7) sono lì; qui il contratto dei comandi, il `CliBackend` e
undo e backup di questa funzione. "§n" rimanda sempre al documento principale.

## Comandi

### `repo.open`

Parametri: `{"path"}`.

Usa il `RepoOpener` iniettato.

### `repo.status`

Parametri: `{}`.

### `repo.refs`.

Parametri: `{}`.

### `repo.log`.

Parametri: `{"skip"?, "limit"?, "rev"?, "all"?, "stashes"?}`.

Paginato (`limit` 200 se manca, risultato `{"commits", "has_more"}`). `git log --date-order` (figli
prima dei genitori, poi il più recente per data del committer; vale anche con più punti di partenza,
e le corsie del grafo §3 gestiscono più punte e più radici). Da HEAD, oppure da `rev` (ref, `HEAD` o
id, dopo `--end-of-options`); HEAD senza commit dà un log vuoto. `"all": true`: da tutti i branch
locali, i branch remoti, i tag e HEAD (`--branches --remotes --tags` e l'id di HEAD, così una HEAD
staccata compare; una HEAD senza commit si salta): mostra i commit dei branch non uniti e quelli
appena scaricati su `origin/*`. Restano fuori `refs/stash`, i backup in `refs/project/backup/` e
ogni altro ref fuori da `refs/heads/`, `refs/remotes/`, `refs/tags/` (es. le note); un tag su un
albero o un blob si ignora. `"stashes": true` (solo con `all`, altrimenti `InvalidParams`): parte
anche dalle basi degli stash (`StashEntry::base`, il primo genitore), così il commit a cui la UI
aggancia uno stash è sempre nel grafo, anche se nessun branch lo contiene più; i commit degli stash
(e quelli dell'indice e dei non tracciati che hanno per genitori) restano fuori, l'elenco è
`stash.list`. `rev` e `all` insieme → `InvalidParams`. `skip`/`limit` paginano nello stesso ordine,
quindi le pagine sono contigue.

### `commit.show`

Parametri: `{"id"}`.

Metadati e file cambiati rispetto al primo genitore.

### `diff.worktree` / `diff.index` / `diff.commit`

Parametri: `{"path"?}` / `{"path"?}` / `{"id", "path"?}`.

worktree rispetto all'indice (senza i non tracciati), indice rispetto a HEAD (albero vuoto se non ci
sono commit), commit rispetto al primo genitore (albero vuoto per un commit radice, come
`commit.show`). Risultato: il `Diff` (`{"files": [...]}`). `path` (stringa o array di byte, come
`RepoPath`) limita al file con quel percorso esatto, vecchio o nuovo: una rinomina resta tale (il
backend cerca prima l'altro percorso con `--diff-filter=R`) e i percorsi vanno a git dopo `--` con
`--literal-pathspecs`. Nessun limite di dimensione
