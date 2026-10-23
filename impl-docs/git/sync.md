# Fetch, pull e push

Parte di [ARCHITETTURA-git.md](../ARCHITETTURA-git.md): principi (§1), regole per ogni chiamata a
git (§5), comandi lenti (§6) e diario (§7) sono lì; qui il contratto dei comandi, il `CliBackend` e
undo e backup di questa funzione. "§n" rimanda sempre al documento principale.

## Comandi

### `remote.fetch`

Parametri: `{"remote"?, "branch"?}`.

Lento (§6), scrive (`writes`: branch remoti, tag, `FETCH_HEAD`). `remote`: il **nome** di un remoto
configurato (`GitBackend::remotes`, cioè `git remote`), controllato con `check_remote_name` (come
git: `refs/remotes/<nome>/x` dev'essere un nome di ref valido; un `-` iniziale è ammesso) e cercato
fra i configurati prima di chiamare il backend: un nome che non c'è → `NotAllowed` con l'elenco dei
remoti (mai passato a git, che lo userebbe come URL o percorso). Senza `remote`: quello
dell'upstream del branch attivo, altrimenti `origin`, altrimenti l'unico remoto (`default_remote`;
nessuno o ambiguo → `NotAllowed`). `branch`: nome breve di un branch **del remoto**
(`check_branch_name`), scaricato da solo come `refs/heads/<branch>`; il remoto non lo ha →
`NotAllowed`. Errori: credenziali rifiutate o da chiedere → `Auth { remote, message }`; remoto non
raggiungibile (cartella che non c'è, host, rete, repository inesistente sul server) →
`RemoteUnreachable { remote, message }`; annullato → `Cancelled`; altro → `Git`. Risultato:
`{"remote", "branch": string | null, "updated": [{"name", "before": oid | null, "after": oid |
null}] | null, "messages": [string], "warnings": [string], "refs": [RefInfo] | null,
"refs_error"?}`: `updated` sono i ref cambiati (confronto di `refs()` prima e dopo, un tag annotato
con il commit puntato; `null` con `refs_error` se i ref non si rileggono: il fetch è fatto),
`messages` le righe di git che non sono avanzamento (`From <url>`, ` * [new branch] ...`),
`warnings` i ref fuori da `refs/remotes/` e `refs/tags/` spostati da un refspec configurato (es. un
mirror verso `refs/heads/`). Non va nel diario (§7). Dettagli del `CliBackend` più sotto.

### `remote.pull`

Parametri: `{"remote"?, "branch"?, "strategy"?}`.

Lento (§6), scrive (`writes`), entra nel diario (§7). Fetch del solo branch dell'upstream con
`GitBackend::fetch` (avanzamento e annullamento come `remote.fetch`; annullato durante il fetch →
`Cancelled { command: "remote.pull" }` anche se il fetch è finito, prima di integrare), poi
l'integrazione in locale con `GitBackend::pull`, che non si interrompe. **Scelta: non `git pull`**,
ma fetch + merge/rebase guidati dal motore: così il comando sa cosa è arrivato prima di scrivere,
sceglie la strategia, controlla lo stato e salva il backup. `branch`: nome breve del branch locale
(HEAD se manca; HEAD staccata senza `branch` → `NotAllowed`); un branch che non è quello di HEAD si
può solo portare avanti (fast-forward del ref con `update-ref` e il valore vecchio, senza toccare il
worktree; divergente → `NotAllowed` che chiede di passarci; aperto in un altro worktree →
`NotAllowed`). `remote`: se c'è dev'essere quello dell'upstream (`NotAllowed` altrimenti): il pull
integra sempre l'upstream. Upstream da `branch.<nome>.remote`/`.merge` (`GitBackend::upstream`);
senza → `NotAllowed` che dice come impostarlo (`git branch --set-upstream-to=<remoto>/<branch>`) o
di pubblicare il branch con `remote.push`; upstream locale (`.`), `merge` che non è un branch,
nessun refspec che lo mappa in `refs/remotes/` → `NotAllowed`, niente scaricato. **Strategia**
(`PullSettings::policy`, letta prima del fetch): `strategy` (`merge`, `rebase`, `ff_only`)
sostituisce la configurazione; altrimenti `branch.<nome>.rebase`, poi `pull.rebase` (`true` →
rebase, `merges` → `--rebase-merges`, `false` → merge, `interactive` → `NotAllowed`) e `pull.ff`
(`only` → solo fast-forward, `false` → sempre un commit di merge); **senza configurazione merge**
(scelta: git ≥ 2.33 rifiuterebbe i branch divergenti; un merge non riscrive niente e si annulla).
Poi (`pull_mode`, da `ahead`/`behind` con `unmerged_count`): niente da fare se il branch contiene
l'upstream (`up_to_date`, niente nel diario), fast-forward se può, altrimenti merge o rebase
(`ff_only` divergente → `NotAllowed`). **Controlli** (`check_pull_state`, prima del fetch per HEAD e
di nuovo nel backend): operazione in corso o file in conflitto → `NotAllowed`; merge e rebase con
qualunque modifica ai file tracciati → `NotAllowed` (mai `--autostash`; i non tracciati vanno bene);
il fast-forward accetta modifiche che non toccano i file cambiati, e se git rifiuta perché
sovrascriverebbe modifiche o non tracciati → `NotAllowed` con i file. **Backup**: prima di scrivere
`refs/project/backup/pull-...` sul branch com'era (tolto se poi niente cambia). **Conflitti**
(scelta): il merge o il rebase fermo in conflitto si **annulla subito** (`--abort`: il worktree era
pulito, quindi torna identico) e l'errore è `PullConflict { branch, upstream, operation: merge |
rebase, files, message }`; lasciare il repo nel merge/rebase in corso non avrebbe uscita dal client
finché non ci sono `merge.abort`/`merge.continue` (S29), l'editor dei conflitti (S30) e
`rebase.abort`/`rebase.continue` (S32); allora si potrà aggiungere un'opzione per tenerlo. Errori
del fetch: `Auth`, `RemoteUnreachable`, `Cancelled`. Risultato: `{"branch", "remote", "upstream",
"strategy": "fast_forward" | "merge" | "rebase" | "up_to_date", "strategy_source": "param" |
"config" | "default", "before", "after", "ahead", "behind", "backup": string | null, "fetch":
{"updated", "messages"}, "warnings", "status" | null, "refs" | null, "can_undo", "can_redo",
"next_undo", "next_redo"}`. Dettagli del `CliBackend` e undo più sotto.

### `remote.push`

Parametri: `{"remote"?, "branch"?, "force"?, "confirm"?, "expected"?}`.

Lento (§6), scrive (`writes`), **non** entra nel diario (un push non si annulla: il remoto è di
altri; il risultato dice cosa è cambiato). Pubblica solo
`refs/heads/<branch>:refs/heads/<remote_branch>` (refspec esplicito: mai tag, `--all`, `--mirror`,
`--tags`, qualunque sia `push.default`, `remote.<nome>.push`, `push.followTags`). `branch`: HEAD se
manca (staccata → `NotAllowed`). **Remoto** come lo sceglie `git push` (risultato `remote_source`):
`remote` (`param`), altrimenti `branch.<nome>.pushRemote` (`push_remote`), `remote.pushDefault`
(`push_default`), il remoto dell'upstream (`upstream`), `origin` (`origin`) o l'unico remoto
(`only`); più remoti senza `origin` → `NotAllowed`; un remoto della configurazione che non è
configurato → `NotAllowed` che nomina la chiave; un remoto con `remote.<r>.mirror=true` →
`NotAllowed` (un mirror pubblica tutti i ref insieme). **Destinazione**: se il remoto è quello
dell'upstream, il branch dell'upstream (con un nome diverso dal branch → `NotAllowed`, come
`push.default=simple`) e il branch remoto locale dell'upstream; altrimenti lo stesso nome, e il
branch remoto locale si ricava dai refspec di fetch del remoto (`tracking_ref` su
`remote.<r>.fetch`, anche con refspec non standard e negativi; nessuno → nessun branch remoto
locale). **Primo push** (nessun upstream): `--set-upstream` verso il remoto scelto (l'upstream si
imposta solo se il push riesce); con un upstream l'upstream non cambia mai (es. pubblicando su
`fork` per `remote.pushDefault`). **Force**: solo con `"force": true`, sempre e solo
`--force-with-lease=refs/heads/<b>:<oid>` con l'oid del branch remoto locale letto prima (vuoto se
non c'è: il branch non deve esistere), mai `--force` né `+refspec`; se il remoto perderebbe commit
che il branch non ha (`unmerged_count` del branch remoto locale) serve `"confirm": true` **con
`"expected"`**, l'id completo (40 cifre) del branch remoto locale mostrato nella conferma: senza,
`ConfirmRequired` con i commit (`missing_commits`, al più 10), l'id letto ora e la frase fissa
`"expected": "<id>"` da ripetere. La conferma vale solo per quel commit: se `expected` non è il
branch remoto locale letto ora (un fetch nel frattempo) → di nuovo `ConfirmRequired` con l'id nuovo,
niente pubblicato; il lease usa `expected`. Prima del push il branch remoto locale si salva in
`refs/project/backup/push-...` (tolto se il push è rifiutato). `confirm` senza `expected`,
`confirm`/`expected` senza `force`, `expected` non completo → `InvalidParams`. `--force-if-includes`
non serve: con un oid esplicito git lo ignora. Errori: `PushRejected { remote, branch, reason:
non_fast_forward | stale_lease | remote_rejected | hook | other, message }` (non fast-forward
suggerisce `remote.pull`; lease: il remoto si è spostato, fare fetch), niente cambia in locale;
`Auth`, `RemoteUnreachable`, `Cancelled` (annullato a metà: il remoto potrebbe aver già aggiornato
il branch, rileggere con un fetch). Risultato: `{"remote", "remote_source": "param" | "push_remote"
| "push_default" | "upstream" | "origin" | "only", "branch", "remote_branch", "kind": "new" |
"fast_forward" | "forced" | "up_to_date", "summary", "before": oid | null, "after", "set_upstream",
"forced", "lost": [Commit], "backup": string | null, "updated" | null, "messages", "warnings",
"status" | null, "refs" | null}`.

## CliBackend: fetch

**Fetch** (`remote.fetch`, S25): `git fetch --progress --end-of-options <remote>
[refs/heads/<branch>]`. Il remoto (sempre un remoto configurato, mai un URL) e il branch
vanno dopo `--end-of-options`: un remoto che si chiama `-x` o `--upload-pack=false` non
diventa un'opzione (test con questi nomi). Il branch è un refspec senza destinazione, senza
`:` né `+`: non scrive ref locali, e il branch remoto si aggiorna con il refspec configurato.
Niente `--prune`, `--force`, `--tags`, `--update-head-ok`, `--recurse-submodules`: tag,
pulizia dei branch remoti spariti (`fetch.prune`, `remote.<nome>.prune`) e sottomoduli come
dice la configurazione dell'utente, come `git fetch` da terminale. **Nessuna richiesta di
input**, mai: stdin chiuso, `GIT_TERMINAL_PROMPT=0` e, su Unix, il processo parte in una
sessione nuova (`setsid`, dipendenza `libc`) senza terminale di controllo, così né git, né
ssh (passphrase, conferma della chiave dell'host), né un credential helper possono aprire
`/dev/tty` e aspettare, anche se CLI o server MCP sono avviati da un terminale. Restano le
richieste grafiche (`SSH_ASKPASS` con un display, `GIT_ASKPASS`, `core.askPass`) e i
credential helper che non chiedono niente (cache, keychain, libsecret, GCM); senza, una
credenziale mancante fallisce subito con `Auth`. stderr si legge mentre git gira
(`Git::run_streaming`): diviso a ogni `\r` e `\n`, le righe di avanzamento vanno al
`JobContext` (§6), le altre, solo quelle chiuse da `\n`, sono i `messages` del risultato o il
testo dell'errore. Errori riconosciuti dal testo (`LC_ALL=C`, `progress::remote_error`):
`Auth` per "Authentication failed", "could not read Username/Password", "terminal prompts
disabled", "Permission denied (publickey...)", "Host key verification failed", HTTP 401/403;
`NotAllowed` per "couldn't find remote ref"; `RemoteUnreachable` per "does not appear to be a
git repository", "Could not resolve host", "Connection refused/timed out", "repository not
found", HTTP 404, "unable to access", "Could not read from remote repository" (guardati dopo
quelli di `Auth`, che li precedono nello stesso stderr). Dopo l'uscita di git il backend
aspetta al massimo un secondo che stderr si chiuda: un processo rimasto in background (un
master ssh con `ControlPersist`) non blocca il comando. Annullamento: vedi §6. Limiti: su
Windows niente `setsid` (non c'è `/dev/tty`; l'annullamento usa `kill`); la configurazione
del repo può dare remoti con URL fuori dalla radice del server MCP (il fetch legge da lì, come
da terminale).

## CliBackend: pull

**Pull** (`remote.pull`, S26): dopo il fetch di `refs/heads/<merge>` (come sopra), il backup
e poi, con `GIT_REFLOG_ACTION=project: remote.pull`, `GIT_EDITOR=:` e `GIT_SEQUENCE_EDITOR=:`:
fast-forward del branch attivo `git -c core.fsmonitor=false merge --ff-only --no-autostash
--no-overwrite-ignore --no-edit --end-of-options <oid>` (git rifiuta prima di scrivere se sovrascriverebbe modifiche
o non tracciati); di un altro branch `update-ref -m ... refs/heads/<b> <nuovo> <vecchio>`;
merge `git -c core.fsmonitor=false merge --no-autostash --no-overwrite-ignore --no-edit --commit --no-squash
--ff|--no-ff -F <file> --end-of-options <oid>` (il messaggio, `Merge remote-tracking branch 'origin/main'` più ` into
<b>` fuori da `main`/`master`, da un file temporaneo nella cartella git: `git merge` non legge
`-F -` da stdin, e il suo percorso va a git come `OsStr`; `--ff`/`--no-ff`, `--commit`, `--no-squash` espliciti contro `merge.ff` e
`branch.<nome>.mergeOptions`); rebase `git -c
core.fsmonitor=false -c rebase.updateRefs=false rebase --no-autostash --no-autosquash [--rebase-merges]
--end-of-options <oid>` (`rebase.updateRefs` sposterebbe altri branch fuori dal diario; senza il
fork-point di `git pull --rebase`: con un upstream riscritto il rebase di solito si ferma in
conflitto, e si annulla). **File ignorati**: git sovrascrive senza dirlo un file ignorato che
l'upstream aggiunge (e un merge annullato lo cancellerebbe); il rebase non ha
`--no-overwrite-ignore`. Per questo, per il branch attivo e prima del backup, i file che
l'upstream porta e HEAD non ha (`diff-tree -r -z --name-only --no-renames --diff-filter=A HEAD
<upstream>`) non devono esistere nel worktree, né al posto di una loro cartella
(`symlink_metadata`): altrimenti `NotAllowed` con i file e niente cambia (per fast-forward e
merge è una seconda difesa dopo `--no-overwrite-ignore`). Per il rebase (anche
`--rebase-merges`) contano anche i file toccati da ognuno dei commit da riapplicare e che
HEAD non ha (`git log -m --no-renames --format= -z --name-only <upstream>..HEAD`): il rebase
passa per gli alberi intermedi, quindi un file che un commit locale aggiunge e uno successivo
toglie (o rinomina, o toglie e ricrea), o che un commit locale modifica mentre l'upstream lo
toglie (conflitto modify/delete, poi `rebase --abort`), verrebbe scritto e poi cancellato
insieme all'ignorato che c'era al suo posto. Il merge passa solo da HEAD al
risultato e non ne ha bisogno. Il `MemoryBackend` fa lo stesso controllo sugli alberi dei
commit da riapplicare. Hook (`pre-merge-commit`, `commit-msg`, `post-merge`,
`post-rewrite`) e firma seguono la configurazione. Fermo (conflitto o altro): file in conflitto
dallo stato e quelli che git dice in conflitto (`Merge conflict in <file>`, e con
`rerere.autoUpdate` `Resolved '<file>' using previous resolution`: anche un conflitto risolto
da rerere lascia il merge/rebase fermo ed è `PullConflict`), `git merge --abort` / `git rebase
--abort`, e se l'annullamento non riesce un
errore che lo dice. Dopo: il branch deve contenere l'upstream, HEAD sul branch, altrimenti
`warnings`. File `--assume-unchanged` diversi fra i due commit → `NotAllowed` (git li
sovrascriverebbe senza controllarli).

## CliBackend: push

**Push** (`remote.push`, S26): `git push --porcelain --progress --no-follow-tags
--recurse-submodules=check [--set-upstream] [--force-with-lease=refs/heads/<dst>:<oid>]
--end-of-options <remote> refs/heads/<b>:refs/heads/<dst>`, con l'ambiente del fetch (nessun
terminale, stdin chiuso, `setsid`). `--recurse-submodules=check` al posto di un eventuale
`push.recurseSubmodules=on-demand`, che pubblicherebbe anche i sottomoduli (e rifiuta un push
che riferisce commit di sottomoduli non pubblicati). Mai `--no-verify`: gli hook `pre-push`
girano. stderr come il fetch (avanzamento al `JobContext`, il resto in `messages` o
nell'errore); l'esito dallo stdout di `--porcelain` (`Git::run_streaming` con
`capture_stdout`): la riga `<flag>\t<da>:<a>\t<riassunto>` del ref (` ` fast-forward, `+`
forzato, `*` nuovo, `=` già aggiornato, `!` rifiutato: `non-fast-forward`/`fetch first`,
`stale info`, `remote rejected`). Uscita non zero senza quella riga: `Auth`/`RemoteUnreachable`
dal testo come il fetch, altrimenti con "failed to push some refs" un hook `pre-push` (o il
controllo dei sottomoduli): `PushRejected` con l'output dell'hook.

## Undo del pull

`remote.pull` entra nel diario quando sposta il branch: voce `remote.pull` con il branch da
`before` ad `after`, HEAD uguale prima e dopo, il backup `refs/project/backup/pull-...` (il
branch com'era, salvato prima di scrivere) e la descrizione `Pull di `main` (fast-forward: da
a1b2c3d a e4f5a6b)` / `(merge di origin/main: ...)` / `(rebase su origin/main: ...)`. Il fetch
fatto dal pull no, come `remote.fetch`.

- **Branch attivo: undo e redo con indice e worktree** (`GitBackend::reset_branch`): spostare
  solo il ref (come per un commit) lascerebbe nel worktree i file del pull, che sembrerebbero
  modifiche locali da annullare a mano. Prima gli stessi controlli di `restore_refs`
  (operazione in corso, HEAD e branch come li ha lasciati il pull, altrimenti `UndoConflict`) e
  i backup dei commit abbandonati (il commit di merge, i commit riscritti dal rebase:
  `refs/project/backup/undo-...`), poi `git -c core.fsmonitor=false reset --keep <oid>`: i file
  diversi fra i due commit tornano com'erano, **solo se non hanno modifiche locali** (altrimenti
  `NotAllowed` e la voce resta), le altre modifiche restano, e un non tracciato non si
  sovrascrive. Prima ancora si controlla che nessun file da ricreare esista nel worktree
  (`reset --keep` sovrascriverebbe un ignorato) e i file `--assume-unchanged`. Limite: fra il
  controllo dei ref e `reset` resta un istante in cui un altro processo potrebbe spostare il
  branch (`reset` aggiorna con il valore letto da sé); il commit spostato da fuori resterebbe nel
  reflog. Il redo è lo stesso spostamento al contrario (il commit di dopo è tenuto dal backup
  dell'undo).
- **Branch non attivo**: solo il ref (fast-forward), undo e redo con `restore_refs`.
- Un push non si annulla da qui; dopo un push l'undo di un pull precedente resta possibile
  (controlla solo il branch locale), ma il remoto ha già i commit.

Il `MemoryBackend` fa lo stesso con i suoi controlli del checkout; registra i backup dei pull in
`commit_backups` (`refs/project/backup/pull-<n>`) e quelli dei force push (`push-<n>`).

## Il fetch non entra nel diario

**Il fetch non entra nel diario** (scelta, S25): sposta solo branch remoti e tag, che
rispecchiano il remoto. Riportarli indietro farebbe vedere un remoto che non esiste più (e il
fetch successivo li rimetterebbe dov'erano), e niente di locale va perso: gli oggetti si
aggiungono soltanto, branch locali, indice e worktree non si toccano, e un branch remoto tolto
da `fetch.prune` è sparito dal remoto. Le voci già nel diario non ne risentono: controllano solo
HEAD e i ref che hanno spostato. L'eccezione è un refspec configurato verso `refs/heads/` (un
mirror): il valore di prima resta nel reflog del ref e il risultato lo segnala in `warnings`.
