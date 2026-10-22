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
cambiati rispetto al primo genitore).

Il server MCP espone `list_commands`, `execute_command` e `open_repo`. All'avvio apre il
repository di `--root` (default: cartella corrente), se c'è. Tutti i percorsi sono confinati in
`--root`, anche la ricerca dei repository (git non risale sopra la radice); un repository si
apre solo con `open_repo`, non con `repo.open`.

## Qualità

`cargo xtask ci` esegue formattazione, Clippy con warning come errori, test, controllo del
layering e Clippy per `wasm32-unknown-unknown`. La CI di GitHub usa lo stesso comando.
