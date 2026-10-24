WASM_TARGET := wasm32-unknown-unknown
# Web server port: it is the one forwarded by the devcontainer (forwardPorts in
# .devcontainer/devcontainer.json). If you change it, update it there too.
PORT        ?= 8080
SHOT        ?= target/desktop.png
REPO        ?= .
WAIT        ?= 6

.DEFAULT_GOAL := help
.PHONY: help setup run desktop-shot cli build release web-serve web-build web-release web-preview \
        check fmt fmt-check clippy test layers ci clean

help: ## Show this list
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

setup: ## Install the wasm target and trunk
	rustup target add $(WASM_TARGET)
	rustup component add clippy rustfmt
	command -v trunk >/dev/null || cargo install --locked trunk

# --- Desktop and CLI ---------------------------------------------------------

run: ## Run the desktop app (debug)
	cargo run -p project

desktop-shot: ## Headless screenshot of the desktop app -> $(SHOT) (REPO=. to open, WAIT=6 seconds)
	cargo build -q -p project
	env -u WAYLAND_DISPLAY LIBGL_ALWAYS_SOFTWARE=1 xvfb-run -a -s "-screen 0 1280x800x24" sh -c \
		'target/debug/project "$(REPO)" & pid=$$!; sleep $(WAIT); import -window root "$(SHOT)"; kill $$pid'
	@echo "screenshot: $(SHOT)"

cli: ## Run the CLI (e.g. make cli ARGS="commands")
	cargo run -q -p project-cli -- $(or $(ARGS),--help)

build: ## Build the whole workspace (debug)
	cargo build --workspace

release: ## Optimized build of desktop, CLI and web site
	cargo xtask release

# --- Web (wasm) ------------------------------------------------------------

web-serve: ## Development server with auto-reload at http://localhost:PORT (default 8080)
	trunk serve --port $(PORT)

web-build: ## Web build (debug) -> dist/
	trunk build

web-release: ## Optimized web build -> dist/
	trunk build --release

web-preview: web-release ## Serve the release build at http://localhost:PORT (default 8080)
	python3 -m http.server -d dist $(PORT)

# --- Quality ---------------------------------------------------------------

check: ## cargo check for desktop and wasm
	cargo check --workspace
	cargo check -p project-ui -p project-web --target $(WASM_TARGET)

fmt: ## Format the code
	cargo fmt --all

fmt-check: ## Check formatting without modifying
	cargo fmt --all -- --check

clippy: ## Lint for desktop and wasm (warning = error)
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy -p project-ui -p project-web --target $(WASM_TARGET) -- -D warnings

test: ## Run the tests
	cargo test --workspace

layers: ## Check the layering between crates
	cargo xtask layers

ci: ## All checks, as in CI (cargo xtask ci)
	cargo xtask ci

clean: ## Remove target/ and dist/
	cargo clean
	rm -rf dist
