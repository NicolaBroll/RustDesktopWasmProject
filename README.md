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

`edit.undo {}` annulla l'ultima operazione che ha spostato ref (oggi `commit.create`, anche con
`amend`) e `edit.redo {}` la rifà; `edit.history {}` (sola lettura) elenca il diario. L'undo
riporta il branch (o HEAD staccata) al valore di prima solo se nessuno l'ha spostato nel
frattempo (un `git commit` o `git reset` da terminale): altrimenti errore e niente cambia.
Indice e worktree non si toccano, come `git reset --soft`: dopo l'undo di un commit le sue
modifiche sono di nuovo in stage. Il commit annullato, se non è più raggiungibile, resta in un
ref `refs/project/backup/undo-...`. Stage e unstage non entrano nel diario. Il diario è in
memoria e vale per il repo aperto: nella CLI più `--cmd` della stessa invocazione lo
condividono, invocazioni diverse no (`edit.undo` da solo dice che non c'è niente da annullare).
Risultato di undo e redo: `{"undone"|"redone": voce, "backups", "status", "can_undo",
"can_redo", "next_undo", "next_redo"}`.

Il server MCP espone `list_commands`, `execute_command` e `open_repo`; `list_commands` riporta
`writes: true` per i comandi che modificano il repository. All'avvio apre il
repository di `--root` (default: cartella corrente), se c'è. Tutti i percorsi sono confinati in
`--root`, anche la ricerca dei repository (git non risale sopra la radice); un repository si
apre solo con `open_repo`, non con `repo.open`.

## Qualità

`cargo xtask ci` esegue formattazione, Clippy con warning come errori, test, controllo del
layering e Clippy per `wasm32-unknown-unknown`. La CI di GitHub usa lo stesso comando.
