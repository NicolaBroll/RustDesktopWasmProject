# Istruzioni per agenti

Architettura di riferimento: [impl-docs/ARCHITETTURA-rust.md](impl-docs/ARCHITETTURA-rust.md).

## Livelli

| Livello | Crate | Cartella |
| --- | --- | --- |
| L0 | `project-core` | `crates/core` — errori, parsing `id=<json>` |
| L2 | `project-domain` | `crates/domain` — `Document`, formato file |
| L5 | `project-engine` | `crates/engine` — `Session` (undo/redo), `Registry`, trait `Platform` |
| L6 | `project-ui` | `crates/ui` — `App` egui, nessun `cfg(target_arch)` |
| L6 | `project-mcp` | `crates/mcp` — server MCP su stdio |
| L7 | `project`, `project-cli`, `project-web` | `apps/*` — solo avvio e collegamento |

Un crate importa solo da livelli **strettamente inferiori**; `cargo xtask layers` lo verifica.
Un nuovo crate va dichiarato in `xtask/src/layers.rs`.

## Regole

- Ogni modifica al documento è un comando del `Registry` (`crates/engine/src/builtin.rs`).
  UI, CLI, MCP e test lo invocano con `Registry::run`; niente logica di modifica nei callback grafici.
- Gli id dei comandi sono stabili: non rinominarli.
- L'I/O della piattaforma passa dal trait `Platform`; le implementazioni stanno nelle app L7.

## Prima di consegnare

```sh
cargo xtask ci   # fmt, clippy -D warnings, test, layering, clippy wasm
```

## Provare senza UI

```sh
cargo run -p project-cli -- commands
cargo run -p project-cli -- run --cmd 'item.add={"text":"latte"}' --cmd document.get
cargo run -p project-cli -- mcp --root .
```
