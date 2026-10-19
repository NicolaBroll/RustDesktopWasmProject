# Roadmap

## Prima iterazione

- [x] Workspace con `core`, `domain`, `engine`, `ui`, `mcp` e le tre app.
- [x] `cargo xtask ci` verde: formattazione, lint, test, layering, wasm.
- [x] La stessa finestra si apre su desktop e con `trunk serve`.
- [x] `Platform` con `pick_file` e `save_file`, implementazioni nativa (`rfd`) e web (input file / download `Blob`).
- [x] Comandi del registro eseguiti da UI, CLI e MCP.

## Prossimi passi

- [ ] Packaging (`packaging/`): AppImage/deb e zip del sito web con `SHA256SUMS.txt`, poi MSI/DMG con firma.
- [ ] `cargo deny check` nella CI (configurazione già in `deny.toml`).
- [ ] Web: gestire l'annullamento del dialogo file (evento `cancel`) e prototipare accessibilità, IME e incolla.
- [ ] Desktop: dialoghi file non bloccanti (`rfd::AsyncFileDialog`).
- [ ] Controllo della UI per agenti (spento di default, solo loopback, token).
