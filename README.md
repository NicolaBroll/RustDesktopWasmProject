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

Comandi: `repo.open {"path"}`, `repo.status`, `repo.refs`, `repo.log {"skip"?, "limit"?, "rev"?}`
(pagine da 200 commit se manca `limit`, con `has_more`), `commit.show {"id"}` (metadati e file
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

`edit.undo {}` annulla l'ultima operazione del diario (oggi `commit.create`, anche con `amend`,
`branch.*` e `worktree.discard`) e `edit.redo {}` la rifà; `edit.history {}` (sola lettura) elenca il diario. L'undo
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
