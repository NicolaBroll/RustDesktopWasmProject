# Gestione dei remoti

Parte di [ARCHITETTURA-git.md](../ARCHITETTURA-git.md): principi (§1), regole per ogni chiamata a
git (§5), comandi lenti (§6) e diario (§7) sono lì; qui il contratto dei comandi, il `CliBackend` e
undo e backup di questa funzione. "§n" rimanda sempre al documento principale.

## Comandi

### `remote.list`

Parametri: `{}`.

Sola lettura. Risultato: `{"remotes": [{"name", "url": string | null, "push_urls": [string],
"fetch": [string], "mirror": bool, "branches": n, "default": bool, "push_default": bool,
"credentials": bool}], "default": string | null, "push_default": string | null}`, in ordine di nome
(`git remote`). `url`: il primo `remote.<r>.url` come scritto (senza `insteadOf`); `push_urls`: i
`pushurl`, o tutti gli `url` se più di uno; vuoto = il push va a `url`. `branches`: i branch remoti
`refs/remotes/<r>/...` (esclusi quelli di un remoto `<r>/...`, senza i simbolici); `default`: il
remoto di `remote.fetch` senza parametri (`default_remote`; `null` se ambiguo); `push_default`:
l'ultimo `remote.pushDefault`. **Credenziali mascherate** (`mask_url`, ovunque un URL esca dal
motore: `remote.list`, `remote.add`, `remote.remove`, la voce del diario in `edit.history`, i
messaggi d'errore sugli URL): `schema://utente:password@host` → `utente:***`; in `http(s)`/`ftp(s)`
anche un utente senza password diventa `***` (spesso è un token); un utente in `ssh://`/`git://` o
alla scp (`git@host:r`) resta; vale dentro un helper (`<x>::<url>`). `credentials`: l'URL ne aveva.
Ahead/behind dei branch rispetto all'upstream non sono qui: sono in `RefInfo::upstream`
(`repo.refs`, da `%(upstream:track)`), riletti nei `refs` di `remote.fetch`, `remote.pull` e
`remote.push`.

### `remote.add`

Parametri: `{"name", "url", "fetch"?}`.

Scrive (`writes`), entra nel diario (§7). Nome nuovo (`check_new_remote_name`: `check_remote_name` e
niente `-` iniziale) → `InvalidParams`; nome già configurato (`git remote`), voci `remote.<name>.*`
nel file del repo o branch remoti `refs/remotes/<name>/...` rimasti da prima (l'undo li
cancellerebbe) → `NotAllowed`. **Politica degli URL** (`check_remote_url`, prima di git): non vuoto,
senza caratteri di controllo né spazi in testa o in coda, non inizia con `-` (`--upload-pack=...`,
`-oProxyCommand=...`), niente trasporti `ext::`/`ext://`/`fd::`/`fd://` (eseguono comandi o usano
descrittori; anche dentro un altro helper e in qualsiasi maiuscolo: da qui mai, qualunque sia
`protocol.allow`), niente utente o host che inizia con `-` (`ssh://-x/r`, `ssh://-oX@h/r`,
`ssh://[-x]/r`, `u@-x:r`); gli altri helper `<x>::` e i percorsi locali vanno bene. Poi `git remote
add --end-of-options <name> <url>` (URL e refspec di default; un nome che git rifiuta →
`InvalidParams`, uno che esiste → `NotAllowed`). Un URL con credenziali si accetta con un avviso in
`warnings` (git lo salva in chiaro). `"fetch": true`: poi un fetch di tutto il remoto come
`remote.fetch` (lento, §6: avanzamento e annullamento dal `JobContext`); un fetch fallito o
annullato **non** annulla l'aggiunta: il remoto resta (ed è nel diario) e l'errore va in
`fetch_error` (`kind`: `auth`, `unreachable`, `cancelled`, `other`). Risultato: `{"remote", "url"
(mascherato), "credentials", "fetched", "fetch": {"updated", "messages", "warnings"} | null,
"fetch_error": {"kind", "message"} | null, "warnings", "remotes": [come remote.list], "refs",
...diario}`.

### `remote.remove`

Parametri: `{"name", "confirm"?}`.

