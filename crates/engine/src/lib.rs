//! L5 — motore indipendente dalla UI.
//!
//! Ogni operazione sul repository passa da [`Registry::run`], usato allo stesso modo da UI,
//! CLI, server MCP e test. I repository git si leggono tramite i trait [`GitBackend`] e
//! [`RepoOpener`]; la [`Session`] tiene il repo aperto.

mod builtin;
mod commands;
mod git;
mod job;
mod journal;
mod platform;
mod session;

pub use builtin::DEFAULT_LOG_LIMIT;
pub use commands::{CommandSpec, Handler, Registry};
pub use git::{
    BACKUP_PREFIX, BranchDeleted, CommitOutcome, CommitRequest, ConfigEntry, DiffSpec,
    DiscardOutcome, DiscardedFile, FetchFailure, FetchOutcome, FetchRequest, GitBackend,
    HEADS_PREFIX, HeadState, LinePatch, LineSelection, LogQuery, MemoryBackend, MemoryOpener,
    RefMove, RefRestore, RepoOpener, RestoreOutcome, StagedVersion, StashedFile, WorktreeBackup,
    WorktreeRestore, WorktreeStash, amend_impact, apply_patch, backup_message, branch_ref,
    branch_short, check_amend_published, check_branch_name, check_commit_state,
    check_new_branch_name, check_no_operation, check_remote_name, check_same_plan, cleanup_message,
    default_remote, discard_description, discard_plan, file_digest, line_patch, path_matches,
    split_message, unknown_remote, valid_ref_name, versions_at_risk, versions_at_risk_hidden,
};
pub use git::{
    INDEX_NOT_RESTORED, STASH_REF, StashApplied, StashApply, StashDrop, StashDropped, StashEntry,
    StashPopped, StashPush, StashRecord, check_stash_apply, check_stash_push, find_stash,
    pop_stash, stash_blocked, stash_branch, stash_local_changes,
};
pub use git::{
    MergeAborted, MergeMode, MergeOutcome, MergeRequest, MergeSource, MergeSourceKind,
    OperationState, check_merge_state, check_merging, clean_merge_message, conflicted_paths,
    merge_source,
};
pub use git::{
    PullMode, PullOperation, PullOutcome, PullPolicy, PullRequest, PullSettings, PullStrategy,
    PushKind, PushOutcome, PushRejection, PushRequest, StrategySource, UpstreamConfig,
    check_pull_state, merge_message, pull_mode, tracking_ref,
};
pub use git::{
    REMOTES_PREFIX, RemoteRef, RemoteSnapshot, RemoteStillListed, check_new_remote_name,
    check_remote_url, ensure_remote_gone, mask_url, masked_entry, owned_by, remote_dependents,
    remove_remote, restore_remote, split_branch_key,
};
pub use job::{CancelToken, JobContext, Progress, ProgressSink};
pub use journal::{HeadChange, Journal, JournalEntry, RefChange};
pub use platform::{Capabilities, Picked, Platform};
pub use project_core::Invocation;
pub use session::Session;

