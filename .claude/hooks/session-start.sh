#!/bin/bash
# Startup of cloud Claude Code sessions: prepares the toolchain for `cargo xtask ci`
# (clippy and rustfmt). Locally use `make setup`.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

rustup component add clippy rustfmt
