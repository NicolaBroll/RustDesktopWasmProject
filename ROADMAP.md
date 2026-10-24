# Roadmap

## First iteration

- [x] Workspace with `core`, `domain`, `engine`, `ui`, `mcp` and the three apps.
- [x] `cargo xtask ci` green: formatting, lint, tests, layering, wasm.
- [x] The same window opens on desktop and with `trunk serve`.
- [x] `Platform` with `pick_file` and `save_file`, native (`rfd`) and web (file input / `Blob` download) implementations.
- [x] Registry commands run from UI, CLI and MCP.
- [x] The web version was removed in S31d (`impl-docs/ROADMAP-git.md`): desktop only.

## Next steps

- [x] Packaging (`packaging/`): Linux AppImage and Windows zip with `SHA256SUMS.txt` (S37); deb, signed MSI/DMG later.
- [ ] `cargo deny check` in CI (configuration already in `deny.toml`).
- [ ] Desktop: non-blocking file dialogs (`rfd::AsyncFileDialog`).
- [ ] UI control for agents (off by default, loopback only, token).