Scrive (`writes`), entra nel diario (§7). `name`: un remoto configurato (`check_remote_name`, `-`
iniziale ammesso; altrimenti `NotAllowed` con l'elenco). Rifiutati (`NotAllowed`, niente cambia): un
remoto definito **anche solo in parte** fuori dal file del repo
(`GitBackend::remote_config_elsewhere`: configurazione globale o di sistema, `include`,
`config.worktree`, `.git/remotes/`; l'errore elenca chiave, ambito e file), perché ne resterebbe una
parte che git vede ancora; il remoto promisor di un clone parziale (`remote.<name>.promisor` vero o
`extensions.partialClone` = `<name>`: senza, gli oggetti mancanti non si scaricherebbero più e
checkout, log e diff fallirebbero); un'operazione in corso. Se dopo la rimozione `git remote` elenca
ancora il remoto (`ensure_remote_gone`), tutto si rimette subito com'era e il risultato è
`NotAllowed`; se il ripristino riesce solo in parte (es. un branch remoto ricreato nel frattempo: la
configurazione si riscrive comunque, con un avviso) o l'elenco dei remoti non si rilegge, la
rimozione entra comunque nel diario, così `edit.undo` e i backup restano disponibili (l'errore lo
dice). Chiede **sempre** `"confirm": true`: senza, `ConfirmRequired` che dice quanti branch remoti
spariscono e quali branch perdono l'upstream o il `pushRemote`, e se `remote.pushDefault` lo nomina;
niente cambia. Toglie (`remove_remote`, vedi "Undo e backup") i branch remoti `refs/remotes/<name>/...` (non quelli di
un remoto `<name>/...`; a differenza di `git remote remove`, i ref scritti fuori da
`refs/remotes/<name>/` da un refspec personalizzato restano), `refs/remotes/<name>/HEAD`, la sezione
`remote.<name>`, `branch.<b>.remote`/`merge` dei branch con quell'upstream, i
`branch.<b>.pushRemote` e `remote.pushDefault` che lo nominano (tutto del file del repo); i tag
scaricati restano, il remoto vero non cambia. Risultato: `{"remote", "removed": RemoteSnapshot (URL
mascherati), "upstreams_removed": [branch], "push_remotes_removed": [branch],
"push_default_removed": bool, "backups": [string], "warnings", "remotes", "refs", ...diario}`.

## CliBackend

**Remoti** (`remote.list`, `remote.add`, `remote.remove`, S27). Lettura di un remoto
(`remote_snapshot`): un solo `git config --local -z --get-regexp
'^(remote\.<nome>\.[^.]+|remote\.pushdefault|branch\..+\.(remote|merge|pushremote))$'`
(solo il file del repo, niente `include` né globale; un valore non UTF-8 → `NotAllowed`), `git
for-each-ref --format=%(refname)%00%(objectname)%00%(symref) refs/remotes/<nome>/` e `git
symbolic-ref -q --end-of-options refs/remotes/<nome>/HEAD` (un simbolico che punta a un ref
che non c'è non compare in `for-each-ref`). `remote.list` legge con `config_values` (tutte le
fonti, come `git remote`). Le voci fuori dal file del repo (`remote_config_elsewhere`): `git
config -z --show-scope --show-origin --includes --get-regexp '^remote\.<nome>\.[^.]+$'` meno
le provenienze di `git config -z --local --show-origin` (un file incluso ha ambito `local` ma
un'altra provenienza), più i file del formato vecchio `remotes/<nome>` e `branches/<nome>` (`git rev-parse --git-path`), che git legge come remoti. L'URL nello stderr di `git remote add` si sostituisce con quello
mascherato prima di farne l'errore. Fetch e push girano con `-c protocol.ext.allow=never -c
protocol.fd.allow=never`: un remoto `ext::`/`fd::` configurato a mano non esegue niente.
Aggiunta: `git remote add --end-of-options <nome> <url>`, mai
`-f`/`--mirror`/`-t`. **Rimozione senza `git remote remove`**: i branch remoti con la
transazione di `restore_refs` (valori attesi, backup), poi esattamente le voci salvate: `git
config --local --remove-section --end-of-options remote.<nome>`, `git config --local
--fixed-value --unset-all --end-of-options <chiave> <valore>` per upstream, `pushRemote` e
`remote.pushdefault` (codice 5, niente da togliere, va bene), `git symbolic-ref --delete
--end-of-options refs/remotes/<nome>/HEAD`. Così niente sparisce senza essere nel diario
(`git remote remove` sceglierebbe da sé i ref da togliere dai refspec, e toglierebbe
`remote.pushDefault` anche da altri file). Ripristino: `git config --local --add
--end-of-options <chiave> <valore>` per ogni voce e `git symbolic-ref -m <reflog>
refs/remotes/<nome>/HEAD <ref>`.

## Undo e backup

`remote.add` e `remote.remove` entrano nel diario con il remoto fotografato
(`JournalEntry::remote`, un `RemoteSnapshot`: configurazione `remote.<r>.*` del file del repo,
branch remoti nome → commit, dove punta `refs/remotes/<r>/HEAD`, le voci
`branch.<b>.remote`/`merge`/`pushRemote` che lo usano e `remote.pushDefault`; nel JSON del diario
gli URL sono mascherati, in memoria no). I `refs` della voce sono i branch remoti, solo da
mostrare. Due operazioni del motore, uguali per i backend:

- **`remove_remote`** (il comando, undo di `remote.add`, redo di `remote.remove`): rifiutato
  durante un'operazione in corso; il remoto deve essere **esattamente** come nella fotografia
  (`RemoteSnapshot::differences`, configurazione e upstream come insiemi), altrimenti
  `UndoConflict` (per il comando, che l'ha appena letta, `NotAllowed`) e niente cambia. Poi i
  branch remoti si tolgono con `restore_refs` (una transazione con i valori attesi; HEAD non si
  tocca) e i commit che non resterebbero raggiungibili da nessun altro ref (branch locali, tag,
  altri remoti, backup) vanno prima in un ref di backup per commit di punta,
  `refs/project/backup/remote-<s>-<ns>` (`RefRestore::backup_kind` di `remote.remove`; `undo-`
  e `redo-` per undo/redo): **scelta**: niente copia di tutti i branch remoti sotto un prefisso
  di backup, perché quasi sempre i loro commit sono anche in branch locali e il nome → commit
  è già nella voce del diario; basta che il gc non tolga quelli che erano solo lì. Infine la
  configurazione (`remove_remote_config`): sezione, `HEAD` simbolico e, solo per
  `remote.remove`, upstream, `pushRemote` e `remote.pushDefault` salvati; un problema qui va in
  `warnings` (i ref sono già tolti).
- **`restore_remote`** (undo di `remote.remove`, redo di `remote.add`): solo se del remoto non
  c'è niente (non è fra i remoti configurati, nessuna voce `remote.<r>.*`, nessun branch remoto),
  altrimenti `UndoConflict` (un remoto ricreato con lo stesso nome non si sovrascrive). Prima i
  branch remoti (`restore_refs`, creati solo se non esistono), poi la configurazione
  (`write_remote_config`). Per `remote.remove` anche gli upstream, ma **solo** per i branch che
  esistono ancora e non hanno nel frattempo un altro upstream (`GitBackend::upstream`) o un
  altro `pushRemote`, e `remote.pushDefault` solo se ora non c'è: gli altri restano come sono,
  con un avviso in `warnings`. La voce del diario si aggiorna con quello che è stato davvero
  rimesso, così il redo confronta e toglie solo quello (l'upstream reimpostato altrove resta).
  Nel confronto una chiave senza valore vale `true` (si riscrive così).

La fotografia nel JSON ha le credenziali mascherate in ogni valore (anche `remote.<r>.proxy`) e
per questo non si deserializza (`JournalEntry::remote` è `skip_deserializing`).

`remote.add` non tocca gli upstream: la sua voce li fotografa com'erano (di solito nessuno) e
l'undo è rifiutato se un branch nel frattempo ha preso il remoto come upstream (es. con il primo
`remote.push`), o se un fetch ha cambiato i branch remoti; l'undo non toglie mai i tag scaricati
(potrebbero essere arrivati anche da altri remoti). Un remoto aggiunto con branch remoti già
presenti sotto `refs/remotes/<nome>/` è rifiutato prima, perché l'undo li cancellerebbe.
