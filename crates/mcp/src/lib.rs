//! L6 — server MCP (Model Context Protocol).
//!
//! Trasporto: JSON-RPC 2.0, un messaggio per riga su stdin/stdout; nessuna
//! porta di rete. Ogni percorso è risolto dentro la radice data a
//! [`Server::new`] e rifiutato se ne esce (anche tramite link simbolici): i repository si
//! aprono solo con il tool `open_repo`, non con `repo.open` tramite `execute_command`.

mod paths;

use paths::Root;
use project_engine::{Registry, RepoOpener, Session};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::Arc;

const PROTOCOL_VERSION: &str = "2025-06-18";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

pub struct Server {
    session: Session,
    registry: Registry,
    root: Root,
}

impl Server {
    /// Server confinato in `root`, che apre i repository con `opener`.
    pub fn new(root: &Path, opener: Arc<dyn RepoOpener>) -> io::Result<Self> {
        Ok(Self {
            session: Session::new(opener),
            registry: Registry::builtin(),
            root: Root::new(root)?,
        })
    }

    /// Apre il repository nella radice, come `open_repo` con `.`.
    pub fn open_root(&mut self) -> Result<Value, String> {
        self.open_repo(json!({ "path": "." }))
    }

    /// Legge richieste da `input` fino a EOF e scrive le risposte su `output`.
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Value>(&line) {
                Ok(message) => self.handle(message),
                Err(e) => Some(error(Value::Null, PARSE_ERROR, &e.to_string())),
            };
            if let Some(response) = response {
                serde_json::to_writer(&mut output, &response)?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    /// Gestisce un messaggio JSON-RPC. `None` per le notifiche.
    pub fn handle(&mut self, message: Value) -> Option<Value> {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Some(error(
                message.get("id").cloned().unwrap_or(Value::Null),
                INVALID_REQUEST,
                "manca `method`",
            ));
        };
        // Le notifiche (senza id) non ricevono risposta.
        let id = message.get("id")?.clone();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        Some(match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                ok(
                    id,
                    json!({
                        "protocolVersion": requested.unwrap_or(PROTOCOL_VERSION),
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": env!("CARGO_PKG_NAME"),
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                    }),
                )
            }
            "ping" => ok(id, json!({})),
            "tools/list" => ok(id, json!({ "tools": tools() })),
            "tools/call" => match serde_json::from_value::<Call>(params) {
                Ok(call) => ok(id, self.call_tool(&call.name, call.arguments)),
                Err(e) => error(id, INVALID_PARAMS, &e.to_string()),
            },
            other => error(
                id,
                METHOD_NOT_FOUND,
                &format!("metodo sconosciuto: {other}"),
            ),
        })
    }

    fn call_tool(&mut self, name: &str, args: Value) -> Value {
        let result = match name {
            "list_commands" => self.list_commands(args),
            "execute_command" => self.execute_command(args),
            "open_repo" => self.open_repo(args),
            other => Err(format!("tool sconosciuto: {other}")),
        };
        match result {
            Ok(value) => tool_result(&value, false),
            Err(message) => tool_result(&Value::String(message), true),
        }
    }

    fn list_commands(&self, args: Value) -> Result<Value, String> {
        #[derive(Deserialize)]
        struct Args {
            #[serde(default)]
            search: String,
        }
        let Args { search } = parse_args(args)?;
        let commands: Vec<_> = self.registry.search(&search).collect();
        Ok(json!(commands))
    }

    fn execute_command(&mut self, args: Value) -> Result<Value, String> {
        #[derive(Deserialize)]
        struct Args {
            id: String,
            #[serde(default)]
            params: Value,
        }
        let Args { id, params } = parse_args(args)?;
        // `repo.open` accetta qualunque percorso: da qui si passa per `open_repo`.
        if id == "repo.open" {
            return Err("per aprire un repository usa il tool `open_repo`".to_owned());
        }
        self.registry
            .run(&id, &mut self.session, params)
            .map_err(|e| e.to_string())
    }

    fn open_repo(&mut self, args: Value) -> Result<Value, String> {
        let PathArg { path } = parse_args(args)?;
        let path = self.root.existing(&path)?;
        let path = path
            .to_str()
            .ok_or_else(|| format!("{}: percorso non UTF-8", path.display()))?;
        self.registry
            .run("repo.open", &mut self.session, json!({ "path": path }))
            .map_err(|e| e.to_string())
    }
}

#[derive(Deserialize)]
struct Call {
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Deserialize)]
struct PathArg {
    path: String,
}

fn parse_args<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| format!("argomenti non validi: {e}"))
}

fn tools() -> Value {
    let path_schema = json!({
        "type": "object",
        "properties": { "path": { "type": "string", "description": "Cartella relativa alla radice" } },
        "required": ["path"],
    });
    json!([
        {
            "name": "list_commands",
            "description": "Elenca i comandi del catalogo, filtrati per testo nell'id o nel titolo.",
            "inputSchema": {
                "type": "object",
                "properties": { "search": { "type": "string" } },
            },
        },
        {
            "name": "execute_command",
            "description": "Esegue un comando del catalogo sulla sessione corrente. Vedi list_commands per id e parametri; i comandi con `writes: true` modificano il repository dell'utente (es. `index.stage`), quelli che possono far perdere dati chiedono `\"confirm\": true`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "params": { "type": "object" },
                },
                "required": ["id"],
            },
        },
        {
            "name": "open_repo",
            "description": "Apre il repository git nella cartella indicata (`.` per la radice); i comandi `repo.*` lavorano poi su questo repo.",
            "inputSchema": path_schema,
        },
    ])
}

