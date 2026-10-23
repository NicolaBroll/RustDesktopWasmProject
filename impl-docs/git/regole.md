# Regole in breve e checklist di sicurezza

Da leggere **sempre** prima di toccare il client git, al posto di tutta
[ARCHITETTURA-git.md](../ARCHITETTURA-git.md): quella e i file per funzione di questa cartella
si aprono solo per la parte che si tocca (il paragrafo citato qui, il file della funzione).

## Invarianti

- Ogni operazione è un comando del `Registry` con id stabile (mai rinominato) e parametri JSON
  `deny_unknown_fields`; chi scrive va in `WRITES`. UI, CLI, MCP e test passano da
  `Registry::run`.
- **Non perdere dati** (§1, §7): ciò che può cancellare lavoro chiede `"confirm": true` e salva
  prima un backup in `refs/project/backup/<tipo>-...`; se il backup non riesce non si scrive.
  Stage/unstage salvano da soli il backup dell'indice, senza `confirm`.
- Chi sposta ref registra una `JournalEntry` (HEAD e ref prima/dopo); undo e redo rifiutano se
  i ref sono cambiati da fuori o se c'è un'operazione in corso (merge, rebase, cherry-pick,
  revert, bisect, am).
- **Chiamate a git** (§5): mai una shell; `--end-of-options` prima di rev e nomi dall'utente,
  `--` e `--literal-pathspecs` per i percorsi (liste via stdin con `--pathspec-file-nul`, mai
  vuote); messaggi via stdin o file; `GIT_TERMINAL_PROMPT=0`, `LC_ALL=C`; niente `--autostash`,
  `--force` (solo `--force-with-lease` con l'oid atteso), `git pull`.
- I percorsi restano byte (`RepoPath`): `from_utf8_lossy` solo per il testo mostrato.
- Ogni comando: test sul `MemoryBackend`; ogni metodo del `CliBackend`: test di integrazione in
  `crates/git/tests` con gli scenari di `common/`.

## Checklist prima di consegnare una scrittura

Sono i problemi trovati più spesso dalle revisioni: controllali prima, uno per uno.

1. **git fallisce a metà.** Un comando che esce con errore può aver già scritto una parte dei
   file o spostato un ref (cartella in sola lettura, file aperto, lock). Un backup **non si
   toglie mai** dopo un fallimento se HEAD, il branch o l'operazione in corso sono cambiati o
   non si può escludere che lo siano; l'errore nomina il backup.
2. **Configurazione dell'utente.** Opzioni che cambiano il comportamento del comando
   (`branch.<b>.mergeOptions`, `pull.*`, `merge.ff`, `commit.gpgsign`, `core.hooksPath`,
   `rerere.autoUpdate`...): passare le opzioni esplicite che servono (`--no-squash`,
   `--commit`, ...) e dopo verificare che il ref sia davvero dove deve essere; se no errore,
   non successo.
3. **File non tracciati, ignorati, `--assume-unchanged`, `--skip-worktree`.** Non devono essere
   sovrascritti né persi; `git add` di un percorso ignorato vuole `-f` (con percorsi espliciti).
4. **Gare.** Fra lettura e scrittura un altro processo può spostare ref o cambiare file: passare
   a git il valore atteso quando si può (`update-ref <nuovo> <vecchio>`, lease) e dopo
   ricontrollare; il diario registra solo il risultato atteso, altrimenti avviso in `warnings`.
5. **Hook e firma** che falliscono (`pre-commit`, `commit-msg`, `pre-merge-commit`,
   `post-checkout`, `gpg.program=false`): lo stato lasciato da git va riportato com'è, con il
   testo di git.
6. **Nomi non UTF-8**, con spazi, che iniziano con `-`: nessun doppione né alterazione.
7. **Operazione in corso o HEAD staccata**: rifiutare o gestire in modo esplicito.
8. **Test** per: il caso senza `confirm`, il ripristino dal backup, il fallimento dopo il backup
   e, dove si riesce a provocarlo, ognuno dei punti sopra. Un test che dipende dai permessi si
   salta se la cartella resta scrivibile (root).

## Costo: output e resoconti

- Durante lo sviluppo: `cargo test -q -p <crate> [--test <file>] [<filtro>]`; alla fine una
  sola volta `cargo xtask ci --quiet` (una riga per passo, l'output solo del passo che fallisce).
- Leggi i file grandi a pezzi (`grep -n` per trovare il punto, poi le righe che servono).
- Resoconto finale breve: al massimo 15-20 righe con cosa cambia nel contratto (id, parametri,
  campi del risultato), file toccati, scelte non ovvie, esito della CI.
