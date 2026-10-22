//! Controllo automatico del layering: un crate importa solo da livelli
//! strettamente inferiori. Aggiungere qui ogni nuovo crate del workspace.

use cargo_metadata::{DependencyKind, MetadataCommand};
use std::collections::HashMap;

fn levels() -> HashMap<&'static str, u8> {
    HashMap::from([
        ("project-core", 0),
        ("project-domain", 2),
        ("project-engine", 5),
        ("project-ui", 6),
        ("project-mcp", 6),
        ("project-git", 6),
        ("project", 7),
        ("project-cli", 7),
        ("project-web", 7),
    ])
}

pub fn check_layers() -> anyhow::Result<()> {
    let metadata = MetadataCommand::new()
        .manifest_path(super::workspace_root().join("Cargo.toml"))
        .no_deps()
        .exec()?;
    let levels = levels();
    let mut errors = Vec::new();

    for pkg in metadata.workspace_packages() {
        let name = pkg.name.as_str();
        let Some(&mine) = levels.get(name) else {
            if name != "xtask" {
                errors.push(format!(
                    "{name}: livello non dichiarato in xtask/src/layers.rs"
                ));
            }
            continue;
        };
        let deps = pkg
            .dependencies
            .iter()
            .filter(|d| matches!(d.kind, DependencyKind::Normal | DependencyKind::Build));
        for dep in deps {
            if let Some(&theirs) = levels.get(dep.name.as_str())
                && theirs >= mine
            {
                errors.push(format!(
                    "{name} (L{mine}) dipende da {} (L{theirs})",
                    dep.name
                ));
            }
        }
    }

    if errors.is_empty() {
        eprintln!("layering: ok ({} crate)", levels.len());
        Ok(())
    } else {
        anyhow::bail!("violazioni del layering:\n  {}", errors.join("\n  "))
    }
}
