# Commit e amend

Parte di [ARCHITETTURA-git.md](../ARCHITETTURA-git.md): principi (§1), regole per ogni chiamata a
git (§5), comandi lenti (§6) e diario (§7) sono lì; qui il contratto dei comandi, il `CliBackend` e
undo e backup di questa funzione. "§n" rimanda sempre al documento principale.

## Comandi

### `commit.create`

Parametri: `{"message", "amend"?, "confirm"?}`.

`git commit -F -` (vedi "CliBackend"). Controlli comuni ai backend (`check_commit_state`,
`CommitRequest::check_message`, `check_amend_published`), sempre prima di scrivere: messaggio vuoto
o di soli spazi (o con NUL) → `InvalidParams`; file in conflitto → `NotAllowed`; rebase,
cherry-pick, revert o `git am` in corso (anche un cherry-pick/revert di più commit fermo fra un
commit e l'altro: `.git/sequencer`) → `NotAllowed` (un commit da qui li concluderebbe o cambierebbe
in modo poco visibile: un cherry-pick prenderebbe l'autore originale); niente in stage (indice
uguale a HEAD, file `intent-to-add` esclusi) senza `amend` e senza merge → `NothingToCommit`, mai un
commit vuoto. Merge in corso: il commit è il commit di merge (genitori HEAD e `MERGE_HEAD`), anche
senza modifiche. Bisect: nessun controllo. Primo commit di un branch e HEAD staccata funzionano (con
HEAD staccata si sposta solo HEAD). Identità mancante: l'errore di git, nessun default. `amend`:
stessi genitori e autore, albero dall'indice, messaggio nuovo (anche solo il messaggio); rifiutato
senza commit, durante un merge e se renderebbe vuoto il commit (errore di git); un merge si riscrive
tenendo i due genitori. Se il commit riscritto è raggiungibile da un branch remoto o da un tag
(`for-each-ref --contains <HEAD> refs/heads/ refs/remotes/ refs/tags/`, senza i ref simbolici;
divisione in `amend_impact`) serve `"confirm": true`, altrimenti `ConfirmRequired` che elenca i ref
(un tag resterebbe sul commit vecchio); un commit non pubblicato si riscrive senza conferma (è nel
reflog e nel backup). "Pubblicato" si vede solo dai ref di tracciamento: un push per URL, senza
`refs/remotes/...`, non è visibile e il commit risulta non pubblicato. Gli altri branch locali e i
tag che contengono il commit riscritto finiscono in `also_on`: dopo l'`amend` restano sulla storia
vecchia. Un `amend` che produce lo stesso commit (stesso albero, messaggio, autore e date: es. due
`amend` uguali nello stesso secondo) è un successo senza cambiamenti: `commit.id == previous`,
`backup` `null`. Prima di scrivere, con `amend`, un ref di backup sul commit riscritto (sotto).
Risultato: `{"commit": Commit, "previous": oid | null, "branch": string | null, "backup": string |
null, "also_on": [string], "status": Status}`: il commit come in `repo.log`, HEAD prima, il branch
spostato con il nome completo (`refs/heads/main`; `null` con HEAD staccata), il ref di backup (solo
`amend`), gli altri branch e tag rimasti sul commit riscritto (solo `amend`, altrimenti vuoto) e il
nuovo stato. Un problema **dopo** un commit riuscito (HEAD spostato da un hook `post-commit` o da un
altro processo, reflog che non torna, stato non rileggibile) è `PostCommit { commit, backup, reason
}`: il commit non va ripetuto, `commit` è il commit creato se noto (`null` se HEAD non è più
quello), `backup` il ref del commit riscritto, che resta.

## CliBackend

`git -c i18n.commitEncoding=UTF-8 commit --no-edit -F - [--amend]` con `GIT_EDITOR=:`:
- hook (`pre-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit`, `post-rewrite`) e
  firma (`commit.gpgSign`, `gpg.format`, `user.signingKey`) seguono la configurazione: mai
  `--no-verify`, `--no-gpg-sign`, `--allow-empty` o `--allow-empty-message`;
- nessun editor (`--no-edit` e `GIT_EDITOR=:`, il valore con cui git non ne avvia): così
  `core.editor`, `commit.verbose`, `commit.template` (con `-F` comunque ignorato) e
  `commit.status` non possono aprire un editor che aspetterebbe un terminale;
