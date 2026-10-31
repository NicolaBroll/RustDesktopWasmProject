//! L7 — CLI: command catalog, batch execution, `eval` REPL, MCP server.

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use project_engine::{Invocation, JobContext, Progress, Registry, Session};
use project_git::CliOpener;
use std::io::{self, BufRead, IsTerminal as _, Write};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(version, about = "BinariGit command line")]
struct Cli {
    /// Repository to open before `run` and `eval` (like `repo.open`).
    #[arg(long, global = true, value_name = "PATH")]
    repo: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Lists the available commands.
    Commands {
        /// Filters by text in the id or title.
        #[arg(long)]
        search: Option<String>,
        /// Prints the catalog as JSON, with the JSON Schema of the parameters (`params`).
        #[arg(long)]
        json: bool,
    },
    /// Runs a sequence of commands and prints the results (JSON, one per line).
    Run {
        /// Command as `id` or `id=<json>`; repeatable.
        #[arg(long = "cmd", required = true)]
        commands: Vec<String>,
    },
    /// Reads `id=<json>` commands from stdin, one per line, and prints the results.
    Eval,
    /// Starts the MCP server on stdin/stdout, with the root repository already open.
    Mcp {
        /// Folder all paths are confined to, including the repository search.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    // Started by git or ssh as askpass (S52): never the case today (the CLI sets no prompter),
    // but the same executable must never run a command in that role.
    if let Some(code) = project_git::askpass_client() {
        std::process::exit(code);
    }
    let Cli { repo, command } = Cli::parse();
    match command {
        Command::Commands { search, json } => {
            let registry = Registry::builtin();
            let specs = registry.search(search.as_deref().unwrap_or_default());
            if json {
                let specs: Vec<_> = specs.collect();
                println!("{}", serde_json::to_string_pretty(&specs)?);
                return Ok(());
            }
            for spec in specs {
                let writes = if spec.writes { "  [writes]" } else { "" };
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
                // In interactive mode an error does not end the session.
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
                anyhow::bail!("`mcp` does not use `--repo`: it opens the repository of `--root`");
            }
            let root = root
                .canonicalize()
                .with_context(|| format!("root {}", root.display()))?;
            let opener = CliOpener::new().confine_to(&root);
            let mut server = project_mcp::Server::new(&root, Arc::new(opener))
                .with_context(|| format!("root {}", root.display()))?;
            // Without a repo in the root the server starts anyway: `open_repo` can be used.
            match server.open_root() {
                Ok(_) => eprintln!("repository open: {}", root.display()),
                Err(e) => eprintln!("no repository open: {e}"),
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
        .run_with(&id, session, params, progress_on_stderr())
        .with_context(|| format!("command `{id}`"))
}

/// Progress of slow commands (`remote.fetch`) on stderr, on a line rewritten with `\r`,
/// only if stderr is a terminal: stdout stays the JSON of the results.
fn progress_on_stderr() -> JobContext {
    if !io::stderr().is_terminal() {
        return JobContext::new();
    }
    JobContext::new().with_progress(|p: &Progress| {
        let end = if p.done { "\n" } else { "\r" };
        eprint!("{}\x1b[K{end}", p.text);
    })
}

/// Session that opens repos with the system `git`, with `repo` already open if there is one.
fn session(repo: Option<&str>) -> anyhow::Result<Session> {
    let mut session = Session::new(Arc::new(CliOpener::new()));
    if let Some(path) = repo {
        session
            .open(path)
            .with_context(|| format!("--repo {path}"))?;
    }
    Ok(session)
}
