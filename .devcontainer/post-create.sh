#!/usr/bin/env bash
set -euo pipefail

# Named volumes are created root-owned: hand the Claude config dir to the container user.
sudo chown -R "$(id -u):$(id -g)" "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"

# Google Chrome stable + system deps, used by the Playwright MCP server (see .mcp.json).
# Chrome stable is not tied to a specific Playwright version, unlike the bundled Chromium.
npx -y playwright@latest install --with-deps chrome

# Display virtuale e librerie per avviare l'app desktop eframe/glow senza schermo
# (`make desktop-shot`): xvfb + xauth per `xvfb-run`, x11-utils (xwininfo, xprop) per
# controllare la finestra, imagemagick (`import`) per lo screenshot; python3 serve a
# `make web-preview`.
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
    xvfb xauth x11-utils imagemagick libxkbcommon-x11-0 libgl1-mesa-dri libegl1 python3

# Pre-fetch the Playwright MCP server so the first Claude session doesn't wait for it.
npm install -g @playwright/mcp@latest

# Toolchain web per egui/wasm: target wasm32, clippy/rustfmt e trunk (vedi `make setup`).
make setup
