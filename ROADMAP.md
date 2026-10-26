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
- [x] `cargo deny check` in CI: licenses, bans and sources in `cargo xtask ci`, advisories weekly (S38).
- [x] Desktop: non-blocking file dialogs (`rfd::AsyncFileDialog`): S39.
- [x] Refresh when the repo changes outside the app (git directory watched with `notify`, focus): S40.
- [ ] Windows MSI (S41: built, waiting for the hand test on Windows), macOS DMG (S42), signing (S43).
- [ ] UI control for agents (off by default, loopback only, token): S44, only if still needed.

Steps and order in [impl-docs/ROADMAP-git.md](impl-docs/ROADMAP-git.md), phase 6.

## Code quality

- [x] Review cleanups (Q00–Q06): `MemoryBackend` only in tests, dead code, `cargo doc` in CI,
  shared helpers in engine and UI, ref prefixes in the domain, git 2.43 fixes.
- [ ] Contracts between the layers (Q10–Q14): git versions in CI, typed results and parameters,
  parameter schema for MCP, `GitBackend` by area.
- [ ] UI structure (Q20–Q22) and optional lints (Q30–Q31).

Steps in [impl-docs/ROADMAP-quality.md](impl-docs/ROADMAP-quality.md).
