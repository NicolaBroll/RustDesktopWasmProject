//! Il repo di esempio di [`MemoryBackend::sample`].

use super::MemoryBackend;
use project_domain::{
    Change, Commit, EntryKind, Oid, RepoPath, Signature, StatusEntry, TrackedInfo,
};

/// Data del primo commit (14 novembre 2023); i successivi sono a un'ora l'uno dall'altro.
const START: i64 = 1_700_000_000;
const HOUR: i64 = 3_600;
const FILE_MODE: u32 = 0o100644;

/// Seed degli id finti: commit `n` → `n`, oggetti tag e file da qui in poi.
const TAG_SEED: u64 = 1_000;
const BLOB_SEED: u64 = 2_000;

struct Person {
    name: &'static str,
    email: &'static str,
    offset_min: i32,
}

const ADA: Person = Person {
    name: "Ada Rossi",
    email: "ada@example.com",
    offset_min: 60,
};
const BRUNO: Person = Person {
    name: "Bruno Verdi",
    email: "bruno@example.com",
    offset_min: -300,
};

/// Commit dell'esempio: (numero, genitori, autore, oggetto, corpo). Il numero dà il seed
/// dell'id e la data (`START + (n - 1) ore`).
const COMMITS: &[(u64, &[u64], &Person, &str, &str)] = &[
    (1, &[], &ADA, "Primo commit", ""),
    (2, &[1], &ADA, "Aggiungi il README", ""),
    (
        3,
        &[2],
        &BRUNO,
        "Struttura del progetto",
        "Cartelle crates/ e apps/, workspace Cargo.",
    ),
    (4, &[3], &ADA, "Parser del log", ""),
    (5, &[4], &BRUNO, "Corsie del grafo: prima versione", ""),
    (6, &[4], &ADA, "Correggi un refuso nel README", ""),
    (
        7,
        &[5],
        &BRUNO,
        "Corsie: riusa le colonne libere",
        "Una corsia che si chiude torna disponibile per la diramazione successiva.\n\n\
         Così il grafo resta stretto anche con molti branch.",
    ),
    (8, &[6], &ADA, "Parser dello stato", ""),
    (9, &[7], &BRUNO, "Test di proprietà per le corsie", ""),
    (10, &[8, 9], &ADA, "Merge branch 'feature/grafo'", ""),
    (11, &[10], &ADA, "Parser dei ref", ""),
    (12, &[11], &BRUNO, "Pannello dei commit", ""),
];

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn signature(who: &Person, time: i64) -> Signature {
    Signature {
        name: who.name.to_owned(),
        email: who.email.to_owned(),
        time,
        offset_min: who.offset_min,
    }
}

fn tracked(path: &str, index: Change, worktree: Change, head_mode: u32, seed: u64) -> StatusEntry {
    let in_head = head_mode != 0;
    let oid_head = if in_head {
        oid(seed)
    } else {
        Oid::from_bytes([0; Oid::LEN])
    };
    // Con una modifica in stage l'indice ha un contenuto diverso da HEAD.
    let oid_index = if index.is_change() {
        oid(seed + 1)
    } else {
        oid_head
    };
    StatusEntry {
        path: RepoPath::from(path),
        index,
        worktree,
        kind: EntryKind::Ordinary(TrackedInfo {
            submodule: None,
            mode_head: head_mode,
            mode_index: FILE_MODE,
            mode_worktree: FILE_MODE,
            oid_head,
            oid_index,
        }),
    }
}

impl MemoryBackend {
    /// Repo di esempio, sempre uguale (id e date fissi):
    ///
    /// ```text
    /// * 12 Pannello dei commit            (main, HEAD)
    /// * 11 Parser dei ref                 (origin/main)
    /// *   10 Merge branch 'feature/grafo' (tag v0.1.0, annotato)
    /// |\
    /// | * 9 Test di proprietà per le corsie   (feature/grafo, origin/feature/grafo)
    /// * | 8 Parser dello stato
    /// | * 7 Corsie: riusa le colonne libere
    /// * | 6 Correggi un refuso nel README
    /// | * 5 Corsie del grafo: prima versione
    /// |/
    /// * 4 Parser del log
    /// * 3 Struttura del progetto
    /// * 2 Aggiungi il README
    /// * 1 Primo commit
    /// ```
    ///
    /// `main` segue `origin/main` ed è avanti di 1; `feature/grafo` segue
    /// `origin/feature/grafo`, allineato. Il worktree non è pulito: `README.md` modificato in
    /// stage e fuori, `docs/guida.md` aggiunto in stage, `src/lib.rs` modificato fuori stage,
    /// `note.txt` non tracciato.
    pub fn sample() -> Self {
        let mut repo = Self::new();
        for &(n, parents, who, summary, body) in COMMITS {
            let time = START + (i64::try_from(n).unwrap_or(0) - 1) * HOUR;
            repo.insert_commit(Commit {
                id: oid(n),
                parents: parents.iter().map(|&p| oid(p)).collect(),
                author: signature(who, time),
                committer: signature(who, time),
                summary: summary.to_owned(),
                body: body.to_owned(),
            });
        }
        repo.set_branch("main", oid(12));
        repo.set_branch("feature/grafo", oid(9));
        repo.set_remote_branch("origin/main", oid(11));
        repo.set_remote_branch("origin/feature/grafo", oid(9));
        repo.set_upstream("main", "origin/main");
        repo.set_upstream("feature/grafo", "origin/feature/grafo");
        repo.set_tag("v0.1.0", oid(10), Some(oid(TAG_SEED)));
        repo.set_head_branch("main");
        repo.set_entries(vec![
            tracked(
                "README.md",
                Change::Modified,
                Change::Modified,
                FILE_MODE,
                BLOB_SEED,
            ),
            tracked(
                "docs/guida.md",
                Change::Added,
                Change::Unmodified,
                0,
                BLOB_SEED + 10,
            ),
            tracked(
                "src/lib.rs",
                Change::Unmodified,
                Change::Modified,
                FILE_MODE,
                BLOB_SEED + 20,
            ),
            StatusEntry {
                path: RepoPath::from("note.txt"),
                index: Change::Untracked,
                worktree: Change::Untracked,
                kind: EntryKind::Untracked,
            },
        ]);
        repo
    }
}
