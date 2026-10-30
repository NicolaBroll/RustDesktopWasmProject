//! L6 — MCP server (Model Context Protocol).
//!
//! Transport: JSON-RPC 2.0, one message per line on stdin/stdout; no network
//! port. Every path is resolved inside the root given to
//! [`Server::new`] and rejected if it escapes (also through symbolic links): repositories are
//! opened only with the `open_repo` tool; `execute_command` refuses the commands with
//! [`CommandSpec::outside_repo`](project_engine::CommandSpec::outside_repo) (`repo.open`,
//! `repo.clone`, `repo.init`: their paths would not be confined).

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
    /// Server confined to `root`, opening repositories with `opener`.
    pub fn new(root: &Path, opener: Arc<dyn RepoOpener>) -> io::Result<Self> {
        Ok(Self {
            session: Session::new(opener),
            registry: Registry::builtin(),
            root: Root::new(root)?,
        })
    }

    /// Opens the repository at the root, like `open_repo` with `.`.
    pub fn open_root(&mut self) -> Result<Value, String> {
        self.open_repo(json!({ "path": "." }))
    }

    /// Reads requests from `input` until EOF and writes the responses to `output`.
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

    /// Handles a JSON-RPC message. `None` for notifications.
    pub fn handle(&mut self, message: Value) -> Option<Value> {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Some(error(
                message.get("id").cloned().unwrap_or(Value::Null),
                INVALID_REQUEST,
                "missing `method`",
            ));
        };
        // Notifications (without an id) get no response.
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
            other => error(id, METHOD_NOT_FOUND, &format!("unknown method: {other}")),
        })
    }

    fn call_tool(&mut self, name: &str, args: Value) -> Value {
        let result = match name {
            "list_commands" => self.list_commands(args),
            "execute_command" => self.execute_command(args),
            "open_repo" => self.open_repo(args),
            other => Err(format!("unknown tool: {other}")),
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
        // Paths outside the repo (`repo.open`, `repo.clone`, `repo.init`) could be anywhere: not
        // confined to the root. Repositories are opened with `open_repo`.
        if self.registry.get(&id).is_some_and(|spec| spec.outside_repo) {
            return Err(format!(
                "`{id}` is not available over MCP: its paths are not confined to the root \
                 (to open a repository use the `open_repo` tool)"
            ));
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
            .ok_or_else(|| format!("{}: non-UTF-8 path", path.display()))?;
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
    serde_json::from_value(args).map_err(|e| format!("invalid arguments: {e}"))
}

/// The general rules of `execute_command`; the parameters of each command are in the
/// `params` schema of `list_commands`.
const EXECUTE_DESCRIPTION: &str = "Runs a catalog command on the current session. \
Ids and parameters (`params`, a JSON Schema per command) are in `list_commands`. \
Commands with `writes: true` change the user's repository; most of them are undone with \
`edit.undo`. Those that can lose data or rewrite published commits fail with an error that asks \
for `\"confirm\": true`: pass it only if the user asked for it, and when the error gives a full \
id pass it back unchanged in `\"expected\"`, so the command acts only on what the user saw. \
Commands with `outside_repo: true` are not available here: open repositories with `open_repo`. \
Network commands (`remote.fetch`, `remote.pull`, `remote.push`, `remote.push_tag`, \
`remote.delete_branch`, `remote.delete_tag`, `remote.add` with `fetch`) take a configured remote \
by name, never a URL, and can be slow; what they change on the remote cannot be undone.";

fn tools() -> Value {
    let path_schema = json!({
        "type": "object",
        "properties": { "path": { "type": "string", "description": "Folder relative to the root" } },
        "required": ["path"],
    });
    json!([
        {
            "name": "list_commands",
            "description": "Lists the catalog commands, filtered by text in the id or title: `id`, `title`, `params` (JSON Schema of the parameters), `writes` (changes the repository), `outside_repo` (not available here).",
            "inputSchema": {
                "type": "object",
                "properties": { "search": { "type": "string" } },
            },
        },
        {
            "name": "execute_command",
            "description": EXECUTE_DESCRIPTION,
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
            "description": "Opens the git repository in the given folder (`.` for the root); the `repo.*` commands then work on this repo.",
            "inputSchema": path_schema,
        },
    ])
}

fn tool_result(value: &Value, is_error: bool) -> Value {
    let text = match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).expect("Value is serializable"),
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

    /// Server on `root` with the sample repo registered in the `repo` subfolder.
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
        assert_eq!(lines.len(), 3, "the notification gets no response");
        assert_eq!(lines[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 3);
        assert_eq!(lines[2]["error"]["code"], PARSE_ERROR);
    }

    #[test]
    fn open_repo_then_execute() {
        let root = temp_root("open");
        let mut server = server(&root);
        let r = call(&mut server, "execute_command", json!({"id": "repo.status"}));
        assert_eq!(r["isError"], true, "no repository open");

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
                (json!("index.stage_lines"), json!(true)),
                (json!("index.unstage"), json!(true)),
                (json!("index.unstage_lines"), json!(true))
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
        let out: Value = serde_json::from_str(text).unwrap();
        assert_eq!(out["backup"], Value::Null);
        let note = out["status"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"] == "note.txt")
            .unwrap();
        assert_eq!(note["index"], "Added");

        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "index.unstage", "params": {"paths": ["../outside"]}}),
        );
        assert_eq!(r["isError"], true, "{r}");

        // Lines: first the diff (line numbers and digest), then staging one line.
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "diff.worktree", "params": {"path": "src/lib.rs"}}),
        );
        let text = r["content"][0]["text"].as_str().unwrap();
        let diff: Value = serde_json::from_str(text).unwrap();
        let file = &diff["files"][0];
        let added = file["hunks"][0]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["kind"] == "Added")
            .unwrap();
        let params = json!({
            "path": "src/lib.rs",
            "lines": [{ "new": added["new_line"] }],
            "digest": file["digest"],
        });
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "index.stage_lines", "params": params}),
        );
        assert_eq!(r["isError"], false, "{r}");
        let text = r["content"][0]["text"].as_str().unwrap();
        let out: Value = serde_json::from_str(text).unwrap();
        let lib = out["status"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"] == "src/lib.rs")
            .unwrap();
        assert_eq!(lib["index"], "Modified");
        assert_eq!(lib["worktree"], "Modified");
        // The same digest is now stale: an error that explains what to do.
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "index.stage_lines", "params": params}),
        );
        assert_eq!(r["isError"], true, "{r}");
        let text = r["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("reload the diff"), "{text}");
    }

    #[test]
    fn commit_create_is_a_write_and_amend_of_published_needs_confirm() {
        let root = temp_root("commit");
        let mut server = server(&root);
        let r = call(
            &mut server,
            "list_commands",
            json!({"search": "commit.create"}),
        );
        let text = r["content"][0]["text"].as_str().unwrap();
        let commands: Value = serde_json::from_str(text).unwrap();
        assert_eq!(commands[0]["id"], "commit.create");
        assert_eq!(commands[0]["writes"], true);

        call(&mut server, "open_repo", json!({"path": "repo"}));
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "commit.create", "params": {"message": "From an agent"}}),
        );
        assert_eq!(r["isError"], false, "{r}");
        let text = r["content"][0]["text"].as_str().unwrap();
        let out: Value = serde_json::from_str(text).unwrap();
        assert_eq!(out["commit"]["summary"], "From an agent");
        assert_eq!(out["branch"], "refs/heads/main");

        // A repo where HEAD is already on `origin/main`: `amend` asks for an explicit confirmation.
        let root = temp_root("commit-published");
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut sample = MemoryBackend::sample();
        let head = project_engine::GitRead::head(&sample)
            .unwrap()
            .target()
            .unwrap();
        sample.set_remote_branch("origin/main", head);
        let mut opener = MemoryOpener::new();
        opener.insert(repo.to_str().unwrap(), sample);
        let mut server = Server::new(&root, Arc::new(opener)).unwrap();
        call(&mut server, "open_repo", json!({"path": "repo"}));
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "commit.create", "params": {"message": "x", "amend": true}}),
        );
        assert_eq!(r["isError"], true, "{r}");
        let text = r["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"confirm\": true"), "{text}");
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "commit.create", "params": {"message": "x", "amend": true, "confirm": true}}),
        );
        assert_eq!(r["isError"], false, "{r}");
    }

    #[test]
    fn undo_and_redo_are_writes_and_use_the_session_journal() {
        let root = temp_root("undo");
        let mut server = server(&root);
        let r = call(&mut server, "list_commands", json!({"search": "edit."}));
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
                (json!("edit.history"), json!(false)),
                (json!("edit.redo"), json!(true)),
                (json!("edit.undo"), json!(true))
            ]
        );

        call(&mut server, "open_repo", json!({"path": "repo"}));
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "commit.create", "params": {"message": "From an agent"}}),
        );
        let text = r["content"][0]["text"].as_str().unwrap();
        let commit: Value = serde_json::from_str(text).unwrap();
        let r = call(&mut server, "execute_command", json!({"id": "edit.undo"}));
        assert_eq!(r["isError"], false, "{r}");
        let text = r["content"][0]["text"].as_str().unwrap();
        let undo: Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            undo["status"]["head"]["Branch"]["target"],
            commit["previous"]
        );
        assert_eq!(undo["can_redo"], true);
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "edit.redo", "params": {}}),
        );
        assert_eq!(r["isError"], false, "{r}");
        let r = call(
            &mut server,
            "execute_command",
            json!({"id": "edit.history"}),
        );
        let text = r["content"][0]["text"].as_str().unwrap();
        let history: Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            history["done"][0]["description"],
            undo["undone"]["description"]
        );
        // Nothing left to redo: an error that says so.
        let r = call(&mut server, "execute_command", json!({"id": "edit.redo"}));
        assert_eq!(r["isError"], true, "{r}");
        let text = r["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("nothing to redo"), "{text}");
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

    /// The catalog of `list_commands` as JSON.
    fn list(server: &mut Server, search: &str) -> Vec<Value> {
        let r = call(server, "list_commands", json!({ "search": search }));
        let text = r["content"][0]["text"].as_str().unwrap();
        serde_json::from_value(serde_json::from_str(text).unwrap()).unwrap()
    }

    #[test]
    fn list_commands_gives_the_parameter_schema() {
        let root = temp_root("schema");
        let mut server = server(&root);
        let commands = list(&mut server, "");
        let schema = |id: &str| {
            let spec = commands.iter().find(|c| c["id"] == id).unwrap();
            assert_eq!(spec["params"]["type"], "object", "{id}");
            assert_eq!(spec["params"]["additionalProperties"], false, "{id}");
            spec["params"].clone()
        };

        let reset = schema("branch.reset");
        assert_eq!(reset["required"], json!(["target", "mode"]));
        let modes: Vec<_> = reset["properties"]["mode"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["const"].clone())
            .collect();
        assert_eq!(modes, [json!("soft"), json!("mixed"), json!("hard")]);
        assert_eq!(reset["properties"]["confirm"]["type"], "boolean");

        let lines = schema("index.stage_lines");
        assert_eq!(lines["required"], json!(["path", "lines", "digest"]));
        let line = &lines["properties"]["lines"]["items"]["oneOf"];
        assert_eq!(line[0]["required"], json!(["old"]));
        assert_eq!(line[1]["required"], json!(["new"]));

        // Subschemas inline: an agent reads each command without resolving `$ref`.
        let rebase = schema("rebase.start");
        let step = &rebase["properties"]["steps"]["items"];
        assert_eq!(step["required"], json!(["action", "commit"]));
        assert_eq!(step["properties"]["commit"]["pattern"], "^[0-9a-fA-F]{40}$");
        assert!(!rebase.to_string().contains("$ref"), "{rebase}");

        // The commands left out of the old description are there too.
        for id in [
            "commit.cherry_pick",
            "branch.rename",
            "conflict.resolve",
            "rebase.continue",
        ] {
            schema(id);
        }
        assert_eq!(schema("repo.status").get("properties"), None);
    }

    #[test]
    fn commands_outside_the_repo_are_refused_through_the_flag() {
        let root = temp_root("outside");
        let mut server = server(&root);
        let outside: Vec<_> = list(&mut server, "")
            .into_iter()
            .filter(|c| c["outside_repo"] == true)
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(outside, ["repo.clone", "repo.init", "repo.open"]);
        for id in &outside {
            let params = json!({"url": "/", "path": "/tmp/x"});
            let r = call(
                &mut server,
                "execute_command",
                json!({"id": id, "params": params}),
            );
            assert_eq!(r["isError"], true, "{id}");
            let text = r["content"][0]["text"].as_str().unwrap();
            assert!(text.contains("not available over MCP"), "{text}");
            assert!(text.contains("open_repo"), "{text}");
        }
        // `open_repo` itself still goes through `repo.open`.
        let r = call(&mut server, "open_repo", json!({"path": "repo"}));
        assert_eq!(r["isError"], false, "{r}");
    }

    #[test]
    fn execute_command_description_points_to_list_commands() {
        let tools = tools();
        let execute = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "execute_command")
            .unwrap();
        let text = execute["description"].as_str().unwrap();
        for rule in [
            "list_commands",
            "writes",
            "confirm",
            "expected",
            "outside_repo",
        ] {
            assert!(text.contains(rule), "{rule}: {text}");
        }
    }

    #[test]
    fn paths_outside_root_are_rejected() {
        let root = temp_root("escape");
        let mut server = server(&root);
        for path in ["..", "/etc", "repo/../..", "does-not-exist"] {
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
