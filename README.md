# Project

Client git grafico in costruzione (vedi [impl-docs/ROADMAP-git.md](impl-docs/ROADMAP-git.md)),
scritto in Rust con architettura a strati: motore indipendente dalla UI,
un unico frontend egui compilato per desktop e per WebAssembly, CLI e server MCP che usano lo stesso
catalogo di comandi. Dettagli in [impl-docs/ARCHITETTURA-rust.md](impl-docs/ARCHITETTURA-rust.md)
e [AGENTS.md](AGENTS.md).

## Avvio

```sh
make setup        # target wasm32 e trunk
make run          # app desktop
make web-serve    # app web su http://localhost:8080
make cli ARGS="commands --search repo"
```

## CLI

```sh
project-cli commands [--search <testo>]
project-cli [--repo <percorso>] run --cmd <id>[=<json>]...   # es. --repo . run --cmd repo.status
project-cli [--repo <percorso>] eval                         # comandi da stdin, uno per riga
project-cli mcp [--root <dir>]                               # server MCP su stdin/stdout
```

`--repo` apre il repository prima dei comandi (equivale a `repo.open` come primo comando).

Comandi: `repo.open {"path"}`, `repo.status`, `repo.refs`, `repo.log {"skip"?, "limit"?, "rev"?, "all"?, "stashes"?}`
(pagine da 200 commit se manca `limit`, con `has_more`; da HEAD, da `rev`, oppure con
`"all": true` da tutti i branch locali e remoti, i tag e HEAD, senza stash né backup, e con in
più `"stashes": true` anche dai commit su cui sono stati creati gli stash; `rev` e
`all` insieme, o `stashes` senza `all`, sono un errore), `commit.show {"id"}` (metadati e file
cambiati rispetto al primo genitore), `diff.worktree {"path"?}` (modifiche non in stage),
`diff.index {"path"?}` (modifiche in stage) e `diff.commit {"id", "path"?}` (commit rispetto al
primo genitore). I diff restituiscono `{"files": [...]}` con hunk e righe (in `diff.worktree` e
`diff.index` ogni file ha anche `digest`, da passare a `index.*_lines`); `path` limita il
risultato a quel file (le rinomine restano tali) ed è una stringa, o un array di byte per i nomi
non UTF-8. Non c'è un limite di dimensione: per i commit grandi conviene chiedere un file alla
volta.

Scritture (segnate `[modifica]` da `commands`): `index.stage {"paths"}` mette in stage i file
(modifiche, file nuovi, cancellazioni; per una rinomina i due percorsi) e `index.unstage {"paths"}`
li toglie dallo stage senza toccare il worktree. `paths` è una lista non vuota di percorsi esatti
(stringhe o array di byte), presi alla lettera; una cartella comprende i file che contiene. Un file
ignorato da `.gitignore` non viene mai forzato: errore, e niente cambia. Mettere in stage un file
in conflitto (chiesto per nome) lo risolve; toglierlo dallo stage è rifiutato. Entrambi
restituiscono `{"status", "backup"}`: il nuovo stato, come `repo.status`, e il ref del backup
dell'indice (o `null`). Il backup si salva da solo prima di scrivere quando una versione in stage
non è né in HEAD né nel worktree e andrebbe persa (`git show <ref>:<percorso>` la recupera); se
non riesce, l'indice non si tocca.

`index.stage_lines {"path", "lines", "digest"}` e `index.unstage_lines {"path", "lines",
"digest"}` mettono in stage o tolgono dallo stage solo alcune righe di un file (solo l'indice,
mai il worktree): `lines` è una lista non vuota di `{"old": n}` (riga tolta) o
`{"new": n}` (riga aggiunta) con i numeri di riga di `diff.worktree` (stage) o `diff.index`
(unstage) per `path`, e `digest` è quello del file nello stesso diff. Se il file è cambiato da
quando il diff è stato letto il comando fallisce senza scrivere niente (va riletto il diff).
Risultato e backup come `index.stage`. File binari, sottomoduli, link simbolici e conflitti si
mettono in stage solo interi.

