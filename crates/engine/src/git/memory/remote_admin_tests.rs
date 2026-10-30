use super::super::MemoryBackend;
use crate::GitRemotes;
use crate::git::{ConfigEntry, RemoteRef};

fn entry(key: &str, value: &str) -> ConfigEntry {
    ConfigEntry {
        key: key.to_owned(),
        value: Some(value.to_owned()),
    }
}

#[test]
fn snapshot_remove_and_write_round_trip() {
    let mut repo = MemoryBackend::sample();
    repo.add_remote("origin/a", "/srv/a.git");
    repo.set_remote_branch("origin/a/x", MemoryBackend::fake_oid(12));
    repo.set_config("remote.origin.tagOpt", &["--no-tags"]);
    repo.set_config("remote.pushDefault", &["origin"]);
    let snapshot = repo.remote_snapshot("origin").unwrap();
    assert_eq!(
        snapshot.config,
        [
            entry("remote.origin.url", "https://example.com/project.git"),
            entry("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
            entry("remote.origin.tagOpt", "--no-tags"),
        ]
    );
    // `refs/remotes/origin/a/x` belongs to the remote `origin/a`.
    let names: Vec<&str> = snapshot.refs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "refs/remotes/origin/feature/graph",
            "refs/remotes/origin/main"
        ]
    );
    assert_eq!(snapshot.upstream_branches(), ["feature/graph", "main"]);
    assert!(snapshot.push_default);
    assert_eq!(snapshot.head, None);

    repo.remove_remote_config(&snapshot);
    for r in &snapshot.refs {
        repo.refs.remove(&r.name);
    }
    let gone = repo.remote_snapshot("origin").unwrap();
    assert!(gone.is_absent() && gone.upstreams.is_empty() && !gone.push_default);
    assert_eq!(repo.remotes().unwrap(), ["origin/a"]);

    for RemoteRef { name, target } in &snapshot.refs {
        repo.set_ref(name.clone(), *target, None);
    }
    assert!(repo.write_remote_config(&snapshot).is_empty());
    let back = repo.remote_snapshot("origin").unwrap();
    assert!(
        snapshot.differences(&back).is_empty(),
        "{:?}",
        snapshot.differences(&back)
    );
}

#[test]
fn create_refuses_an_existing_name() {
    let mut repo = MemoryBackend::sample();
    assert!(repo.create_remote("origin", "/x").is_err());
    repo.create_remote("fork", "/x").unwrap();
    assert_eq!(repo.remotes().unwrap(), ["fork", "origin"]);
}
