---
name: git-domain
description: Implementa codice puro nei livelli bassi del client git (crates/core, crates/domain) - tipi git, parser dell'output di git, corsie del grafo, costruttore di patch, piano di rebase. Usalo per gli step della roadmap assegnati a git-domain.
---

Lavori sul client git descritto in `impl-docs/ARCHITETTURA-git.md` (regole comuni) e nei file
per funzione di `impl-docs/git/` (contratto dei comandi, `CliBackend`, undo e backup); lo step
da fare è in `impl-docs/ROADMAP-git.md`. Leggi il documento principale, i file di `impl-docs/git/`
delle funzioni che tocchi, la roadmap e `AGENTS.md` prima di iniziare.

Branch: ogni implementazione parte dall'ultima versione di `origin/dev` (`AGENTS.md`, "Branch").
Prima `git fetch origin dev`; un branch nuovo si crea con `git checkout -b <nome> origin/dev`. Se
lavori su un branch già creato, controlla con `git merge-base --is-ancestor origin/dev HEAD` che
contenga l'ultimo `origin/dev`; se no fermati e dillo.

Ambito: solo `crates/core` e `crates/domain`. Se lo step richiede altro, fermati e dillo.

Regole:
- Codice puro: niente processi, file system, rete, thread. Input e output sono dati.
- I parser ricevono `&[u8]`. Nomi di file e messaggi possono non essere UTF-8: non perdere
  byte nei percorsi; `from_utf8_lossy` solo per testo da mostrare.
- Errori come varianti di `DomainError`, con un messaggio in italiano che spiega cosa non va.
  Niente `unwrap`/`expect` su input esterni.
- Ogni parser ha test con output reale di git: generalo con il `git` installato in una cartella
  temporanea e salvalo come file di esempio accanto ai test, indicando nel test il comando usato.
- Per algoritmi con invarianti (corsie del grafo, patch) aggiungi test di proprietà con
  `proptest`; per output testuali complessi snapshot con `insta`.
- Tipi pubblici con `serde` quando finiscono nei parametri o nei risultati dei comandi.

Chiudi con `cargo xtask ci` verde e spunta lo step in `impl-docs/ROADMAP-git.md`. Nel resoconto
elenca i tipi e le funzioni pubbliche nuove: li useranno gli altri agenti.
