# Roadmap

## First iteration

- [x] Workspace with `core`, `domain`, `engine`, `ui`, `mcp` and the three apps.
- [x] `cargo xtask ci` green: formatting, lint, tests, layering, wasm.
- [x] The same window opens on desktop and with `trunk serve`.
- [x] `Platform` with `pick_file` and `save_file`, native (`rfd`) and web (file input / `Blob` download) implementations.
- [x] Registry commands run from UI, CLI and MCP.

## Next steps

- [ ] Packaging (`packaging/`): AppImage/deb and a zip of the web site with `SHA256SUMS.txt`, then signed MSI/DMG.
- [ ] `cargo deny check` in CI (configuration already in `deny.toml`).
- [ ] Web: handle cancelling the file dialog (`cancel` event) and prototype accessibility, IME and paste.
- [ ] Desktop: non-blocking file dialogs (`rfd::AsyncFileDialog`).
- [ ] UI control for agents (off by default, loopback only, token).
