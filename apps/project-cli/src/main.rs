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
    /// Il repo si apre con `repo.open={"path":"..."}`.
    Run {
        /// Comando come `id` oppure `id=<json>`; ripetibile.
        #[arg(long = "cmd", required = true)]
        commands: Vec<String>,
    },
    /// Legge comandi `id=<json>` da stdin, uno per riga, e stampa i risultati.
    Eval,
    /// Avvia il server MCP su stdin/stdout.
    Mcp {
        /// Cartella a cui sono confinati tutti i percorsi.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Commands { search } => {
            let registry = Registry::builtin();
            for spec in registry.search(search.as_deref().unwrap_or_default()) {
                println!("{:<20} {:<34} {}", spec.id, spec.title, spec.params_help);
            }
            Ok(())
        }
        Command::Run { commands } => {
            let registry = Registry::builtin();
            let mut session = session();
            for text in &commands {
                let result = execute(&registry, &mut session, text)?;
                println!("{result}");
            }
            Ok(())
        }
        Command::Eval => {
            let registry = Registry::builtin();
            let mut session = session();
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
            let mut server = project_mcp::Server::new(&root, Arc::new(CliOpener::new()))
                .with_context(|| format!("radice {}", root.display()))?;
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

/// Sessione senza repo aperto, che apre i repo con il `git` di sistema.
fn session() -> Session {
    Session::new(Arc::new(CliOpener::new()))
}
