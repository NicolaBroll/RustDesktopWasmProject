//! L6 — server MCP (Model Context Protocol).
//!
//! Trasporto: JSON-RPC 2.0, un messaggio per riga su stdin/stdout; nessuna
//! porta di rete. Ogni percorso è risolto dentro la radice data a
//! [`Server::new`] e rifiutato se ne esce (anche tramite link simbolici).

mod paths;

use paths::Root;
use project_engine::{Document, Registry, Session};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::Path;

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
    pub fn new(root: &Path) -> io::Result<Self> {
        Ok(Self {
            session: Session::default(),
            registry: Registry::builtin(),
            root: Root::new(root)?,
        })
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
            "open_document" => self.open_document(args),
            "save_document" => self.save_document(args),
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
        self.registry
            .run(&id, &mut self.session, params)
            .map_err(|e| e.to_string())
    }

    fn open_document(&mut self, args: Value) -> Result<Value, String> {
        let PathArg { path } = parse_args(args)?;
        let path = self.root.existing(&path)?;
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let content = String::from_utf8(bytes).map_err(|_| "il file non è UTF-8".to_owned())?;
        self.registry
            .run(
                "document.load",
                &mut self.session,
                json!({ "content": content }),
            )
            .map_err(|e| e.to_string())?;
        Ok(json!({ "opened": path }))
    }

    fn save_document(&mut self, args: Value) -> Result<Value, String> {
        let PathArg { path } = parse_args(args)?;
        let path = self.root.writable(&path)?;
        let bytes = Document::to_bytes(self.session.document());
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(json!({ "saved": path }))
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
        "properties": { "path": { "type": "string", "description": "Percorso relativo alla radice" } },
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
            "description": "Esegue un comando del catalogo sulla sessione corrente. Vedi list_commands per id e parametri.",
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
            "name": "open_document",
            "description": "Apre un documento dalla radice consentita.",
            "inputSchema": path_schema,
        },
        {
            "name": "save_document",
            "description": "Salva il documento corrente nella radice consentita.",
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

    fn temp_root(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("project-mcp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
        let mut server = Server::new(&root).unwrap();
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
        assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 4);
        assert_eq!(lines[2]["error"]["code"], PARSE_ERROR);
    }

    #[test]
    fn execute_save_open() {
        let root = temp_root("io");
        let mut server = Server::new(&root).unwrap();
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "item.add", "params": {"text": "x"}}),
        );
        assert_eq!(r["isError"], false);
        let r = call(&mut server, "save_document", json!({"path": "doc.json"}));
        assert_eq!(r["isError"], false, "{r}");

        let mut other = Server::new(&root).unwrap();
        let r = call(&mut other, "open_document", json!({"path": "doc.json"}));
        assert_eq!(r["isError"], false, "{r}");
        assert_eq!(other.session.document(), server.session.document());
    }

    #[test]
    fn paths_outside_root_are_rejected() {
        let root = temp_root("escape");
        let mut server = Server::new(&root).unwrap();
        for path in ["../fuori.json", "/etc/passwd", "sub/../../x.json"] {
            let r = call(&mut server, "save_document", json!({ "path": path }));
            assert_eq!(r["isError"], true, "{path}");
            let r = call(&mut server, "open_document", json!({ "path": path }));
            assert_eq!(r["isError"], true, "{path}");
        }
    }

    #[test]
    fn unknown_tool_is_tool_error() {
        let root = temp_root("unknown");
        let mut server = Server::new(&root).unwrap();
        assert_eq!(call(&mut server, "nope", json!({}))["isError"], true);
        let r = call(&mut server, "execute_command", json!({"id": "nope"}));
        assert_eq!(r["isError"], true);
    }
}