`commit.create {"message", "amend"?, "confirm"?}` crea un commit con quello che è in stage
(`git commit -F -`: messaggio via stdin, hook e firma secondo la configurazione dell'utente, mai
un editor). Messaggio vuoto, niente in stage, file in conflitto, rebase/cherry-pick/revert in
corso: errore e niente cambia; durante un merge il commit è il commit di merge. Con `"amend":
true` riscrive l'ultimo commit (stesso autore e genitori) e salva il commit riscritto in un ref
`refs/project/backup/amend-...`; se quel commit è già su un branch remoto o ha un tag serve
anche `"confirm": true`, e gli altri branch e tag che lo contengono sono elencati in `also_on`.
Un hook che rifiuta o una firma che non riesce danno un errore con tutto il
loro output; un problema dopo un commit riuscito (es. un hook `post-commit` che sposta HEAD) dà
un errore che dice di non ripetere il commit e nomina il backup. Risultato: `{"commit",
"previous", "branch", "backup", "also_on", "status"}` (il commit come in `repo.log`, HEAD prima,
il branch spostato, il backup o `null`, i ref rimasti sul commit riscritto, il nuovo stato).

`worktree.discard {"paths", "confirm"}` scarta le modifiche non in stage dei file `paths` (file o
cartelle, presi alla lettera): il worktree torna come l'indice, l'indice non si tocca (un file
`MM` resta con la sua modifica in stage). Senza `"confirm": true` dà un errore che elenca i file
che si perderebbero, e niente cambia. Prima di scrivere salva il worktree con `git stash create`
in un ref `refs/project/backup/discard-...` (`git show <ref>:<percorso>` lo recupera; se il
backup non riesce niente si scarta) e registra lo scarto nel diario: `edit.undo` rimette i file
identici, solo se nel frattempo non sono cambiati. I file non tracciati non si toccano mai
(chiesti per nome: errore; il backup non li coprirebbe); rifiutati anche i sottomoduli chiesti per
nome e, perché `git stash create` non salverebbe il backup, un branch senza commit e un repo con
file in conflitto o `intent-to-add` (`git add -N`). Rifiutato anche se al posto di una cartella del
percorso di un file c'è un file o un link (non tracciato o ignorato, che `git restore`
cancellerebbe). Se i byte di un file non sono quelli che git ricreerebbe (es. CRLF con
`core.autocrlf=input`) il backup li salva così come sono, e l'undo li rimette identici. Undo e
redo di uno scarto sono rifiutati anche se HEAD è cambiata. Risultato: `{"files": [{"path", "change",
"mode", "oid"}], "backup", "stash", "status", "can_undo", "can_redo", "next_undo",
"next_redo"}`.

