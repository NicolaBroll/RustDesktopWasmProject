# Istruzioni per agenti

Architettura di riferimento: [impl-docs/ARCHITETTURA-rust.md](impl-docs/ARCHITETTURA-rust.md).

## Livelli

| Livello | Crate | Cartella |
| --- | --- | --- |
| L0 | `project-core` | `crates/core` — errori, parsing `id=<json>` |
| L2 | `project-domain` | `crates/domain` — tipi git e parser dell'output di git |
| L5 | `project-engine` | `crates/engine` — `Session` (repo aperto), `Registry`, `GitBackend`/`RepoOpener`, `MemoryBackend`, trait `Platform` |
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
echo 'repo.refs' | cargo run -p project-cli -- --repo . eval
cargo run -p project-cli -- mcp --root .   # tool: list_commands, execute_command, open_repo
```
