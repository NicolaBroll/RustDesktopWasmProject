# Provare senza UI

Esempi di `project-cli` per ogni comando (il diario di undo/redo vale solo dentro la stessa
invocazione o sessione MCP). Contratti completi nei file per funzione di questa cartella.

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
