#!/bin/bash
# Avvio delle sessioni Claude Code cloud: prepara la toolchain per `cargo xtask ci`
# (target wasm32 per il clippy wasm, clippy e rustfmt). In locale c'è `make setup`.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

rustup target add wasm32-unknown-unknown
rustup component add clippy rustfmt
