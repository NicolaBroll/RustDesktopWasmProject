# Client git: roadmap a step

Architettura in [ARCHITETTURA-git.md](ARCHITETTURA-git.md). Ogni step è piccolo, si chiude con
`cargo xtask ci` verde e lascia il progetto funzionante. Il campo **Obiettivo** si può usare così
com'è come obiettivo di una sessione; **Fatto quando** dice come verificarlo. I rimandi `§` sono alle sezioni dell'architettura.

Regole valide per tutti gli step:

- Lavorare solo sui crate indicati; se serve toccarne altri, dirlo prima.
- Nuove dipendenze nel `[workspace.dependencies]` del `Cargo.toml` radice, verificando la
  versione corrente.
- Aggiornare la casella dello step in questo file quando è finito.
- Gli step che scrivono nel repo dell'utente (da S18 in poi) passano da `safety-reviewer` prima
  della consegna.
- Consegna di ogni step: `cargo xtask ci` verde in locale, pull request verso `dev` e squash
  merge. La CI di GitHub per ora gira solo sulle pull request verso `master`, non su quelle verso
  `dev`. Lo step successivo riparte dal `dev` aggiornato; `dev` andrà in `master` con una pull
  request.

## Fase 0 — Preparazione

### S00 — CI sul branch principale
- [x] **Agente:** `git-engine`
- **Obiettivo:** il workflow `.github/workflows/ci.yml` ascolta i push su `main`, ma il branch
  principale del repo è `master`. Farlo partire sui push a `master`.
- **Fatto quando:** il trigger `push` di `ci.yml` indica `master`.

## Fase 1 — Leggere un repo

### S01 — Tipi git nel dominio
- [x] **Agente:** `git-domain`
- **Obiettivo:** aggiungere in `crates/domain` un modulo `git` con `Oid` (parsing ed esadecimale,
  `Display`, `serde` come stringa), `Signature`, `Commit`, `RefKind`, `RefInfo`, `Upstream`,
  `Head`. L'esempio "lista di voci" resta com'è.
- **Fatto quando:** test su parsing di `Oid` validi e non validi e round-trip JSON; CI verde.

### S02 — Parser del log
- [x] **Agente:** `git-domain`
- **Obiettivo:** funzione pura `parse_log(&[u8]) -> Result<Vec<Commit>, DomainError>` per il
  formato `-z` a campi fissi descritto in §3, più la costante con la stringa `--format`.
- **Fatto quando:** test con output catturato da un repo vero (commit di merge, commit radice,
  corpo vuoto, corpo su più righe, autore non UTF-8); CI verde.

### S03 — Parser dello stato
- [x] **Agente:** `git-domain`
- **Obiettivo:** `Status`, `StatusEntry`, `Operation` e `parse_status(&[u8])` per
  `git status --porcelain=v2 -z --branch --untracked-files=all`.
- **Fatto quando:** test su file modificati, in stage, rinominati, non tracciati, in conflitto,
  HEAD staccata, branch senza upstream, ahead/behind; CI verde.

### S04 — Parser dei ref
- [x] **Agente:** `git-domain`
- **Obiettivo:** formato di `git for-each-ref` (costante) e `parse_refs(&[u8])` per branch locali,
  remoti, tag e stash, con upstream e ahead/behind.
- **Fatto quando:** test con output catturato; CI verde.

### S05 — Trait del backend e `MemoryBackend`
- [x] **Agente:** `git-engine`
- **Obiettivo:** in `crates/engine` i trait `GitBackend` (sola lettura: `head`, `refs`, `log`,
  `status`) e `RepoOpener`, più `MemoryBackend` con un costruttore di repo di esempio (una
  dozzina di commit con due branch e un merge).
- **Fatto quando:** test che leggono il repo di esempio dal `MemoryBackend`; CI verde.

### S06 — Crate `project-git` con `CliBackend` in lettura
- [x] **Agente:** `git-engine`
- **Obiettivo:** nuovo crate `crates/git` (`project-git`, L6, dichiarato in
  `xtask/src/layers.rs`) con `CliBackend` e `CliOpener` che implementano i trait di S05 eseguendo
  `git` secondo le regole di §5 e usando i parser di S02–S04. Controllo della versione minima.