use project_core::CoreError;
use project_domain::{DomainError, Oid};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("comando sconosciuto: `{0}`")]
    UnknownCommand(String),
    #[error("parametri non validi per `{id}`: {reason}")]
    InvalidParams { id: &'static str, reason: String },
    /// Al percorso non c'è un repository git (o non si riesce ad aprirlo).
    #[error("`{path}` non è un repository git: {reason}")]
    RepoNotFound { path: String, reason: String },
    /// Il comando ha bisogno di un repo, ma la sessione non ne ha uno aperto.
    #[error("nessun repository aperto: aprirne uno con `repo.open`")]
    NoRepo,
    /// La rev non corrisponde a nessun ref né commit (o l'id abbreviato è ambiguo).
    #[error("revisione sconosciuta: `{0}`")]
    UnknownRev(String),
    /// Il programma `git` non si avvia (non installato o non nel `PATH`).
    #[error("impossibile eseguire `git`: {0}")]
    GitNotFound(String),
    /// Il `git` installato è più vecchio della versione minima supportata.
    #[error("git {found} non è supportato: serve almeno la versione {required}")]
    GitTooOld { found: String, required: String },
    /// Un comando git è terminato con errore; `stderr` è il suo messaggio.
    #[error("`git {command}` non riuscito: {stderr}")]
    Git {
        /// Il sottocomando git, es. `status`.
        command: String,
        stderr: String,
    },
    /// L'indice è bloccato da un altro processo git: il file `lock` (di solito
    /// `.git/index.lock`) esiste. Non si riprova e non si cancella il lock: se nessun git è in
    /// esecuzione è rimasto da un crash e va tolto a mano.
    #[error(
        "l'indice è bloccato: `{lock}` esiste. Un altro processo git sta lavorando sul repository; \
         riprovare quando ha finito. Se nessun git è in esecuzione il file è rimasto da un crash \
         e va rimosso a mano"
    )]
    IndexLocked { lock: String },
    /// L'operazione è rifiutata nello stato attuale del repo (es. stage di un file ignorato,
    /// unstage di un file in conflitto); niente è stato modificato.
    #[error("operazione rifiutata: {0}")]
    NotAllowed(String),
    /// Il diff da cui vengono le righe scelte (`index.stage_lines`, `index.unstage_lines`) non
    /// è più quello attuale: il file è cambiato da quando il diff è stato letto (o non ha più
    /// modifiche). Niente è stato modificato; va riletto il diff e rifatta la selezione.
    #[error(
        "il diff di `{path}` non è più quello letto ({reason}): niente modificato, rileggere il \
         diff e rifare la selezione"
    )]
    StaleDiff { path: String, reason: String },
    /// `commit.create` senza niente da committare: l'indice è uguale a HEAD (o, su un branch
    /// senza commit, è vuoto) e non c'è un merge in corso. Niente è stato modificato.
    #[error(
        "niente da committare: l'indice è uguale a HEAD. Mettere in stage le modifiche \
         (`index.stage`), oppure usare `amend` per cambiare solo il messaggio dell'ultimo commit"
    )]
    NothingToCommit,
    /// L'operazione può far perdere lavoro o riscrive qualcosa di già pubblicato e serve
    /// `"confirm": true` nei parametri (§1). `reason` dice cosa succederebbe. Niente è stato
    /// modificato.
    #[error(
        "`{command}` chiede una conferma: {reason}. Niente modificato; per procedere ripetere \
         il comando con `\"confirm\": true`"
    )]
    ConfirmRequired {
        command: &'static str,
        reason: String,
        /// L'id completo (40 cifre) da ripetere in `"expected"` per confermare: per il force di
        /// `remote.push` (il branch remoto locale letto ora) e per `stash.drop` (lo stash da
        /// eliminare), che è anche nel testo di `reason`; `None` per gli altri comandi.
        expected: Option<String>,
    },
    /// Il commit è **riuscito**, ma dopo qualcosa non torna: HEAD spostato da un hook
    /// `post-commit` o da un altro processo, `previous` non verificabile, stato non rileggibile.
    /// Non va ripetuto: `commit` è il commit creato (se noto), `backup` il ref del commit
    /// riscritto da `amend` (se c'è), che resta.
    #[error("{}", post_commit_text(.commit, .backup, .reason))]
    PostCommit {
        commit: Option<Oid>,
        backup: Option<String>,
        reason: String,
    },
    /// `edit.undo`/`edit.redo`: HEAD o i ref non sono più come li ha lasciati l'operazione da
    /// annullare (o l'undo da rifare): qualcuno li ha spostati da fuori (un `git commit` o
    /// `git reset` da terminale, un altro programma). Niente è stato modificato e la voce
    /// resta nel diario; `refs` dice cosa è cambiato, una riga per ref.
    #[error(
        "`{command}` rifiutato: i ref sono cambiati dopo l'operazione ({}). Niente modificato: \
         per non sovrascrivere lavoro fatto altrove, l'operazione va annullata a mano",
        .refs.join("; ")
    )]
    UndoConflict {
        command: &'static str,
        refs: Vec<String>,
    },
    /// `branch.checkout` (o l'undo/redo di un checkout) rifiutato da git perché sovrascriverebbe
    /// modifiche locali: file modificati (in stage o no) diversi nel branch di destinazione, o
    /// file non tracciati o ignorati che il branch di destinazione contiene. Niente è stato
    /// modificato (git controlla prima di scrivere); `files` sono i file indicati da git,
    /// `stderr` il suo messaggio. Le modifiche vanno committate, messe da parte o scartate
    /// prima del checkout.
    #[error(
        "checkout di `{target}` rifiutato: sovrascriverebbe modifiche locali ({}). Niente \
         modificato: committare, mettere da parte o scartare queste modifiche e riprovare",
        list(.files)
    )]
    CheckoutConflict {
        /// Il branch (nome breve) o il commit di destinazione.
        target: String,
        files: Vec<String>,
        stderr: String,
    },
    /// Il remoto ha rifiutato le credenziali, o servirebbe chiederle (nome utente, password,
    /// passphrase, conferma della chiave dell'host) e non c'è modo di farlo senza terminale:
    /// "Authentication failed", "could not read Username", "Permission denied (publickey)",
    /// HTTP 401/403, "Host key verification failed". Niente è stato modificato. `message` è il
    /// testo di git (stderr).
    #[error(
        "autenticazione con il remoto `{remote}` non riuscita: {message}. Niente modificato: \
         controllare le credenziali (credential helper, chiave SSH caricata nell'agent, host \
         noto in `known_hosts`)"
    )]
    Auth { remote: String, message: String },
    /// Il remoto non si raggiunge: l'URL punta a una cartella che non c'è o non è un
    /// repository, host sconosciuto, connessione rifiutata o interrotta, repository inesistente
    /// sul server. Niente è stato modificato. `message` è il testo di git (stderr).
    #[error("il remoto `{remote}` non è raggiungibile: {message}. Niente modificato")]
    RemoteUnreachable { remote: String, message: String },
    /// Il comando è stato annullato da chi lo ha lanciato
    /// ([`CancelToken::cancel`](crate::CancelToken::cancel)). Quello che il comando dice di sé
    /// vale anche qui (es. `remote.fetch` annullato: i ref già aggiornati restano, nessun ref
    /// locale toccato).
    #[error("`{command}` annullato")]
    Cancelled { command: &'static str },
    /// `remote.pull`: il rebase dell'upstream si è fermato in conflitto. Il backend l'ha
    /// **annullato** (`git rebase --abort`): branch, indice e worktree sono come prima del pull
    /// (che partiva da un worktree pulito), il fetch resta fatto. `files` sono i file in
    /// conflitto, `message` il testo di git. Per integrare le modifiche risolvendo i conflitti,
    /// per ora, git da terminale o la strategia merge (finché S32 non porta
    /// `rebase.continue`/`rebase.abort`). Un **merge** fermo invece non è un errore: resta in
    /// corso ([`PullOutcome::in_progress`](crate::PullOutcome::in_progress)).
    #[error(
        "pull di `{branch}` interrotto: il {operation} con `{upstream}` ha conflitti in {}. Il \
         {operation} è stato annullato e il branch è com'era (il fetch è fatto): per integrare \
         `{upstream}` risolvendo i conflitti, `git {operation} {upstream}` da terminale, oppure \
         usare l'altra strategia (`strategy`)",
        list(.files)
    )]
    PullConflict {
        branch: String,
        /// Il branch remoto (nome breve, es. `origin/main`).
        upstream: String,
        operation: PullOperation,
        files: Vec<String>,
        message: String,
    },
    /// `remote.push` rifiutato: niente è cambiato, né sul remoto né in locale (il branch remoto
    /// locale resta com'era). `branch` è il branch sul remoto, `message` il testo di git.
    #[error(
        "push di `{branch}` su `{remote}` rifiutato: {}. Niente modificato ({message})",
        .reason.advice()
    )]
    PushRejected {
        remote: String,
        branch: String,
        reason: PushRejection,
        message: String,
    },
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Domain(#[from] DomainError),
}

