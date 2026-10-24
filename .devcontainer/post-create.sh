#!/usr/bin/env bash
set -euo pipefail

# Named volumes are created root-owned: hand the Claude config dir to the container user.
sudo chown -R "$(id -u):$(id -g)" "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"

# Google Chrome stable + system deps, used by the Playwright MCP server (see .mcp.json).
# Chrome stable is not tied to a specific Playwright version, unlike the bundled Chromium.
npx -y playwright@latest install --with-deps chrome

# Virtual display and libraries to run the eframe/glow desktop app headless
# (`make desktop-shot`): xvfb + xauth for `xvfb-run`, x11-utils (xwininfo, xprop) to
# inspect the window, imagemagick (`import`) for the screenshot; python3 is needed by
# `make web-preview`.
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
    xvfb xauth x11-utils imagemagick libxkbcommon-x11-0 libgl1-mesa-dri libegl1 python3

# Pre-fetch the Playwright MCP server so the first Claude session doesn't wait for it.
npm install -g @playwright/mcp@latest

# Web toolchain for egui/wasm: wasm32 target, clippy/rustfmt and trunk (see `make setup`).
make setup
