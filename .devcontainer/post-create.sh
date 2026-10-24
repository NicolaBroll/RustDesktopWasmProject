#!/usr/bin/env bash
set -euo pipefail

# Named volumes are created root-owned: hand the Claude config dir to the container user.
sudo chown -R "$(id -u):$(id -g)" "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"

# Google Chrome stable + system deps, used by `assets/render.sh` (Playwright is only the installer).
# Chrome stable is not tied to a specific Playwright version, unlike the bundled Chromium.
npx -y playwright@latest install --with-deps chrome

# Virtual display and libraries to run the eframe/glow desktop app headless
# (`make desktop-shot`): xvfb + xauth for `xvfb-run`, x11-utils (xwininfo, xprop) to
# inspect the window, imagemagick (`import`) for the screenshot, xdotool to click and type in
# it when exploring the app.
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
    xvfb xauth x11-utils imagemagick xdotool libxkbcommon-x11-0 libgl1-mesa-dri libegl1

# clippy and rustfmt for `cargo xtask ci` (see `make setup`).
make setup
