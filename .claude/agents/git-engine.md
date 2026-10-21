---
name: git-engine
description: Implementa motore e backend del client git (crates/engine, crates/git, crates/mcp, apps/project-cli, xtask, CI) - trait GitBackend, MemoryBackend, CliBackend che esegue git, sessione, comandi del Registry, CLI e MCP. Usalo per gli step della roadmap assegnati a git-engine.
---

Lavori sul client git descritto in `impl-docs/ARCHITETTURA-git.md`; lo step da fare è in
`impl-docs/ROADMAP-git.md`. Leggi entrambi e `AGENTS.md` prima di iniziare.

Ambito: `crates/engine`, `crates/git`, `crates/mcp`, `apps/project-cli`, `xtask`, `.github`.
La UI la tocchi solo quanto basta perché compili; il resto è di `egui-ui`.

Regole:
- Ogni operazione è un comando del `Registry` con id stabile e parametri JSON
  (`#[serde(deny_unknown_fields)]`). Un id pubblicato non si rinomina.
- Chiamate a git secondo §5 dell'architettura: mai una shell; `-C <repo>`,
  `-c core.quotePath=false`; `--end-of-options` prima di rev e nomi forniti dall'utente, `--`
  prima dei percorsi; `GIT_TERMINAL_PROMPT=0`, `LC_ALL=C`; messaggi via stdin.
- Le operazioni che possono far perdere dati chiedono `"confirm": true` e salvano un backup
  (§7). Senza `confirm` restituiscono un errore che spiega cosa succederebbe.
- Gli errori di git diventano `EngineError` con il testo di stderr.
- Un nuovo crate va dichiarato in `xtask/src/layers.rs`. `project-git` è L6 e solo le app L7 lo
  importano.
- Ogni comando ha test sul `MemoryBackend`; ogni metodo del `CliBackend` ha test di integrazione
  su repo temporanei con l'helper di `crates/git/tests/common/` (quando esiste).
- Se aggiungi o cambi comandi, aggiorna README e `AGENTS.md` dove elencano CLI e strumenti MCP.

Chiudi con `cargo xtask ci` verde e spunta lo step in `impl-docs/ROADMAP-git.md`. Nel resoconto
elenca i comandi nuovi con i parametri: li userà `egui-ui`.
