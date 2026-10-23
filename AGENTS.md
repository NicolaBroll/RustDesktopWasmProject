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
- Gli stash (`stash.*`) si indicano con la posizione `index` e l'id `expected`; `apply`/`pop`
  non scrivono mai sopra file con modifiche locali (controllo prima di git) e i conflitti
  restano con lo stash tenuto. `stash.push` e `stash.drop` entrano nel diario, `apply` e `pop`
  no; `drop` (e il drop di `pop`) salva prima un backup e vuole `confirm` con `expected`.
  `stash.push` rifiuta i file `--assume-unchanged` (git ne perderebbe le modifiche).
- `remote.pull` è fetch e poi integrazione guidata dal motore (mai `git pull`, mai
  `--autostash`); `remote.push` usa solo un refspec esplicito e, per il force,
  `--force-with-lease` con l'oid atteso (mai `--force`). Il pull entra nel diario, il push no.
  Un pull in merge che si ferma (conflitti, hook) lascia il merge in corso come `merge.start`
  (`in_progress`, `conflicts`; fuori dal diario finché `merge.continue`); in rebase si annulla
  (`PullConflict`) finché S32 non porta `rebase.continue`/`rebase.abort`.
- `merge.start` unisce una rev nel branch attivo (fast-forward o commit di merge, mai
  `--autostash`, rifiutato con modifiche ai file tracciati per un commit di merge); con conflitti
  il merge **resta in corso** (come il pull in merge; il pull in rebase invece si annulla) e si esce solo con
  `merge.continue` (nessun file in conflitto rimasto) o `merge.abort` (sempre `"confirm": true`,
  backup di indice e worktree prima). Entra nel diario solo il merge concluso; `repo.operation` è
  la lettura per il banner dell'operazione in corso (`impl-docs/git/merge.md`).
- Un comando che sposta ref (commit, e poi branch, checkout, merge, reset...) registra una
  `JournalEntry` nel diario della `Session` con HEAD e i ref prima e dopo, così `edit.undo` e
  `edit.redo` lo coprono (§7 dell'architettura git).
- L'I/O della piattaforma passa dal trait `Platform`; le implementazioni stanno nelle app L7.

## Branch

- Si lavora sempre su `dev`, non su `master`: le pull request vanno verso `dev`.
- **Ogni implementazione parte dall'ultima versione di `origin/dev`**: prima `git fetch origin
  dev`, poi il branch nuovo con `git checkout -b <nome> origin/dev` (mai da `master`, da un
  `dev` locale non aggiornato o da un altro branch di step). Chi continua su un branch già
  creato controlla che contenga l'ultimo `origin/dev` (`git merge-base --is-ancestor origin/dev
  HEAD`); se no si ferma e lo dice.
- Prima di iniziare uno step, controllare lo stato della roadmap su `origin/dev`.

## Client git (in sviluppo)

Il progetto sta diventando un client git grafico: architettura in
[impl-docs/ARCHITETTURA-git.md](impl-docs/ARCHITETTURA-git.md) (regole comuni) e in
[impl-docs/git/](impl-docs/git/) (un file per funzione: contratto dei comandi, `CliBackend`, undo
e backup; un comando nuovo va nel file della sua funzione), step in
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
# grafo di tutti i branch (locali e remoti), tag e HEAD; senza stash e backup
cargo run -p project-cli -- --repo . run --cmd 'repo.log={"all":true,"limit":20}'
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
# fetch da un remoto configurato (solo branch remoti e tag; avanzamento su stderr se è un terminale)
cargo run -p project-cli -- --repo . run --cmd remote.fetch --cmd 'remote.fetch={"remote":"origin","branch":"main"}'
# pull (fetch + fast-forward/merge/rebase secondo pull.rebase/pull.ff, o "strategy"; annullabile con edit.undo;
# merge con conflitti: resta in corso, "in_progress":true, poi merge.continue o merge.abort)
cargo run -p project-cli -- --repo . run --cmd remote.pull --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'remote.pull={"branch":"main","strategy":"rebase"}'
# push del branch attivo (remoto: pushRemote/pushDefault/upstream/origin; primo push: imposta l'upstream);
# force solo con lease e, se il remoto perde commit, "confirm":true con "expected" (l'id mostrato dalla conferma)
cargo run -p project-cli -- --repo . run --cmd remote.push --cmd 'remote.push={"force":true,"confirm":true,"expected":"<id dalla conferma>"}'
# remoti: elenco (credenziali negli URL mascherate), aggiunta (con fetch), rimozione ("confirm":true) e undo
cargo run -p project-cli -- --repo . run --cmd remote.list
cargo run -p project-cli -- --repo . run --cmd 'remote.add={"name":"fork","url":"../fork.git","fetch":true}' --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'remote.remove={"name":"fork","confirm":true}' --cmd edit.undo
# stash: elenco, push (con i non tracciati), apply/pop (index ed expected dall'elenco), drop ("confirm":true con "expected", backup) e undo
cargo run -p project-cli -- --repo . run --cmd 'stash.push={"message":"a metà","include_untracked":true}' --cmd stash.list --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'stash.pop={"index":0,"expected":"<id da stash.list>","restore_index":true}'
cargo run -p project-cli -- --repo . run --cmd 'stash.drop={"index":0,"confirm":true,"expected":"<id dalla conferma>"}' --cmd edit.undo
# grafo con anche i commit su cui sono stati creati gli stash
cargo run -p project-cli -- --repo . run --cmd 'repo.log={"all":true,"stashes":true,"limit":20}'
# merge di un branch nel branch attivo (fast-forward o commit di merge; annullabile con edit.undo)
cargo run -p project-cli -- --repo . run --cmd 'merge.start={"rev":"prova"}' --cmd edit.undo
cargo run -p project-cli -- --repo . run --cmd 'merge.start={"rev":"origin/main","no_ff":true,"message":"Unisci main"}'
# con conflitti il merge resta in corso: banner (tipo, file in conflitto, commit in ingresso), poi
# risolvere i file, metterli in stage e concludere; oppure annullare ("confirm":true, backup)
cargo run -p project-cli -- --repo . run --cmd repo.operation
cargo run -p project-cli -- --repo . run --cmd 'index.stage={"paths":["a.txt"]}' --cmd merge.continue
cargo run -p project-cli -- --repo . run --cmd 'merge.abort={"confirm":true}'
# diario delle operazioni: vale solo dentro la stessa invocazione (o sessione MCP)
cargo run -p project-cli -- --repo . run --cmd 'commit.create={"message":"prova"}' --cmd edit.undo --cmd edit.history
echo 'repo.refs' | cargo run -p project-cli -- --repo . eval
cargo run -p project-cli -- mcp --root .   # tool: list_commands, execute_command, open_repo
```
