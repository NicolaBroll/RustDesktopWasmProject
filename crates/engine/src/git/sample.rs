//! Il repo di esempio di [`MemoryBackend::sample`].

use super::MemoryBackend;
use project_domain::{
    Change, ChangedFile, Commit, EntryKind, Oid, RepoPath, Signature, StatusEntry, TrackedInfo,
};
use std::collections::BTreeMap;

/// Data del primo commit (14 novembre 2023); i successivi sono a un'ora l'uno dall'altro.
const START: i64 = 1_700_000_000;
const HOUR: i64 = 3_600;
const FILE_MODE: u32 = 0o100644;

/// Seed degli id finti: commit `n` → `n`, oggetti tag e file da qui in poi.
const TAG_SEED: u64 = 1_000;
const BLOB_SEED: u64 = 2_000;
/// Primo seed dei contenuti dei file nei commit (uno nuovo per ogni versione di un file).
const HISTORY_SEED: u64 = 3_000;

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

/// Una modifica a un file in un commit, rispetto al primo genitore.
enum Edit {
    Add(&'static str),
    Modify(&'static str),
    Delete(&'static str),
    /// Da, a, somiglianza: con 100 il contenuto non cambia.
    Rename(&'static str, &'static str, u8),
}

use Edit::{Add, Delete, Modify, Rename};

/// File cambiati da ogni commit di [`COMMITS`], rispetto al primo genitore. Per il merge (10)
/// sono i file portati da `feature/grafo`, con il contenuto del secondo genitore.
const CHANGES: &[(u64, &[Edit])] = &[
    (1, &[Add(".gitignore"), Add(".gitkeep")]),
    (2, &[Add("README.md")]),
    (
        3,
        &[
            Add("Cargo.toml"),
            Add("src/lib.rs"),
            Add("src/main.rs"),
            Delete(".gitkeep"),
        ],
    ),
    (4, &[Add("src/log.rs"), Modify("src/lib.rs")]),
    (5, &[Add("src/graph.rs")]),
    (6, &[Modify("README.md")]),
    (7, &[Modify("src/graph.rs")]),
    (8, &[Add("src/status.rs"), Modify("src/lib.rs")]),
    (9, &[Add("tests/graph.rs"), Modify("Cargo.toml")]),
    (
        10,
        &[
            Add("src/graph.rs"),
            Add("tests/graph.rs"),
            Modify("Cargo.toml"),
        ],
    ),
    (11, &[Add("src/refs.rs"), Modify("src/lib.rs")]),
    (
        12,
        &[
            Add("src/panel.rs"),
            Modify("src/lib.rs"),
            Rename("src/main.rs", "src/bin/app.rs", 92),
        ],
    ),
];

type Tree = BTreeMap<&'static str, Oid>;

/// I file cambiati da ogni commit, con id dei contenuti coerenti lungo la storia: il contenuto
/// "vecchio" di un file è quello lasciato dal commit precedente che lo ha toccato.
fn history_changes() -> Vec<(Oid, Vec<ChangedFile>)> {
    let zero = Oid::from_bytes([0; Oid::LEN]);
    let mut next_seed = HISTORY_SEED;
    let mut fresh = || {
        next_seed += 1;
        oid(next_seed)
    };
    let mut trees: BTreeMap<u64, Tree> = BTreeMap::new();
    let mut out = Vec::new();
    for &(n, edits) in CHANGES {
        let parents = COMMITS
            .iter()
            .find(|c| c.0 == n)
            .map(|c| c.1)
            .unwrap_or_default();
        let mut tree = parents
            .first()
            .map(|p| trees[p].clone())
            .unwrap_or_default();
        let theirs = parents.get(1).map(|p| &trees[p]);
        // In un merge il contenuto viene dal secondo genitore, se lo ha.
        let mut new_content = |path: &str| match theirs.and_then(|t| t.get(path)) {
            Some(id) => *id,
            None => fresh(),
        };
        let file =
            |change, path: &str, original: Option<&str>, old: Option<Oid>, new: Option<Oid>| {
                ChangedFile {
                    path: RepoPath::from(path),
                    change,
                    original: original.map(RepoPath::from),
                    score: None,
                    old_mode: if old.is_some() { FILE_MODE } else { 0 },
                    new_mode: if new.is_some() { FILE_MODE } else { 0 },
                    old_oid: old.unwrap_or(zero),
                    new_oid: new.unwrap_or(zero),
                }
            };
        let mut files = Vec::new();
        for edit in edits {
            files.push(match *edit {
                Add(path) => {
                    let new = new_content(path);
                    tree.insert(path, new);
                    file(Change::Added, path, None, None, Some(new))
                }
                Modify(path) => {
                    let old = tree[path];
                    let new = new_content(path);
                    tree.insert(path, new);
                    file(Change::Modified, path, None, Some(old), Some(new))
                }
                Delete(path) => {
                    let old = tree.remove(path);
                    file(Change::Deleted, path, None, old, None)
                }
                Rename(from, to, score) => {
                    let old = tree.remove(from);
                    let new = if score == 100 {
                        old
                    } else {
                        Some(new_content(to))
                    };
                    if let Some(new) = new {
                        tree.insert(to, new);
                    }
                    ChangedFile {
                        score: Some(score),
                        ..file(Change::Renamed, to, Some(from), old, new)
                    }
                }
            });
        }
        // Come git: in ordine di percorso.
        files.sort_by(|a, b| a.path.cmp(&b.path));
        trees.insert(n, tree);
        out.push((oid(n), files));
    }
    out
}

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
    ///
    /// Ogni commit ha i suoi file cambiati (per `commit.show`): aggiunte, modifiche, una
    /// cancellazione (`.gitkeep` nel commit 3) e una rinomina con modifiche
    /// (`src/main.rs` → `src/bin/app.rs` nel commit 12).
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
        for (id, files) in history_changes() {
            repo.set_changes(id, files);
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