- **Fatto quando:** test di integrazione su un repo temporaneo creato dal test (vedi §9);
  `cargo xtask layers` passa; CI verde.

### S07 — Helper per gli scenari di test
- [x] **Agente:** `git-scenarios`
- **Obiettivo:** in `crates/git/tests/common/` un helper che crea repo temporanei in ambiente
  deterministico (§9) con funzioni tipo `commit_file`, `branch`, `merge`, `conflict`. Riscrivere
  i test di S06 con l'helper.
- **Fatto quando:** i test usano l'helper e passano due volte di fila con lo stesso risultato;
  CI verde.

### S08 — Sessione del repo e comandi di lettura
- [x] **Agente:** `git-engine`
- **Obiettivo:** `Session` contiene il backend aperto e il `RepoOpener` iniettato. Comandi
  `repo.open`, `repo.status`, `repo.refs`, `repo.log` (paginato). Rimuovere l'esempio "lista di
  voci" (`Document`, `item.*`, `document.*`): è un cambio voluto del catalogo, da annotare nel
  commit. Aggiornare UI e app quanto basta perché compilino (la UI mostra un messaggio
  provvisorio).
- **Fatto quando:** test dei comandi sul `MemoryBackend`; `project-cli commands` mostra il nuovo
  catalogo; CI verde.

### S09 — CLI e MCP sul repo
- [x] **Agente:** `git-engine`
- **Obiettivo:** `project-cli --repo <percorso> run --cmd repo.status` e `eval` usano
  `CliOpener`; `mcp --root <dir>` apre il repo nella radice e confina i percorsi lì.
  Sostituire `open_document`/`save_document` con `open_repo`. Aggiornare README e `AGENTS.md`.
- **Fatto quando:** test end-to-end in `apps/project-cli/tests` su un repo temporaneo; CI verde.

## Fase 2 — Vedere

### S10 — Lavoro in background
- [x] **Agente:** `egui-ui`
- **Obiettivo:** `Platform::spawn` (§6) con implementazione a thread sul desktop ed esecuzione
  immediata sul web. Nella UI, un esecutore che manda comandi alla sessione condivisa e riceve i
  risultati da un canale, con stato "in corso" visibile.
- **Fatto quando:** test del motore o della UI con un comando finto lento; la UI resta reattiva;
  CI verde.

### S11 — Layout principale
- [x] **Agente:** `egui-ui`
- **Obiettivo:** barra in alto, colonna sinistra con i ref, lista dei commit al centro (senza
  grafo), pannello dettagli a destra (§8). Desktop: apertura di un repo con il dialogo cartelle.
  Web: `MemoryBackend` con il repo di esempio.
- **Fatto quando:** `make run` apre un repo vero e mostra branch e commit; screenshot della build
  web con Playwright; CI verde.

### S12 — Lista dei commit virtualizzata
- [x] **Agente:** `egui-ui`
- **Obiettivo:** `ScrollArea::show_rows` con caricamento a pagine di `repo.log` durante lo
  scorrimento; selezione con mouse e tastiera.
- **Fatto quando:** un repo con almeno 50.000 commit scorre fluido (verifica a mano, annotata nel
  commit); CI verde.

### S13 — Corsie del grafo
- [x] **Agente:** `git-domain`
- **Obiettivo:** algoritmo delle corsie (§3) come funzione pura, con test di proprietà
  (`proptest`) sugli invarianti e casi fissi (lineare, branch, merge, ottopus merge, più radici).
- **Fatto quando:** test passano; CI verde.

### S14 — Disegno del grafo
- [x] **Agente:** `egui-ui`
- **Obiettivo:** disegnare corsie e archi accanto alla lista dei commit, con un colore per corsia
  e le etichette dei ref sulla riga del commit.
- **Fatto quando:** screenshot web del repo di esempio con merge visibile; CI verde.

### S15 — Dettagli del commit
- [x] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** comando `commit.show` (metadati e file cambiati con `git diff-tree`) e pannello
  dettagli nella UI.
- **Fatto quando:** test del comando su repo temporaneo; il pannello mostra i dati del commit
  selezionato; CI verde.

