# Architettura a strati in Rust: desktop e WebAssembly

Guida di partenza per un nuovo progetto strutturato come PdfCraft e GridCraft: motore indipendente dalla UI, livelli numerati con dipendenze solo verso il basso, un solo frontend egui compilato nativo e wasm.

Convenzioni: `myapp` è il nome segnaposto. Le versioni delle dipendenze e alcune firme (in particolare `eframe` e `cargo_metadata`) cambiano spesso: **verifica ogni snippet sulla versione che installi**. Gli snippet sono schemi di partenza, non codice testato.

## 1. Livelli

| Livello | Crate | Contenuto |
| --- | --- | --- |
| L0 | `myapp-core` | tipi di base, errori, parsing dell'input |
| L2 | `myapp-domain` | modello dei dati, formati di file, logica di dominio |
| L5 | `myapp-engine` | sessione, catalogo comandi, undo, I/O tramite trait |
| L6 | `myapp-ui` | interfaccia egui, indipendente dal target |
| L6 | `myapp-mcp` | server MCP |
| L7 | `myapp`, `myapp-cli`, `myapp-web` | eseguibili sottili: avvio e collegamento |

I numeri L1, L3 e L4 restano liberi per livelli futuri (per esempio un renderer neutro rispetto al toolkit). Regola unica: un crate importa solo da livelli **strettamente inferiori**. Due crate dello stesso livello non si importano a vicenda.

## 2. Struttura del repository

```
myapp/
├─ Cargo.toml                 workspace
├─ .cargo/config.toml         alias per xtask
├─ crates/
│  ├─ core/        (myapp-core)
│  ├─ domain/      (myapp-domain)
│  ├─ engine/      (myapp-engine)
│  ├─ ui/          (myapp-ui)
│  └─ mcp/         (myapp-mcp)
├─ apps/
│  ├─ myapp/       desktop
│  ├─ myapp-cli/
│  └─ myapp-web/   wasm, build con trunk
├─ xtask/          ci, layers, release
├─ packaging/  docs/  assets/  .github/
└─ AGENTS.md  ATTRIBUTION.md  ROADMAP.md  clippy.toml  rustfmt.toml  deny.toml
```

`Cargo.toml` radice:

```toml
[workspace]
resolver = "2"
members = ["crates/*", "apps/*", "xtask"]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
myapp-core   = { path = "crates/core" }
myapp-domain = { path = "crates/domain" }
myapp-engine = { path = "crates/engine" }
myapp-ui     = { path = "crates/ui" }
myapp-mcp    = { path = "crates/mcp" }
```

`.cargo/config.toml`:

```toml
[alias]
xtask = "run --package xtask --"
```

## 3. Controllo automatico del layering (`xtask layers`)

Dipendenze di `xtask/Cargo.toml`: `cargo_metadata`, `anyhow`.

```rust
use cargo_metadata::{DependencyKind, MetadataCommand};
use std::collections::HashMap;

fn levels() -> HashMap<&'static str, u8> {
    HashMap::from([
        ("myapp-core", 0),
        ("myapp-domain", 2),
        ("myapp-engine", 5),
        ("myapp-ui", 6),
        ("myapp-mcp", 6),
        ("myapp", 7),
        ("myapp-cli", 7),
        ("myapp-web", 7),
    ])
}

pub fn check_layers() -> anyhow::Result<()> {
    let metadata = MetadataCommand::new().exec()?;
    let levels = levels();
    let mut errors = Vec::new();

    for pkg in metadata.workspace_packages() {
        let name = pkg.name.to_string();
        let Some(&mine) = levels.get(name.as_str()) else {
            if name != "xtask" {
                errors.push(format!("{name}: livello non dichiarato"));
            }
            continue;
        };
        for dep in pkg.dependencies.iter().filter(|d| d.kind == DependencyKind::Normal) {
            if let Some(&theirs) = levels.get(dep.name.as_str()) {
                if theirs >= mine {
                    errors.push(format!("{name} (L{mine}) dipende da {} (L{theirs})", dep.name));
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("violazioni del layering:\n  {}", errors.join("\n  "))
    }
}
```

`cargo xtask ci` esegue in ordine: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `check_layers()` e `cargo check -p myapp-ui --target wasm32-unknown-unknown`. Lo stesso comando va usato in locale e nel workflow di CI.

## 4. Motore: stato, comandi, I/O

### Stato a copia su scrittura e undo

```rust
// crates/engine/src/session.rs
use std::sync::Arc;
use myapp_domain::Document;

pub struct Session {
    current: Arc<Document>,
    undo: Vec<Arc<Document>>,
    redo: Vec<Arc<Document>>,
}

impl Session {
    pub fn new(doc: Document) -> Self {
        Self { current: Arc::new(doc), undo: vec![], redo: vec![] }
    }

    /// Ogni modifica passa da qui: snapshot quasi gratuito grazie ad Arc.
    pub fn edit<R>(&mut self, f: impl FnOnce(&mut Document) -> R) -> R {
        self.undo.push(Arc::clone(&self.current));
        self.redo.clear();
        f(Arc::make_mut(&mut self.current))
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(&mut self.current, prev));
                true
            }
            None => false,
        }
    }

    pub fn document(&self) -> &Document {
        &self.current
    }
}
```

`Document` deve implementare `Clone`. Per documenti grandi sostituisci i campi con strutture persistenti (crate `im` o `rpds`) per rendere la clonazione davvero economica.

### Catalogo di comandi

Un comando ha un id stabile e parametri JSON. UI, CLI, MCP e test usano lo stesso registro.

