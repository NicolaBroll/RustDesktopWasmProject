# Project

Lista di voci con titolo, scritta in Rust con architettura a strati: motore indipendente dalla UI,
un unico frontend egui compilato per desktop e per WebAssembly, CLI e server MCP che usano lo stesso
catalogo di comandi. Dettagli in [impl-docs/ARCHITETTURA-rust.md](impl-docs/ARCHITETTURA-rust.md)
e [AGENTS.md](AGENTS.md).

## Avvio

```sh
make setup        # target wasm32 e trunk
make run          # app desktop
make web-serve    # app web su http://localhost:8080
make cli ARGS="commands --search item"
```

## CLI

```sh
project-cli commands [--search <testo>]
project-cli run [--in doc.json] --cmd <id>[=<json>]... [--out doc.json]
project-cli eval [--in doc.json] [--out doc.json]   # comandi da stdin, uno per riga
project-cli mcp [--root <dir>]                        # server MCP su stdin/stdout
```

Il server MCP espone `list_commands`, `execute_command`, `open_document` e `save_document`;
i percorsi sono confinati in `--root` (default: cartella corrente).

## Qualità

`cargo xtask ci` esegue formattazione, Clippy con warning come errori, test, controllo del
layering e Clippy per `wasm32-unknown-unknown`. La CI di GitHub usa lo stesso comando.