### S16 — Parser del diff
- [x] **Agente:** `git-domain`
- **Obiettivo:** `Diff`, `FileDiff`, `Hunk`, `Line` e `parse_diff(&[u8])` per unified diff,
  con rinomine, file nuovi ed eliminati, binari, "no newline at end of file".
- **Fatto quando:** test con output catturati; CI verde.

### S17 — Vista del diff
- [x] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** comandi `diff.worktree`, `diff.index`, `diff.commit`; vista con numeri di riga,
  colori per aggiunte e rimozioni, intestazioni degli hunk.
- **Fatto quando:** test dei comandi; il diff si apre dal pannello dettagli; CI verde.

## Fase 3 — Modificare in locale

### S18 — Stage e unstage di file
- [x] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** `index.stage`, `index.unstage` e area di stage nella UI (riga "modifiche non
  committate" in cima alla lista).
- **Fatto quando:** test di integrazione; revisione di `safety-reviewer`; CI verde.

### S19 — Costruttore di patch
- [x] **Agente:** `git-domain`
- **Obiettivo:** costruttore di patch per righe selezionate (§3), in entrambe le direzioni
  (stage e unstage).
- **Fatto quando:** snapshot `insta` e test che applicano la patch con `git apply --check` su un
  repo temporaneo; CI verde.

### S20 — Stage per hunk e per riga
- [x] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** `index.stage_lines`, `index.unstage_lines`: patch del costruttore applicata
  da un applicatore rigoroso nel motore (ogni hunk solo alla sua posizione) e scritta con
  `hash-object --no-filters` / `update-index --index-info`; selezione di hunk e righe nella
  vista del diff.
- **Fatto quando:** test di integrazione su file con più hunk; revisione di `safety-reviewer`;
  CI verde.

### S21 — Commit
- [x] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** `commit.create` (messaggio via stdin, opzione `amend`), con hook e firma
  secondo la configurazione dell'utente; casella del messaggio nell'area di stage.
- **Fatto quando:** test di integrazione, anche con un hook `pre-commit` che fallisce; CI verde.

### S22 — Diario delle operazioni e undo
- [x] **Agente:** `git-engine`
- **Obiettivo:** diario (§7) per i comandi che spostano ref; `edit.undo` e `edit.redo` con
  controllo che i ref non siano cambiati nel frattempo.
- **Fatto quando:** test: commit poi undo riporta il branch al commit precedente; undo rifiutato
  se il ref è stato spostato da fuori; revisione di `safety-reviewer`; CI verde.

### S23 — Scartare modifiche con backup
- [x] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** `worktree.discard` con `confirm`, backup `git stash create` e ref di backup
  (§7), annullabile con `edit.undo`; conferma nella UI.
- **Fatto quando:** test: scarta, poi undo ripristina il file identico; senza `confirm` non
  succede nulla; revisione di `safety-reviewer`; CI verde.

### S24 — Branch
- [x] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** `branch.create`, `branch.checkout` (errore chiaro con modifiche in conflitto),
  `branch.delete` (`confirm` se non unito), `branch.rename`; menu contestuale nella colonna dei
  ref.
- **Fatto quando:** test di integrazione, anche con nomi che iniziano con `-`; revisione di
  `safety-reviewer`; CI verde.

## Fase 4 — Remoti

### S25 — Fetch
- [x] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** `remote.fetch` in background con avanzamento da stderr (`--progress`) ed errori
  di autenticazione riconosciuti; pulsante nella barra.
- **Fatto quando:** test con un remoto locale (`git init --bare` in cartella temporanea); CI verde.

### S25b — Grafo di tutti i ref
- [x] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** il grafo mostra anche i commit dei branch non uniti e quelli appena scaricati
  su `origin/*`: `repo.log {"all": true}` parte da branch locali e remoti, tag e HEAD (anche
  staccata), senza stash né backup; la UI lo usa per il grafo.
- **Fatto quando:** test di integrazione (branch non unito, branch remoto dopo un fetch, stash e
  backup esclusi, HEAD staccata, pagine contigue) e sul `MemoryBackend`; CI verde.

### S26 — Pull e push
- [ ] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** `remote.pull` (strategia secondo la configurazione: merge o rebase) e
  `remote.push` (impostazione dell'upstream al primo push; `--force-with-lease` solo con
  `confirm`).
- **Fatto quando:** test con remoto locale, anche push rifiutato perché non fast-forward;
  revisione di `safety-reviewer`; CI verde.

### S27 — Gestione dei remoti
- [ ] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** `remote.list`, `remote.add`, `remote.remove`; ahead/behind visibili accanto ai
  branch.
- **Fatto quando:** test di integrazione; CI verde.

## Fase 5 — Funzioni avanzate

### S28 — Stash
- [ ] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** `stash.push`, `stash.apply`, `stash.pop`, `stash.drop` (`confirm`); stash nella
  colonna sinistra e nel grafo.
- **Fatto quando:** test di integrazione; revisione di `safety-reviewer`; CI verde.

### S29 — Merge e stato "in corso"
- [ ] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** `merge.start`, `merge.abort`, `merge.continue`; banner nella UI quando il repo è
  in merge, rebase, cherry-pick o revert, con i file in conflitto.
- **Fatto quando:** test con lo scenario di conflitto di S07; CI verde.

### S30 — Editor dei conflitti
- [ ] **Agente:** `egui-ui`, con `git-domain` per il parser dei marcatori
- **Obiettivo:** vista a tre parti (base, nostra, loro) e risultato modificabile; scelta per
  blocco; segna come risolto con `index.stage`.
- **Fatto quando:** test del parser dei marcatori di conflitto; risoluzione completa di un
  conflitto dalla UI; CI verde.

### S31 — Cherry-pick e revert
- [ ] **Agente:** `git-engine`, poi `egui-ui`
- **Obiettivo:** `commit.cherry_pick`, `commit.revert` dal menu contestuale del commit, con
  gestione dei conflitti di S29.
- **Fatto quando:** test di integrazione; CI verde.

### S32 — Rebase interattivo
- [ ] **Agente:** `git-domain` (piano), `git-engine` (esecuzione)
- **Obiettivo:** piano di rebase (pick, reword, squash, fixup, drop, riordino) come dati puri;
  esecuzione con `GIT_SEQUENCE_EDITOR` che scrive il piano; `rebase.abort` e `rebase.continue`.
- **Fatto quando:** test di integrazione per ogni azione del piano; revisione di
  `safety-reviewer`; CI verde.

### S33 — Rebase con trascinamento
- [ ] **Agente:** `egui-ui`
- **Obiettivo:** riordinare, unire ed eliminare commit trascinandoli nella lista, con anteprima
  del piano prima di eseguire.
- **Fatto quando:** screenshot web con il piano in anteprima; il rebase eseguito dalla UI dà lo
  stesso risultato del test di S32; CI verde.

### S34 — Blame e cronologia di un file
- [ ] **Agente:** `git-domain` (parser `--porcelain`), poi `git-engine` e `egui-ui`
- **Obiettivo:** `file.blame`, `file.log` (con `--follow`) e relative viste.
- **Fatto quando:** test del parser e dei comandi; CI verde.

### S35 — Più repo e repo recenti
- [ ] **Agente:** `egui-ui`
- **Obiettivo:** schede con più repo aperti; elenco dei repo recenti salvato tra un avvio e
  l'altro.
- **Fatto quando:** riapertura dell'app con le schede precedenti; CI verde.

### S36 — Repo enormi con `gix`
- [ ] **Agente:** `git-engine`
- **Obiettivo:** misurare `repo.log` sul kernel Linux; se lento, leggere il log con `gix` nel
  `CliBackend` mantenendo lo stesso risultato.
- **Fatto quando:** confronto dei tempi annotato nel commit; i test di integrazione danno lo
  stesso risultato con le due letture; CI verde.

### S37 — Packaging Linux
- [ ] **Agente:** `git-engine`
- **Obiettivo:** AppImage e pacchetto deb (vedi anche [ROADMAP.md](../ROADMAP.md)) con
  `SHA256SUMS.txt`.
- **Fatto quando:** il pacchetto si installa e avvia su Ubuntu pulito; CI verde.
