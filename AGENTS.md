# Istruzioni per agenti

Architettura di riferimento: [impl-docs/ARCHITETTURA-rust.md](impl-docs/ARCHITETTURA-rust.md).

## Livelli

| Livello | Crate | Cartella |
| --- | --- | --- |
| L0 | `project-core` | `crates/core` — errori, parsing `id=<json>` |
| L2 | `project-domain` | `crates/domain` — tipi git e parser dell'output di git |
| L5 | `project-engine` | `crates/engine` — `Session` (repo aperto e diario per undo/redo), `Registry`, `GitBackend`/`RepoOpener`, `MemoryBackend`, trait `Platform` |
| L6 | `project-ui` | `crates/ui` — `App` egui, nessun `cfg(target_arch)` |
| L6 | `project-mcp` | `crates/mcp` — server MCP su stdio |
| L6 | `project-git` | `crates/git` — `CliBackend`/`CliOpener`: esegue `git`; lo importano solo le app L7 |
| L7 | `project`, `project-cli`, `project-web` | `apps/*` — solo avvio e collegamento |

Un crate importa solo da livelli **strettamente inferiori**; `cargo xtask layers` lo verifica.
Un nuovo crate va dichiarato in `xtask/src/layers.rs`.

## Regole

- Ogni operazione sul repository è un comando del `Registry` (`crates/engine/src/builtin.rs`).
  UI, CLI, MCP e test lo invocano con `Registry::run`; niente logica di modifica nei callback grafici.
- Gli id dei comandi sono stabili: non rinominarli.
- Un comando che modifica il repo va in `WRITES` (`builtin.rs`): `CommandSpec::writes` lo segnala a
  UI, CLI e MCP. Se può far perdere dati chiede `"confirm": true` e salva un backup; se riscrive
  l'unica copia di qualcosa senza che git chieda conferma (stage/unstage) salva da solo un backup
  e non scrive se il backup fallisce (§1 e §7
  dell'architettura git). Chi riscrive un commit (`commit.create` con `amend`) salva sempre un
  ref di backup del commit riscritto e chiede `confirm` se quel commit è già pubblicato.
- Un comando che sposta ref (commit, e poi branch, checkout, merge, reset...) registra una
  `JournalEntry` nel diario della `Session` con HEAD e i ref prima e dopo, così `edit.undo` e
  `edit.redo` lo coprono (§7 dell'architettura git).
- L'I/O della piattaforma passa dal trait `Platform`; le implementazioni stanno nelle app L7.

## Branch

- Si lavora sempre su `dev`, non su `master`: ogni nuovo branch parte dall'ultimo `origin/dev`
  e le pull request vanno verso `dev`.
- Prima di iniziare uno step, `git fetch origin dev` e controllare lo stato della roadmap su
  `origin/dev`.

## Client git (in sviluppo)

Il progetto sta diventando un client git grafico: architettura in
[impl-docs/ARCHITETTURA-git.md](impl-docs/ARCHITETTURA-git.md), step in
[impl-docs/ROADMAP-git.md](impl-docs/ROADMAP-git.md). Ogni step indica l'agente da usare
(`.claude/agents/`): `git-domain`, `git-engine`, `egui-ui`, `git-scenarios`; gli step che
scrivono nel repo dell'utente passano anche da `safety-reviewer`.

## Prima di consegnare

```sh
cargo xtask ci   # fmt, clippy -D warnings, test, layering, clippy wasm
```

## Provare senza UI

```sh
cargo run -p project-cli -- commands
cargo run -p project-cli -- --repo . run --cmd repo.status --cmd 'repo.log={"limit":5}'
cargo run -p project-cli -- --repo . run --cmd 'commit.show={"id":"HEAD"}'
cargo run -p project-cli -- --repo . run --cmd diff.worktree --cmd 'diff.commit={"id":"HEAD","path":"README.md"}'
# scrivono nell'indice del repo (segnati `[modifica]` in `commands`, `writes` in MCP)
cargo run -p project-cli -- --repo . run --cmd 'index.stage={"paths":["README.md"]}' --cmd 'index.unstage={"paths":["README.md"]}'
# righe: numeri e `digest` dal diff del file, poi
cargo run -p project-cli -- --repo . run --cmd 'index.stage_lines={"path":"README.md","lines":[{"new":3}],"digest":"<da diff.worktree>"}'
# commit di quello che è in stage; amend di un commit già pubblicato vuole anche "confirm":true
cargo run -p project-cli -- --repo . run --cmd 'commit.create={"message":"Titolo\n\nCorpo"}'
cargo run -p project-cli -- --repo . run --cmd 'commit.create={"message":"Titolo corretto","amend":true}'
# scarta le modifiche non in stage (worktree come l'indice): vuole "confirm":true, salva un backup
cargo run -p project-cli -- --repo . run --cmd 'worktree.discard={"paths":["README.md"],"confirm":true}' --cmd edit.undo
# branch: crea, passa, rinomina, elimina (non unito: "confirm":true; backup in refs/project/backup/delete-...)
cargo run -p project-cli -- --repo . run --cmd 'branch.create={"name":"prova"}' --cmd 'branch.checkout={"name":"prova"}' --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'branch.rename={"name":"prova","new_name":"prova2"}' --cmd 'branch.delete={"name":"prova2"}'
# diario delle operazioni: vale solo dentro la stessa invocazione (o sessione MCP)
cargo run -p project-cli -- --repo . run --cmd 'commit.create={"message":"prova"}' --cmd edit.undo --cmd edit.history
echo 'repo.refs' | cargo run -p project-cli -- --repo . eval
cargo run -p project-cli -- mcp --root .   # tool: list_commands, execute_command, open_repo
```
