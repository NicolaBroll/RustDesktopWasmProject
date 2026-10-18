#!/usr/bin/env bash
set -euo pipefail

# Named volumes are created root-owned: hand the Claude config dir to the container user.
sudo chown -R "$(id -u):$(id -g)" "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"

# Google Chrome stable + system deps, used by the Playwright MCP server (see .mcp.json).
# Chrome stable is not tied to a specific Playwright version, unlike the bundled Chromium.
npx -y playwright@latest install --with-deps chrome

# Pre-fetch the Playwright MCP server so the first Claude session doesn't wait for it.
npm install -g @playwright/mcp@latest

# Toolchain web per egui/wasm: target wasm32, clippy/rustfmt e trunk (vedi `make setup`).
make setup
