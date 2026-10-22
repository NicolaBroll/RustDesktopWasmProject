//! L7 — CLI: catalogo comandi, esecuzione in batch, REPL `eval`, server MCP.

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use project_engine::{Invocation, Registry, Session};
use project_git::CliOpener;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(version, about = "Riga di comando di Project")]
struct Cli {
    /// Repository da aprire prima di `run` ed `eval` (come `repo.open`).
    #[arg(long, global = true, value_name = "PERCORSO")]
    repo: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Elenca i comandi disponibili.
    Commands {
        /// Filtra per testo nell'id o nel titolo.
        #[arg(long)]
        search: Option<String>,
    },
    /// Esegue una sequenza di comandi e stampa i risultati (JSON, uno per riga).
    Run {
        /// Comando come `id` oppure `id=<json>`; ripetibile.
        #[arg(long = "cmd", required = true)]
        commands: Vec<String>,
    },
    /// Legge comandi `id=<json>` da stdin, uno per riga, e stampa i risultati.
    Eval,
    /// Avvia il server MCP su stdin/stdout, con il repository della radice già aperto.
    Mcp {
        /// Cartella a cui sono confinati tutti i percorsi, anche la ricerca dei repository.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let Cli { repo, command } = Cli::parse();
    match command {
        Command::Commands { search } => {
            let registry = Registry::builtin();
            for spec in registry.search(search.as_deref().unwrap_or_default()) {
                let writes = if spec.writes { "  [modifica]" } else { "" };
                println!(
                    "{:<20} {:<34} {}{writes}",
                    spec.id, spec.title, spec.params_help
                );
            }
            Ok(())
        }
        Command::Run { commands } => {
            let registry = Registry::builtin();
            let mut session = session(repo.as_deref())?;
            for text in &commands {
                let result = execute(&registry, &mut session, text)?;
                println!("{result}");
            }
            Ok(())
        }
        Command::Eval => {
            let registry = Registry::builtin();
            let mut session = session(repo.as_deref())?;
            let mut stdout = io::stdout().lock();
            for line in io::stdin().lock().lines() {
                let line = line?;
                if line.trim().is_empty() || line.trim_start().starts_with('#') {
                    continue;
                }
                // In modalità interattiva un errore non interrompe la sessione.
                let reply = match execute(&registry, &mut session, &line) {
                    Ok(result) => serde_json::json!({ "ok": result }),
                    Err(e) => serde_json::json!({ "error": format!("{e:#}") }),
                };
                writeln!(stdout, "{reply}")?;
            }
            Ok(())
        }
        Command::Mcp { root } => {
            if repo.is_some() {
                anyhow::bail!("`mcp` non usa `--repo`: apre il repository di `--root`");
            }
            let root = root
                .canonicalize()
                .with_context(|| format!("radice {}", root.display()))?;
            let opener = CliOpener::new().confine_to(&root);
            let mut server = project_mcp::Server::new(&root, Arc::new(opener))
                .with_context(|| format!("radice {}", root.display()))?;
            // Senza repo nella radice il server parte lo stesso: si può usare `open_repo`.
            match server.open_root() {
                Ok(_) => eprintln!("repository aperto: {}", root.display()),
                Err(e) => eprintln!("nessun repository aperto: {e}"),
            }
            server.serve(io::stdin().lock(), io::stdout().lock())?;
            Ok(())
        }
    }
}

fn execute(
    registry: &Registry,
    session: &mut Session,
    text: &str,
) -> anyhow::Result<serde_json::Value> {
    let Invocation { id, params } = text.parse()?;
    registry
        .run(&id, session, params)
        .with_context(|| format!("comando `{id}`"))
}

/// Sessione che apre i repo con il `git` di sistema, con `repo` già aperto se c'è.
fn session(repo: Option<&str>) -> anyhow::Result<Session> {
    let mut session = Session::new(Arc::new(CliOpener::new()));
    if let Some(path) = repo {
        session
            .open(path)
            .with_context(|| format!("--repo {path}"))?;
    }
    Ok(session)
}