fn tool_result(value: &Value, is_error: bool) -> Value {
    let text = match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).expect("Value è serializzabile"),
    };
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_engine::{MemoryBackend, MemoryOpener};

    fn temp_root(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("project-mcp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    /// Server su `root` con il repo di esempio registrato nella sottocartella `repo`.
    fn server(root: &Path) -> Server {
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut opener = MemoryOpener::new();
        opener.insert(repo.to_str().unwrap(), MemoryBackend::sample());
        Server::new(root, Arc::new(opener)).unwrap()
    }

    fn call(server: &mut Server, name: &str, arguments: Value) -> Value {
        let response = server
            .handle(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": name, "arguments": arguments },
            }))
            .unwrap();
        response["result"].clone()
    }

    #[test]
    fn serve_over_lines() {
        let root = temp_root("serve");
        let mut server = server(&root);
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n",
            "not json\n",
        );
        let mut out = Vec::new();
        server.serve(input.as_bytes(), &mut out).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 3, "la notifica non ha risposta");
        assert_eq!(lines[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 3);
        assert_eq!(lines[2]["error"]["code"], PARSE_ERROR);
    }

    #[test]
    fn open_repo_then_execute() {
        let root = temp_root("open");
        let mut server = server(&root);
        let r = call(&mut server, "execute_command", json!({"id": "repo.status"}));
        assert_eq!(r["isError"], true, "nessun repo aperto");

        let r = call(&mut server, "open_repo", json!({"path": "repo"}));
        assert_eq!(r["isError"], false, "{r}");
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "repo.log", "params": {"limit": 2}}),
        );
        assert_eq!(r["isError"], false, "{r}");
        let text = r["content"][0]["text"].as_str().unwrap();
        let log: Value = serde_json::from_str(text).unwrap();
        assert_eq!(log["commits"].as_array().unwrap().len(), 2);

        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "commit.show", "params": {"id": "HEAD"}}),
        );
        assert_eq!(r["isError"], false, "{r}");
        let text = r["content"][0]["text"].as_str().unwrap();
        let details: Value = serde_json::from_str(text).unwrap();
        assert!(
            !details["files"].as_array().unwrap().is_empty(),
            "{details}"
        );

        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "diff.commit", "params": {"id": "HEAD", "path": "src/main.rs"}}),
        );
        assert_eq!(r["isError"], false, "{r}");
        let text = r["content"][0]["text"].as_str().unwrap();
        let diff: Value = serde_json::from_str(text).unwrap();
        assert_eq!(diff["files"][0]["change"], "Renamed", "{diff}");
    }

    #[test]
    fn write_commands_are_marked_and_change_the_repo() {
        let root = temp_root("write");
        let mut server = server(&root);
        let r = call(&mut server, "list_commands", json!({"search": "index."}));
        let text = r["content"][0]["text"].as_str().unwrap();
        let commands: Value = serde_json::from_str(text).unwrap();
        let ids: Vec<_> = commands
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c["id"].clone(), c["writes"].clone()))
            .collect();
        assert_eq!(
            ids,
            [
                (json!("index.stage"), json!(true)),
                (json!("index.unstage"), json!(true))
            ]
        );

        call(&mut server, "open_repo", json!({"path": "repo"}));
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "index.stage", "params": {"paths": ["note.txt"]}}),
        );
        assert_eq!(r["isError"], false, "{r}");
        let text = r["content"][0]["text"].as_str().unwrap();
        let status: Value = serde_json::from_str(text).unwrap();
        let note = status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"] == "note.txt")
            .unwrap();
        assert_eq!(note["index"], "Added");

        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "index.unstage", "params": {"paths": ["../fuori"]}}),
        );
        assert_eq!(r["isError"], true, "{r}");
    }

    #[test]
    fn open_root_uses_the_root_folder() {
        let root = temp_root("root");
        let mut opener = MemoryOpener::new();
        opener.insert(root.to_str().unwrap(), MemoryBackend::sample());
        let mut server = Server::new(&root, Arc::new(opener)).unwrap();
        let opened = server.open_root().unwrap();
        assert_eq!(opened["path"], root.to_str().unwrap());
        let r = call(&mut server, "execute_command", json!({"id": "repo.refs"}));
        assert_eq!(r["isError"], false, "{r}");

        let mut empty = server_without_repos(&root);
        assert!(empty.open_root().is_err());
    }

    fn server_without_repos(root: &Path) -> Server {
        Server::new(root, Arc::new(MemoryOpener::new())).unwrap()
    }

    #[test]
    fn repo_open_is_only_reachable_through_open_repo() {
        let root = temp_root("bypass");
        let mut server = server(&root);
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "repo.open", "params": {"path": "/"}}),
        );
        assert_eq!(r["isError"], true);
        assert!(
            r["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("open_repo")
        );
    }

    #[test]
    fn paths_outside_root_are_rejected() {
        let root = temp_root("escape");
        let mut server = server(&root);
        for path in ["..", "/etc", "repo/../..", "non-esiste"] {
            let r = call(&mut server, "open_repo", json!({ "path": path }));
            assert_eq!(r["isError"], true, "{path}");
        }
    }

    #[test]
    fn unknown_tool_is_tool_error() {
        let root = temp_root("unknown");
        let mut server = server(&root);
        assert_eq!(call(&mut server, "nope", json!({}))["isError"], true);
        let r = call(&mut server, "execute_command", json!({"id": "nope"}));
        assert_eq!(r["isError"], true);
    }
}