impl EngineError {
    /// [`EngineError::NotAllowed`] per lo stage di file ignorati chiesti per nome: niente è
    /// stato messo in stage.
    pub fn ignored(paths: &[impl std::fmt::Display]) -> Self {
        EngineError::NotAllowed(format!(
            "file ignorati da .gitignore (o da un altro file di esclusione), niente messo in \
             stage: {}",
            list(paths)
        ))
    }

    /// [`EngineError::NotAllowed`] per l'unstage di file in conflitto: niente è stato tolto
    /// dallo stage.
    pub fn unmerged(paths: &[impl std::fmt::Display]) -> Self {
        EngineError::NotAllowed(format!(
            "file in conflitto, niente tolto dallo stage (si perderebbero le versioni del \
             conflitto; per risolverlo va messo in stage): {}",
            list(paths)
        ))
    }
}

impl EngineError {
    /// [`EngineError::NotAllowed`] per lo stage di una cartella con file in conflitto: un
    /// conflitto si risolve solo chiedendo il file per nome. Niente è stato messo in stage.
    pub fn unmerged_in_folder(paths: &[impl std::fmt::Display]) -> Self {
        EngineError::NotAllowed(format!(
            "la cartella contiene file in conflitto, niente messo in stage (metterli in stage \
             li segnerebbe come risolti: vanno chiesti per nome): {}",
            list(paths)
        ))
    }
}

impl EngineError {
    /// [`EngineError::InvalidParams`] di `commit.create` per un messaggio che la pulizia di git
    /// (`commit.cleanup`, es. `strip` che toglie i commenti) ha reso vuoto: niente committato.
    pub fn empty_commit_message() -> Self {
        EngineError::InvalidParams {
            id: "commit.create",
            reason: "il messaggio è vuoto dopo la pulizia di git (`commit.cleanup` toglie le \
                     righe di commento e gli spazi), niente committato: scrivere un messaggio \
                     che non sia solo commenti"
                .to_owned(),
        }
    }
}

fn post_commit_text(commit: &Option<Oid>, backup: &Option<String>, reason: &str) -> String {
    let mut text = match commit {
        Some(id) => format!("il commit {id} è stato creato, ma {reason}"),
        None => format!("il commit è stato creato, ma {reason}"),
    };
    if let Some(backup) = backup {
        text.push_str(&format!(
            "; il commit riscritto da `amend` è salvato in `{backup}`"
        ));
    }
    text.push_str(": non ripetere il commit, rileggere stato e log");
    text
}

fn list(paths: &[impl std::fmt::Display]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
