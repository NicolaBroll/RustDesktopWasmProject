SHOT        ?= target/desktop.png
REPO        ?= .
WAIT        ?= 6
# Virtual display of `desktop-start`: xdotool and import use it with DISPLAY=$(XDISPLAY).
XDISPLAY    ?= :99
DEMO        ?= target/demo-repo

.DEFAULT_GOAL := help
.PHONY: help setup run desktop-shot desktop-start desktop-stop demo-repo cli build release appimage windows \
        check fmt fmt-check clippy test layers ci clean

help: ## Show this list
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

setup: ## Install clippy, rustfmt and cargo-deny
	rustup component add clippy rustfmt
	cargo deny --version >/dev/null 2>&1 || cargo install --locked cargo-deny

# --- Desktop and CLI ---------------------------------------------------------

run: ## Run the desktop app (debug)
	cargo run -p project

desktop-shot: ## Headless screenshot of the desktop app -> $(SHOT) (REPO=. to open, WAIT=6 seconds)
	cargo build -q -p project
	env -u WAYLAND_DISPLAY LIBGL_ALWAYS_SOFTWARE=1 xvfb-run -a -s "-screen 0 1280x800x24" sh -c \
		'target/debug/project "$(REPO)" & pid=$$!; sleep $(WAIT); import -window root "$(SHOT)"; kill $$pid'
	@echo "screenshot: $(SHOT)"

desktop-start: ## Desktop app on Xvfb $(XDISPLAY), left running to explore it (REPO=)
	cargo build -q -p project
	@$(MAKE) -s desktop-stop
	@mkdir -p target
	setsid Xvfb $(XDISPLAY) -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 & echo $$! > target/xvfb.pid
	@sleep 1
	env -u WAYLAND_DISPLAY DISPLAY=$(XDISPLAY) LIBGL_ALWAYS_SOFTWARE=1 setsid target/debug/project "$(REPO)" \
		>target/desktop.log 2>&1 & echo $$! > target/desktop.pid
	@# Without a window manager the keyboard reaches the window only if it is given focus.
	DISPLAY=$(XDISPLAY) timeout 60 xdotool search --sync --name BinariGit windowfocus --sync
	@echo "app on $(XDISPLAY): DISPLAY=$(XDISPLAY) xdotool mousemove X Y click 1 | type 'text' | key Return"
	@echo "screenshot: DISPLAY=$(XDISPLAY) import -window root target/shot.png; stop: make desktop-stop"

desktop-stop: ## Stop the app and the Xvfb of desktop-start
	-@for f in target/desktop.pid target/xvfb.pid; do [ -f $$f ] && kill $$(cat $$f) 2>/dev/null; rm -f $$f; done; true

demo-repo: ## Demo repo to explore the app -> $(DEMO) (recreated every time)
	rm -rf "$(DEMO)" "$(DEMO)-origin.git"
	scripts/demo-repo.sh "$(DEMO)"

cli: ## Run the CLI (e.g. make cli ARGS="commands")
	cargo run -q -p project-cli -- $(or $(ARGS),--help)

build: ## Build the whole workspace (debug)
	cargo build --workspace

release: ## Optimized build of desktop and CLI
	cargo xtask release

appimage: ## Linux AppImage -> target/appimage/BinariGit-<version>-x86_64.AppImage + SHA256SUMS.txt
	packaging/appimage.sh

windows: ## Windows zip (cross-compiled, mingw-w64) -> target/windows/BinariGit-<version>-x86_64-windows.zip
	packaging/windows.sh

# --- Quality ---------------------------------------------------------------

check: ## cargo check of the workspace
	cargo check --workspace

fmt: ## Format the code
	cargo fmt --all

fmt-check: ## Check formatting without modifying
	cargo fmt --all -- --check

clippy: ## Lint (warning = error)
	cargo clippy --workspace --all-targets -- -D warnings

test: ## Run the tests
	cargo test --workspace

layers: ## Check the layering between crates
	cargo xtask layers

ci: ## All checks, as in CI (cargo xtask ci)
	cargo xtask ci

clean: ## Remove target/
	cargo clean
