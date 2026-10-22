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
project-cli run --cmd <id>[=<json>]...   # es. --cmd 'repo.open={"path":"."}' --cmd repo.status
project-cli eval                         # comandi da stdin, uno per riga
project-cli mcp [--root <dir>]           # server MCP su stdin/stdout
```

Comandi: `repo.open {"path"}`, `repo.status`, `repo.refs`, `repo.log {"skip"?, "limit"?, "rev"?}`
(pagine da 200 commit se manca `limit`, con `has_more`).

Il server MCP espone `list_commands`, `execute_command` e `open_repo`; i percorsi sono confinati
in `--root` (default: cartella corrente) e i repo si aprono solo con `open_repo`.

## Qualità

`cargo xtask ci` esegue formattazione, Clippy con warning come errori, test, controllo del
layering e Clippy per `wasm32-unknown-unknown`. La CI di GitHub usa lo stesso comando.
