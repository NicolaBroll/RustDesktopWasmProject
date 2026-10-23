---
name: git-scenarios
description: Scrive test di integrazione del client git su repo veri creati in cartelle temporanee (crates/git/tests) - helper di scenari, casi limite come conflitti, HEAD staccata, rebase interrotto, submodule, CRLF, nomi non UTF-8. Usalo per S07 e quando uno step ha bisogno di scenari nuovi.
---

Lavori sul client git. Leggi prima `impl-docs/git/regole.md` (invarianti, checklist, come
tenere basso l'output), poi §9 di `impl-docs/ARCHITETTURA-git.md` e solo i file di
`impl-docs/git/` delle funzioni che provi.

Branch: ogni implementazione parte dall'ultima versione di `origin/dev` (`AGENTS.md`, "Branch").
Prima `git fetch origin dev`; un branch nuovo si crea con `git checkout -b <nome> origin/dev`. Se
lavori su un branch già creato, controlla con `git merge-base --is-ancestor origin/dev HEAD` che
contenga l'ultimo `origin/dev`; se no fermati e dillo.

Ambito: `crates/git/tests` e le dipendenze di test di `crates/git`. Non cambi il codice di
produzione: se un test trova un bug, lascia il test (fallisce, con un commento che descrive il
bug) e segnalalo nel resoconto, così lo sistema `git-engine`.

Regole:
- Ogni test crea il proprio repo in una cartella `tempfile` e non tocca nulla fuori.
- Ambiente deterministico per ogni processo git: `GIT_CONFIG_GLOBAL=/dev/null`,
  `GIT_CONFIG_NOSYSTEM=1`, autore e committer fissi, `GIT_AUTHOR_DATE` e `GIT_COMMITTER_DATE`
  fissi e crescenti, branch iniziale fissato (`-c init.defaultBranch=main`).
- I remoti sono repo `--bare` locali: niente rete.
- Gli scenari sono funzioni con nomi chiari (`repo_with_merge_conflict`, `repo_detached_head`,
  ...) e un commento che dice cosa preparano.
- I test devono passare sempre: eseguili due volte di fila prima di consegnare.

Durante il lavoro `cargo test -q -p project-git --test <file>`; chiudi con `cargo xtask ci
--quiet` verde. Resoconto breve: scenari nuovi ed eventuali bug trovati.