`branch.create {"name", "start"?}` crea il branch `name` (nome breve) su `start` (una rev; HEAD se
manca) senza cambiare branch; un branch esistente non si sovrascrive mai, e un nome nuovo che git
non accetta (anche uno che inizia con `-`) è un errore. Risultato: `{"branch", "target", "refs"}`
più lo stato del diario. `branch.checkout {"name"}` passa al branch con `git switch`, mai forzato:
se una modifica locale, un file non tracciato o un ignorato verrebbero sovrascritti dà un errore
che elenca i file, e niente cambia; le altre modifiche restano. Rifiutato durante merge, rebase,
ecc. e con file in conflitto. Risultato: `{"branch", "previous", "changed", "warnings",
"status"}`. `branch.delete {"name", "confirm"?, "expected"?}` elimina un branch (con `expected`, l'id completo del
commit visto, rifiuta senza toccare niente se il branch nel frattempo punta altrove): il branch attivo o aperto in un
altro worktree è rifiutato; se ha commit che non sono né in HEAD né nel suo upstream serve
`"confirm": true`. Prima salva sempre il commit in `refs/project/backup/delete-...`; come `git
branch -d` toglie anche la configurazione `branch.<nome>.*` (riportata in `removed_config` e
salvata per l'undo). Risultato: `{"branch", "target", "unmerged", "backup", "removed_config",
"warnings", "refs"}`. `branch.rename {"name", "new_name"}` rinomina con `git
branch -m` (mai sopra un branch esistente): reflog, upstream e HEAD seguono. Risultato:
`{"branch", "previous", "target", "head", "warnings", "refs"}`. Tutti e quattro entrano nel
diario: l'undo elimina il branch creato, ricrea quello eliminato sullo stesso commit con la sua
configurazione (se nel frattempo non ne è comparsa una nuova; il reflog del branch eliminato non
torna), rinomina all'indietro e torna al branch (o commit) di prima con un altro checkout
sicuro; sempre solo se nel frattempo nessuno ha toccato quei ref o HEAD.

`remote.fetch {"remote"?, "branch"?}` scarica dal remoto (`git fetch --progress`): commit nuovi,
branch remoti e tag come dice la configurazione del remoto (anche `fetch.prune`), mai i branch
locali, l'indice o il worktree. `remote` è il nome di un remoto configurato (mai un URL o un
percorso; un nome che non c'è dà un errore con l'elenco dei remoti); se manca, quello
dell'upstream del branch attivo, poi `origin`, poi l'unico remoto. Con `branch` scarica solo quel
branch del remoto. Non chiede mai niente da tastiera (niente terminale, stdin chiuso): credenziali
rifiutate o mancanti danno un errore di autenticazione, un remoto che non si raggiunge (es. URL
verso una cartella che non c'è) un errore a parte. Risultato: `{"remote", "branch", "updated":
[{"name", "before", "after"}], "messages", "warnings", "refs"}` (i ref cambiati, le righe di git,
il nuovo elenco dei ref). Non entra nel diario. Nella CLI, su un terminale, l'avanzamento va su
stderr; la UI lo mostra mentre il fetch gira e può annullarlo.

`remote.pull {"remote"?, "branch"?, "strategy"?}` scarica il branch dell'upstream (come
`remote.fetch`, annullabile) e lo integra nel branch `branch` (HEAD se manca): fast-forward, merge
o rebase secondo `branch.<nome>.rebase`/`pull.rebase` e `pull.ff` (`strategy` = `"merge"`,
`"rebase"` o `"ff_only"` li sostituisce; senza configurazione merge). Un branch che non è quello
attivo si può solo portare avanti, senza toccare il worktree. Mai `--autostash`: merge e rebase
vogliono il worktree pulito (file tracciati), il fast-forward rifiuta solo se sovrascriverebbe
modifiche locali o file non tracciati. Rifiutato (e niente scaricato) con HEAD staccata, un branch
senza upstream (l'errore dice come impostarlo, o di pubblicarlo con `remote.push`), un merge o
rebase in corso, file in conflitto, `pull.rebase=interactive`; rifiutato anche se il pull
porterebbe un file che esiste già nel worktree come non tracciato o ignorato (git lo
sovrascriverebbe). Prima di scrivere salva il branch in
`refs/project/backup/pull-...`. Se il merge o il rebase si ferma in conflitto viene annullato:
il branch resta com'era e l'errore elenca i file in conflitto (per risolverli, per ora, git da
terminale). Entra nel diario: `edit.undo` riporta il branch al commit di prima con indice e
worktree (come `git reset --keep`: rifiutato se un file da ripristinare ha modifiche locali o se
un file da ricreare c'è già). Risultato: `{"branch", "remote", "upstream", "strategy":
"fast_forward"|"merge"|"rebase"|"up_to_date", "strategy_source", "before", "after", "ahead",
"behind", "backup", "fetch": {"updated", "messages"}, "warnings", "status", "refs"}` più lo stato
del diario.

`remote.push {"remote"?, "branch"?, "force"?, "confirm"?, "expected"?}` pubblica il branch (HEAD se manca)
con un refspec esplicito `refs/heads/<b>:refs/heads/<b>`: mai tag o altri branch, qualunque sia
`push.default`, `remote.<nome>.push` o `push.followTags`. Il remoto si sceglie come `git push`:
`remote`, poi `branch.<nome>.pushRemote`, `remote.pushDefault`, il remoto dell'upstream, `origin`
o l'unico remoto (`remote_source` nel risultato); un remoto mirror è rifiutato. Sul remoto
dell'upstream va al branch dell'upstream, altrove allo stesso nome; al primo push (nessun
upstream) imposta l'upstream. Un
upstream con un altro nome è rifiutato (come `push.default=simple`). Un push non fast-forward è
rifiutato (errore che suggerisce il pull) e niente cambia. `"force": true` usa solo
`--force-with-lease` con l'id del branch remoto locale letto prima (mai `--force`); se il remoto
perderebbe commit serve anche `"confirm": true` con `"expected"`, l'id del branch remoto locale
che l'errore di conferma mostra (senza, l'errore li elenca; se nel frattempo è cambiato si chiede
di nuovo la conferma e niente si pubblica) e prima si salvano
in `refs/project/backup/push-...`; se il remoto si è spostato nel frattempo il push è rifiutato.
Un hook `pre-push` che rifiuta dà un errore con il suo output. Non entra nel diario (un push non
si annulla). Risultato: `{"remote", "remote_source", "branch", "remote_branch", "kind":
"new"|"fast_forward"|"forced"|"up_to_date", "summary", "before", "after", "set_upstream",
"forced", "lost", "backup", "updated", "messages", "warnings", "status", "refs"}`.

`remote.list {}` (sola lettura) elenca i remoti in ordine di nome: `{"remotes": [{"name", "url",
"push_urls", "fetch", "mirror", "branches", "default", "push_default", "credentials"}],
"default", "push_default"}` (`push_urls` vuoto = il push va a `url`; `default` è il remoto che
`remote.fetch` usa senza parametri). **Le credenziali negli URL non si mostrano mai**:
`https://utente:token@host/r` diventa `https://utente:***@host/r`, `https://token@github.com/r`
diventa `https://***@github.com/r` (in `http(s)`/`ftp(s)` l'utente da solo è spesso un token),
`credentials: true` dice che c'erano; un utente ssh (`git@host:r`) resta. Vale anche per i
risultati di `remote.add`/`remote.remove` e per il diario (`edit.history`).

`remote.add {"name", "url", "fetch"?}` aggiunge un remoto (`git remote add --end-of-options`,
refspec di default; `"fetch": true` poi scarica come `remote.fetch`, con avanzamento e
annullamento: un fetch fallito non annulla l'aggiunta e finisce in `fetch_error`). Rifiutati
prima di chiamare git: nomi che iniziano con `-` o non validi, URL vuoti, con caratteri di
controllo, che iniziano con `-` (`--upload-pack=...`), con i trasporti `ext::`/`fd::` (eseguono
comandi) o con un host che inizia con `-` (`ssh://-oProxyCommand=...`); un nome già usato o con
branch remoti rimasti da prima. Un URL con credenziali è accettato con un avviso (git lo salva in
chiaro). Risultato: `{"remote", "url", "credentials", "fetched", "fetch", "fetch_error",
"warnings", "remotes", "refs"}`. Entra nel diario: l'undo toglie configurazione e branch remoti
scaricati solo se sono ancora com'erano (e nessun branch ha preso il remoto come upstream).

`remote.remove {"name", "confirm": true}` rimuove un remoto configurato nel file del repo
(rifiutato se è definito anche solo in parte altrove, globale o `include`, o se è il remoto
promisor di un clone parziale): la
sezione `remote.<nome>`, i branch remoti `refs/remotes/<nome>/...` (con `HEAD`), l'upstream dei
branch che lo usano, i loro `pushRemote` e `remote.pushDefault` se lo nominano. Senza `confirm`
l'errore elenca i branch che perderebbero l'upstream e il numero di branch remoti, e niente
cambia. Prima di scrivere tutto si salva nel diario e i commit che restavano solo nei branch
remoti vanno in `refs/project/backup/remote-...`. Risultato: `{"remote", "removed",
"upstreams_removed", "push_remotes_removed", "push_default_removed", "backups", "warnings",
"remotes", "refs"}`. L'undo rimette configurazione, branch remoti e upstream, solo se nessuno ha
ricreato un remoto con quel nome; un upstream reimpostato nel frattempo resta, con un avviso.
Ahead/behind dei branch rispetto all'upstream sono in `repo.refs` (`upstream.ahead`/`behind`) e
nei `refs` dei risultati di fetch, pull e push.

`stash.list {}` (sola lettura) elenca gli stash dal più recente: `{"stashes": [{"index",
"name": "stash@{n}", "id", "base", "message", "branch", "time", "untracked"}]}` (`base` è il
commit su cui lo stash è stato creato, `untracked` dice se ha anche file non tracciati; il
contenuto si legge con `commit.show`/`diff.commit` sull'`id`). `stash.push {"message"?,
"include_untracked"?, "keep_index"?, "paths"?}` mette da parte le modifiche (`git stash push`;
mai gli ignorati; un messaggio di una riga; rifiutato con un merge o un rebase in corso, file in
conflitto o `intent-to-add`, file `--assume-unchanged` fra quelli scelti, di cui git perderebbe
le modifiche, o niente da mettere da parte) e restituisce `{"stash", "stashes",
"status"}`; entra nel diario: l'undo riapplica lo stash con la divisione fra in stage e no e lo
toglie dall'elenco (solo se HEAD è ancora sullo stesso commit), il redo lo rifà. `stash.apply
{"index"?, "expected"?, "restore_index"?}` applica lo stash `stash@{index}` (0 se manca;
`expected`, l'id completo visto, protegge da un elenco cambiato nel frattempo) senza toglierlo,
`stash.pop` con gli stessi parametri lo toglie anche, dopo averne salvato il commit in
`refs/project/backup/stash-...`. Tutti e due sono rifiutati, senza cambiare niente, se lo stash
toccherebbe file con modifiche locali, non tracciati o ignorati (`git stash apply` in questi casi
scriverebbe a metà; un file in stage che ha già la versione dello stash, come dopo un push con
`keep_index`, non conta); se il merge con HEAD va in conflitto i file restano in conflitto come li
lascia git (`conflicts`) e `pop` tiene lo stash. Risultato: `{"stash", "conflicts", "dropped",
"backup", "warnings", "stashes", "status"}`; non entrano nel diario. Se dopo l'apply il drop di
`pop` non riesce, il pop non è un errore: `dropped: false` e un avviso (lo stash va eliminato
con `stash.drop`, il pop non va ripetuto). `stash.drop {"index"?, "expected", "confirm": true}`
elimina uno stash (senza `confirm` l'errore dice cosa si perderebbe e dà l'id da ripetere in
`expected`; `confirm` senza `expected` è un errore), con il backup in `refs/project/backup/stash-...`: entra nel diario e l'undo lo
rimette in cima all'elenco. Risultato: `{"stash", "backup", "warnings", "stashes"}`.

`edit.undo {}` annulla l'ultima operazione del diario (oggi `commit.create`, anche con `amend`,
`branch.*`, `worktree.discard`, `remote.pull`, `remote.add`, `remote.remove`, `stash.push` e
`stash.drop`) e `edit.redo {}` la rifà; `edit.history {}` (sola lettura) elenca il diario. L'undo
riporta il branch (o HEAD staccata) al valore di prima solo se nessuno l'ha spostato nel
frattempo (un `git commit` o `git reset` da terminale): altrimenti errore e niente cambia.
Per un commit indice e worktree non si toccano, come `git reset --soft`: dopo l'undo di un
commit le sue modifiche sono di nuovo in stage. L'undo di uno scarto rimette i file del backup
(`git restore --source=<stash> --worktree`) solo se sono ancora come li ha lasciati lo scarto e
con la stessa versione in stage; il redo li scarta di nuovo solo se sono ancora come li ha
lasciati l'undo. Il commit annullato, se non è più raggiungibile, resta in un
ref `refs/project/backup/undo-...`. Stage e unstage non entrano nel diario. Il diario è in
memoria e vale per il repo aperto: nella CLI più `--cmd` della stessa invocazione lo
condividono, invocazioni diverse no (`edit.undo` da solo dice che non c'è niente da annullare).
Risultato di undo e redo: `{"undone"|"redone": voce, "backups", "warnings", "status",
"can_undo", "can_redo", "next_undo", "next_redo"}` (`warnings`, di solito vuoto, segnala ad
esempio HEAD cambiata da un altro processo proprio durante l'undo). Durante un merge, rebase,
cherry-pick, revert, bisect o `git am` undo e redo sono rifiutati.

Il server MCP espone `list_commands`, `execute_command` e `open_repo`; `list_commands` riporta
`writes: true` per i comandi che modificano il repository. All'avvio apre il
repository di `--root` (default: cartella corrente), se c'è. Tutti i percorsi sono confinati in
`--root`, anche la ricerca dei repository (git non risale sopra la radice); un repository si
apre solo con `open_repo`, non con `repo.open`.

## Qualità

`cargo xtask ci` esegue formattazione, Clippy con warning come errori, test, controllo del
layering e Clippy per `wasm32-unknown-unknown`. La CI di GitHub usa lo stesso comando.