```rust
// crates/engine/src/commands.rs
use crate::{EngineError, Session};
use serde_json::Value;
use std::collections::BTreeMap;

pub struct CommandSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub params_help: &'static str,
}

pub type Handler = fn(&mut Session, Value) -> Result<Value, EngineError>;

#[derive(Default)]
pub struct Registry {
    map: BTreeMap<&'static str, (CommandSpec, Handler)>,
}

impl Registry {
    pub fn register(&mut self, spec: CommandSpec, handler: Handler) {
        self.map.insert(spec.id, (spec, handler));
    }

    pub fn list(&self) -> impl Iterator<Item = &CommandSpec> {
        self.map.values().map(|(spec, _)| spec)
    }

    pub fn run(&self, id: &str, session: &mut Session, params: Value) -> Result<Value, EngineError> {
        let (_, handler) = self.map.get(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?;
        handler(session, params)
    }
}
```

Regola: ogni pulsante, voce di menu e scorciatoia della UI invoca `Registry::run`. Niente logica di modifica dentro i callback grafici.

### Interfaccia di piattaforma

```rust
// crates/engine/src/platform.rs
pub struct Picked { pub name: String, pub bytes: Vec<u8> }

#[derive(Clone, Copy, Default)]
pub struct Capabilities { pub print: bool, pub reveal_in_file_manager: bool, pub window_control: bool }

pub trait Platform: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    /// Asincrono sul web: il risultato arriva tramite callback.
    fn pick_file(&self, on_done: Box<dyn FnOnce(Option<Picked>) + Send>);
    fn save_file(&self, suggested_name: &str, bytes: Vec<u8>);
}
```

Implementazione nativa nell'app desktop (dialogo di sistema, ad esempio con la crate `rfd`), implementazione web nell'app wasm (input file nascosto e download tramite `Blob`). La UI legge `capabilities()` e nasconde i comandi non disponibili.

## 5. Interfaccia e due target

`crates/ui` espone `App`, che implementa `eframe::App` e riceve un `Arc<dyn Platform>`; non contiene `cfg(target_arch)` se non in casi eccezionali.

### Desktop (`apps/myapp/src/main.rs`)

```rust
use std::sync::Arc;

fn main() -> eframe::Result {
    let platform = Arc::new(NativePlatform::default());
    eframe::run_native(
        "MyApp",
        eframe::NativeOptions::default(),
        // La firma del creatore cambia tra versioni di eframe: Ok(Box::new(..)) nelle recenti.
        Box::new(move |cc| Ok(Box::new(myapp_ui::App::new(cc, platform.clone())))),
    )
}
```

### Web (`apps/myapp-web/src/main.rs`)

```rust
#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;
    use std::sync::Arc;

    wasm_bindgen_futures::spawn_local(async {
        let canvas = web_sys::window().unwrap().document().unwrap()
            .get_element_by_id("app_canvas").unwrap()
            .dyn_into::<web_sys::HtmlCanvasElement>().unwrap();

        eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(myapp_ui::App::new(cc, Arc::new(WebPlatform::default()))))),
            )
            .await
            .expect("avvio non riuscito");
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
```

`apps/myapp-web/index.html`:

```html
<!DOCTYPE html>
<html lang="it">
<head>
  <meta charset="utf-8" />
  <title>MyApp</title>
  <link data-trunk rel="rust" />
</head>
<body>
  <canvas id="app_canvas"></canvas>
</body>
</html>
```

Build web: installa `trunk` e il target (`rustup target add wasm32-unknown-unknown`), poi `cd apps/myapp-web && trunk serve --release`. Per il rilascio, `trunk build --release` produce un sito statico.

Attenzione: la UI egui sul web è disegnata su canvas. Accessibilità, selezione del testo, IME e incolla meritano un prototipo precoce.

## 6. Superficie per agenti

- **CLI** (`myapp-cli`): `commands --search <testo>`, `run --cmd <id>=<json> --out file`, `eval`. Usa `clap`.
- **Server MCP** (`myapp-mcp`): parte solo con `myapp-cli mcp`, parla su stdin/stdout, nessuna porta di rete, opzione `--root <dir>` che confina tutti i percorsi. Espone tool come `list_commands` ed `execute_command`, che raggiunge tutto il catalogo.
- **Controllo della UI** (opzionale, più avanti): spento di default, solo loopback, token in un file leggibile dal solo utente.

## 7. Qualità, licenze, packaging

- `rustfmt`, Clippy con warning come errori, test unitari e di proprietà (`proptest`) sul motore.
- `cargo deny` con `deny.toml` per licenze e vulnerabilità; `ATTRIBUTION.md` per ogni asset.
- Corpus di file reali e, dove esiste, un'implementazione indipendente come oracolo di confronto.
- Pacchetti per piattaforma (MSI, DMG, AppImage, Flatpak, deb, rpm, zip del sito web) con `SHA256SUMS.txt`; firma su Windows e macOS. Parti da due piattaforme desktop e dal web.

## 8. Prima iterazione (checklist)

- [ ] Workspace con `core`, `engine`, `ui` e le tre app, anche solo con una finestra vuota.
- [ ] `cargo xtask ci` verde: formattazione, lint, test, layering, `cargo check` per wasm.
- [ ] La stessa finestra si apre su desktop e con `trunk serve`.
- [ ] `Platform` definita con `pick_file` e `save_file` e le due implementazioni.
- [ ] Un comando di esempio nel registro, eseguito sia dalla UI sia dalla CLI.

## 9. Da verificare prima di partire

- Versioni correnti di `eframe`, `egui`, `wasm-bindgen`, `trunk` e `cargo_metadata`.
- Quale backend grafico usa `eframe` per il web nella versione scelta (wgpu o glow).
- Se il dominio scelto ha già una crate matura da usare al posto di scrivere tutto da zero; se sì, isolala dietro un'interfaccia e dichiaralo nel README.
