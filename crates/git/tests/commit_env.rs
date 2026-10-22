//! `commit.create` con un ambiente ostile ereditato (es. l'app avviata da un hook git o da un
//! terminale con variabili git impostate): il commit va nel repo aperto, con il suo indice,
//! senza la configurazione iniettata e senza editor. Un file a parte perché cambia l'ambiente
//! del processo, che i test di un file condividono.

mod common;

use common::Repo;
use project_engine::{CommitRequest, GitBackend};

#[test]
fn inherited_environment_does_not_change_the_commit() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "primo");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    let other = Repo::new();
    other.commit_file("x.txt", "x\n", "altro repo");
    let other_head = other.rev("HEAD");
    let mut backend = repo.open();

    let other_git = other.path().join(".git");
    let other_index = other_git.join("index");
    let vars: [(&str, &std::ffi::OsStr); 6] = [
        ("GIT_DIR", other_git.as_os_str()),
        ("GIT_INDEX_FILE", other_index.as_os_str()),
        ("GIT_EDITOR", "false".as_ref()),
        (
            "GIT_CONFIG_PARAMETERS",
            "'commit.gpgsign=true' 'gpg.program=false'".as_ref(),
        ),
        ("GIT_CONFIG_COUNT", "1".as_ref()),
        ("GIT_CONFIG_KEY_0", "core.hooksPath".as_ref()),
    ];
    // SAFETY: è l'unico test di questo eseguibile, nessun altro thread legge l'ambiente.
    unsafe {
        for (key, value) in &vars {
            std::env::set_var(key, value);
        }
        std::env::set_var("GIT_CONFIG_VALUE_0", "/inesistente");
    }
    let result = backend.commit(&CommitRequest::new("nel repo giusto"));
    unsafe {
        for (key, _) in &vars {
            std::env::remove_var(key);
        }
        std::env::remove_var("GIT_CONFIG_VALUE_0");
    }
    let out = result.unwrap();
    assert_eq!(out.commit.id, repo.rev("HEAD"));
    assert_eq!(
        repo.git(&["show", "--format=", "--name-only", "HEAD"]),
        "b.txt"
    );
    assert_eq!(repo.git(&["log", "-1", "--format=%B"]), "nel repo giusto");
    assert_eq!(other.rev("HEAD"), other_head);
}
