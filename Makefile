WASM_TARGET := wasm32-unknown-unknown
# Porta del server web: è quella inoltrata dal devcontainer (forwardPorts in
# .devcontainer/devcontainer.json). Se la cambi, aggiornala anche lì.
PORT        ?= 8080

.DEFAULT_GOAL := help
.PHONY: help setup run cli build release web-serve web-build web-release web-preview \
        check fmt fmt-check clippy test layers ci clean

help: ## Mostra questo elenco
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

setup: ## Installa target wasm e trunk
	rustup target add $(WASM_TARGET)
	rustup component add clippy rustfmt
	command -v trunk >/dev/null || cargo install --locked trunk

# --- Desktop e CLI ---------------------------------------------------------

run: ## Avvia l'app desktop (debug)
	cargo run -p project

cli: ## Esegue la CLI (es. make cli ARGS="commands")
	cargo run -q -p project-cli -- $(or $(ARGS),--help)

build: ## Build di tutto il workspace (debug)
	cargo build --workspace

release: ## Build ottimizzata desktop, CLI e sito web
	cargo xtask release

# --- Web (wasm) ------------------------------------------------------------

web-serve: ## Server di sviluppo con auto-reload su http://localhost:PORT (default 8080)
	trunk serve --port $(PORT)

web-build: ## Build web (debug) -> dist/
	trunk build

web-release: ## Build web ottimizzata -> dist/
	trunk build --release

web-preview: web-release ## Serve la build di release su http://localhost:PORT (default 8080)
	python3 -m http.server -d dist $(PORT)

# --- Qualità ---------------------------------------------------------------

check: ## cargo check per desktop e wasm
	cargo check --workspace
	cargo check -p project-ui -p project-web --target $(WASM_TARGET)

fmt: ## Formatta il codice
	cargo fmt --all

fmt-check: ## Verifica la formattazione senza modificare
	cargo fmt --all -- --check

clippy: ## Lint per desktop e wasm (warning = errore)
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy -p project-ui -p project-web --target $(WASM_TARGET) -- -D warnings

test: ## Esegue i test
	cargo test --workspace

layers: ## Verifica il layering tra i crate
	cargo xtask layers

ci: ## Tutti i controlli, come in CI (cargo xtask ci)
	cargo xtask ci

clean: ## Rimuove target/ e dist/
	cargo clean
	rm -rf dist