- la pulizia del messaggio è quella di `commit.cleanup`; senza configurazione, con `-F` git
  usa `whitespace` (le righe `#` restano: `#123 corregge` è un messaggio valido). Se la
  pulizia lo rende vuoto (es. `strip` e solo commenti) git rifiuta: `InvalidParams`;
- il messaggio arriva da JSON ed è UTF-8: `i18n.commitEncoding=UTF-8` evita che una
  configurazione diversa lo etichetti con un'altra codifica;
- un hook che rifiuta o una firma che non riesce: `EngineError::Git` con stderr e stdout di
  git (git manda su stderr anche l'output degli hook), nessun commit, indice invariato
  (tranne quello che l'hook stesso ha cambiato: un `pre-commit` che fa `git add` cambia
  l'indice, e se riesce quello che viene committato, come con `git commit`). Indice
  bloccato: `IndexLocked`;
- hook e firma possono essere lenti: il comando resta sincrono (§6), stdout e stderr si
  leggono insieme e stdin si scrive da un altro thread, quindi un hook che stampa molto non
  blocca. Nessun timeout: una pinentry grafica di gpg o la conferma di una chiave ssh
  aspettano legittimamente l'utente; senza terminale né pinentry grafica gpg fallisce subito.
  Se l'app è avviata da un terminale, una pinentry testuale può usare quel terminale;
- un hook che lascia processi in background con stdout o stderr aperti fa aspettare il
  comando finché quei processi non li chiudono (git e il backend leggono fino alla fine);
- una firma ssh con chiave protetta da passphrase (senza agent) e un hook che legge
  `/dev/tty` possono aspettare sul terminale di controllo, se c'è (CLI o server MCP avviati
  da un terminale); l'app desktop di solito non ne ha. Nessun timeout, per scelta (vedi
  sopra);
- ambiente: oltre alle variabili di §5 si tolgono `GIT_AUTHOR_DATE` e `GIT_COMMITTER_DATE`
  ereditate (un'app avviata da un hook darebbe a ogni commit la data di allora); nome ed
  email (`GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_*`, `EMAIL`) restano, come per
  git da terminale: sono l'identità scelta dall'utente;
- dopo il commit il backend rilegge HEAD e controlla che l'ultima voce del reflog del ref
  aggiornato (il branch, o HEAD se staccata) porti da `previous` al nuovo commit
  (`<ref>@{1}` e `<ref>@{0}`): così si scopre anche un altro processo che ha fatto `amend`
  fra la lettura di `previous` e `git commit` (git avrebbe riscritto quell'altro commit, non
  quello salvato nel backup). Controlla poi i genitori (con `amend` gli stessi del commit
  riscritto). Se qualcosa non torna: `PostCommit` (il commit resta). Limite: senza reflog
  (`core.logAllRefUpdates=false` e nessun file di log per il ref) resta solo il controllo
  sui genitori. Dentro `git commit` il ref si aggiorna con il valore vecchio letto da git,
  quindi uno spostamento durante gli hook fa fallire il commit.

## Backup del commit riscritto (`amend`)

`commit.create` con `amend` sostituisce HEAD con un commit nuovo: il vecchio resta solo nel
reflog, che scade. Prima di eseguire `git commit --amend` il backend crea
`refs/project/backup/amend-<secondi>-<nanosecondi>` sul commit riscritto (`update-ref <ref>
<id> ""`, solo se non esiste, come per l'indice) e lo restituisce in `"backup"`; se il ref non
si crea non si committa. Se poi `git commit` fallisce e HEAD è ancora il commit letto prima, il
ref si toglie (`update-ref -d <ref> <id>`, che lo toglie solo se punta ancora a quel commit):
non c'è niente da recuperare. Resta una finestra breve fra la rilettura di HEAD e
`update-ref -d` in cui un altro processo potrebbe spostare HEAD: il backup sparirebbe, ma il
commit resterebbe nel reflog. Se invece HEAD è cambiato (un hook o un altro processo) il
backup resta e l'errore lo nomina; lo nomina anche `PostCommit` dopo un commit riuscito. Un
`amend` che produce lo stesso commit toglie il backup (punterebbe al commit attuale). Un commit nuovo non
salva backup: il commit precedente è il suo genitore. Il `MemoryBackend` registra i backup in
`MemoryBackend::commit_backups`.

Per il diario (S22) `commit.create` restituisce già i valori prima e dopo del ref spostato:
`branch` (o `null` con HEAD staccata), `previous` e `commit.id`.
