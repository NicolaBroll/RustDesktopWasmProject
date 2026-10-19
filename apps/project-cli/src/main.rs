//! L7 — CLI: catalogo comandi, esecuzione in batch, REPL `eval`, server MCP.

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use project_engine::{Document, Invocation, Registry, Session};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

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
    Run {
        /// Documento di partenza (altrimenti uno vuoto).
        #[arg(long = "in")]
        input: Option<PathBuf>,
        /// Comando come `id` oppure `id=<json>`; ripetibile.
        #[arg(long = "cmd", required = true)]
        commands: Vec<String>,
        /// Dove salvare il documento risultante.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Legge comandi `id=<json>` da stdin, uno per riga, e stampa i risultati.
    Eval {
        #[arg(long = "in")]
        input: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
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
        Command::Run {
            input,
            commands,
            out,
        } => {
            let registry = Registry::builtin();
            let mut session = open(input.as_deref())?;
            for text in &commands {
                let result = execute(&registry, &mut session, text)?;
                println!("{result}");
            }
            save(&session, out.as_deref())
        }
        Command::Eval { input, out } => {
            let registry = Registry::builtin();
            let mut session = open(input.as_deref())?;
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
            save(&session, out.as_deref())
        }
        Command::Mcp { root } => {
            let mut server = project_mcp::Server::new(&root)
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

fn open(path: Option<&Path>) -> anyhow::Result<Session> {
    let document = match path {
        Some(path) => {
            let bytes = std::fs::read(path).with_context(|| path.display().to_string())?;
            Document::from_bytes(&bytes).with_context(|| path.display().to_string())?
        }
        None => Document::default(),
    };
    Ok(Session::new(document))
}

fn save(session: &Session, path: Option<&Path>) -> anyhow::Result<()> {
    if let Some(path) = path {
        std::fs::write(path, session.document().to_bytes())
            .with_context(|| path.display().to_string())?;
    }
    Ok(())
}
