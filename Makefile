# Makefile for agentic-tui (multi-stage agentic orchestrator)
#
# Common targets for building, running, and checking the project.
# Override GOAL and WORKSPACE when running the tool, e.g.:
#   make run GOAL="Add a health check endpoint" WORKSPACE=greentic
#
# The `web` crate (crates/web) is a wasm32-only Leptos app built with trunk.
# It is excluded from the workspace default-members, so plain `make build`,
# `make test`, and `make verify` do not touch it. The server embeds its
# `dist/` output via rust-embed, so on a fresh clone run `make web` once
# before building the server with `--web` support; without a `dist/`
# directory the server crate fails to compile.

CARGO ?= cargo
BIN   := agentic-tui

# Optional goal and workspace for `make run`. Leave GOAL empty to enter the
# goal in the TUI, the same as running the binary without a goal argument.
GOAL ?=
WORKSPACE ?=

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help message
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

.PHONY: build
build: ## Build a debug binary
	$(CARGO) build

.PHONY: release
release: ## Build an optimized release binary
	$(CARGO) build --release

.PHONY: run
run: ## Run the orchestrator (GOAL="..." WORKSPACE=name|path; omit GOAL to enter it in the TUI)
	$(CARGO) run -- $(if $(GOAL),"$(GOAL)",) $(if $(WORKSPACE),--workspace "$(WORKSPACE)",)

.PHONY: check
check: ## Type-check without producing a binary
	$(CARGO) check

.PHONY: fmt
fmt: ## Format the source with rustfmt
	$(CARGO) fmt

.PHONY: fmt-check
fmt-check: ## Verify formatting without modifying files
	$(CARGO) fmt --check

.PHONY: lint
lint: ## Run clippy with warnings denied
	$(CARGO) clippy --all-targets -- -D warnings

.PHONY: test
test: ## Run the test suite
	$(CARGO) test

.PHONY: verify
verify: fmt-check lint test ## Run formatting, lint, and test checks

.PHONY: web
web: ## Build the Leptos web UI with trunk (required once before the server can embed it)
	cd crates/web && trunk build

.PHONY: web-check
web-check: ## Type-check the web crate for its wasm32 target
	$(CARGO) check -p web --target wasm32-unknown-unknown

.PHONY: clean
clean: ## Remove build artifacts
	$(CARGO) clean
