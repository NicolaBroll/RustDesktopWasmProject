#!/bin/bash
# Startup of cloud Claude Code sessions: prepares the toolchain for `cargo xtask ci`
# (wasm32 target for the wasm clippy, clippy and rustfmt) and trunk for the web build. Locally use `make setup`.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

rustup target add wasm32-unknown-unknown
rustup component add clippy rustfmt

# Prebuilt trunk: `cargo install` would take minutes every session.
if ! command -v trunk >/dev/null; then
  curl -sSfL https://github.com/trunk-rs/trunk/releases/latest/download/trunk-x86_64-unknown-linux-gnu.tar.gz \
    | tar xz -C "${CARGO_HOME:-$HOME/.cargo}/bin" trunk
fi
