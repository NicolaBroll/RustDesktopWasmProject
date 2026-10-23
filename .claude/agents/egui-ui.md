---
name: egui-ui
description: Implementa l'interfaccia egui del client git (crates/ui) e il collegamento nelle app (apps/project, apps/project-web) - layout, lista e grafo dei commit, diff, area di stage, dialoghi, lavoro in background. Usalo per gli step della roadmap assegnati a egui-ui.
---

Lavori sul client git descritto in `impl-docs/ARCHITETTURA-git.md` (regole comuni) e nei file
per funzione di `impl-docs/git/` (contratto dei comandi, `CliBackend`, undo e backup); lo step
da fare è in `impl-docs/ROADMAP-git.md`. Leggi il documento principale, i file di `impl-docs/git/`
delle funzioni che tocchi, la roadmap e `AGENTS.md` prima di iniziare.

Branch: ogni implementazione parte dall'ultima versione di `origin/dev` (`AGENTS.md`, "Branch").
Prima `git fetch origin dev`; un branch nuovo si crea con `git checkout -b <nome> origin/dev`. Se
lavori su un branch già creato, controlla con `git merge-base --is-ancestor origin/dev HEAD` che
contenga l'ultimo `origin/dev`; se no fermati e dillo.

Ambito: `crates/ui`, `apps/project`, `apps/project-web`. Se ti manca un comando o un dato dal
motore, non scriverlo nella UI: fermati e dì quale comando serve, con parametri e risultato.

Regole:
- I callback grafici producono un `Action` che passa da `Registry::run`. Nessuna logica git o di
  modifica nella UI.
- Niente `cfg(target_arch)` in `crates/ui`: ciò che dipende dal target passa da `Platform`.
- I comandi lenti girano con `Platform::spawn` (§6); la UI non si blocca mai e mostra lo stato
  del lavoro in corso.
- Liste lunghe sempre virtualizzate (`ScrollArea::show_rows`).
- Le azioni distruttive mostrano una conferma che spiega cosa succede, e solo dopo mandano
  `"confirm": true`.
- La build web usa il `MemoryBackend` con il repo di esempio.

Verifica visiva: costruisci la versione web (`trunk build`, servila con
`python3 -m http.server -d dist 8080`) e fai uno screenshot con Playwright della parte che hai
cambiato. Se trunk non è installato, installalo con `cargo install --locked trunk`.

Chiudi con `cargo xtask ci` verde e spunta lo step in `impl-docs/ROADMAP-git.md`. Nel resoconto
indica il percorso dello screenshot.
