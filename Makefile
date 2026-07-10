# Makefile for agentic-tui (multi-stage agentic orchestrator)
#
# Common targets for building, running, and checking the project.
#
# The tool is a Cargo workspace: `crates/server` is the axum server and CLI
# binary (`agentic-tui`), `crates/shared` holds serde state shared with the
# web crate, and `crates/web` is a wasm32-only Leptos app built with trunk
# into `crates/web/dist`. The server embeds that `dist/` output
# via rust-embed at compile time, so `dist/` must exist before the server
# crate compiles. `web` is a workspace member but is excluded from the
# workspace default-members, so plain `cargo build`, `cargo test`, and
# `cargo clippy --all-targets` do not touch it; it is built with `trunk
# build` and type-checked with `cargo check -p web --target
# wasm32-unknown-unknown`.

CARGO ?= cargo
BIN   := agentic-tui

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help message
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

.PHONY: web
web: ## Build the Leptos web UI with trunk (installs Tailwind deps, then trunk runs the Tailwind pre_build hook)
	cd crates/web && npm install --no-audit --no-fund && trunk build --release

.PHONY: build
build: web ## Build the web UI, then a debug server binary
	$(CARGO) build

.PHONY: release
release: web ## Build the web UI, then an optimized release server binary
	$(CARGO) build --release

.PHONY: run
run: ## Run the server (starts a loopback server and opens the browser)
	$(CARGO) run -p $(BIN)

.PHONY: check
check: web ## Build the web UI, then type-check the native crates without producing a binary
	$(CARGO) check

.PHONY: fmt
fmt: ## Format the source with rustfmt
	$(CARGO) fmt

.PHONY: fmt-check
fmt-check: ## Verify formatting without modifying files
	$(CARGO) fmt --check

.PHONY: lint
lint: web ## Build the web UI, then run clippy with warnings denied on the native crates
	$(CARGO) clippy --all-targets -- -D warnings

.PHONY: test
test: web ## Build the web UI, then run the test suite for the native crates
	$(CARGO) test

.PHONY: test-unit
test-unit: web ## Run unit tests only (in-crate #[cfg(test)] modules)
	$(CARGO) test --lib

.PHONY: test-e2e
test-e2e: web ## Run end-to-end integration tests (crates/server/tests)
	$(CARGO) test --test '*'

.PHONY: test-e2e-browser
test-e2e-browser: release ## Run the Playwright browser smoke test against the release binary
	cd e2e && npm install && npx playwright install chromium && npx playwright test

.PHONY: web-check
web-check: ## Type-check the web crate for its wasm32 target
	$(CARGO) check -p web --target wasm32-unknown-unknown

.PHONY: verify
verify: web fmt-check lint test web-check ## Build the web UI, then run formatting, lint, test, and web wasm checks

.PHONY: clean
clean: ## Remove build artifacts
	$(CARGO) clean
	rm -rf crates/web/dist
