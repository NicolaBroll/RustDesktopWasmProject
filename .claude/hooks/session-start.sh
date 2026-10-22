#!/bin/bash
# Avvio delle sessioni Claude Code cloud: prepara la toolchain per `cargo xtask ci`
# (target wasm32 per il clippy wasm, clippy e rustfmt) e trunk per la build web. In locale c'è `make setup`.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

rustup target add wasm32-unknown-unknown
rustup component add clippy rustfmt

# trunk precompilato: `cargo install` richiederebbe minuti a ogni sessione.
if ! command -v trunk >/dev/null; then
  curl -sSfL https://github.com/trunk-rs/trunk/releases/latest/download/trunk-x86_64-unknown-linux-gnu.tar.gz \
    | tar xz -C "${CARGO_HOME:-$HOME/.cargo}/bin" trunk
fi
