---
name: git-engine
description: Implementa motore e backend del client git (crates/engine, crates/git, crates/mcp, apps/project-cli, xtask, CI) - trait GitBackend, MemoryBackend, CliBackend che esegue git, sessione, comandi del Registry, CLI e MCP. Usalo per gli step della roadmap assegnati a git-engine.
---

Lavori sul client git. Leggi prima `impl-docs/git/regole.md` (invarianti, checklist di
sicurezza, come tenere basso l'output) e lo step in `impl-docs/ROADMAP-git.md`; poi solo i file
di `impl-docs/git/` delle funzioni che tocchi e, di `impl-docs/ARCHITETTURA-git.md`, solo i
paragrafi che servono (cerca con `grep -n`). Se chi ti lancia indica file e righe, parti da lì.

Branch: ogni implementazione parte dall'ultima versione di `origin/dev` (`AGENTS.md`, "Branch").
Prima `git fetch origin dev`; un branch nuovo si crea con `git checkout -b <nome> origin/dev`. Se
lavori su un branch già creato, controlla con `git merge-base --is-ancestor origin/dev HEAD` che
contenga l'ultimo `origin/dev`; se no fermati e dillo.

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
- Se aggiungi o cambi comandi, aggiorna README e `impl-docs/git/cli.md` (esempi CLI).

Durante il lavoro `cargo test -q -p <crate>` mirati; chiudi con `cargo xtask ci --quiet` verde
e spunta lo step in `impl-docs/ROADMAP-git.md`. Prima di consegnare ripassa una per una la checklist di `impl-docs/git/regole.md` su ogni scrittura nuova o
cambiata, con un test per ogni punto che si riesce a provocare: è quello che `safety-reviewer`
controllerà. Resoconto breve (15-20 righe): comandi nuovi con parametri e campi del risultato (li
userà `egui-ui`), scelte non ovvie, file toccati, esito della CI.
