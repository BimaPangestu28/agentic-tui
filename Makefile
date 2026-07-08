# Makefile for agentic-tui (PRD Generator TUI)
#
# Common targets for building, running, and checking the project.
# Override REPO and GOAL when running the tool, e.g.:
#   make run GOAL="Add per-tenant rate limiting" REPO=/path/to/repo

CARGO ?= cargo
BIN   := agentic-tui

# Default goal and repository for `make run`.
GOAL ?= Add per-tenant rate limiting in the API gateway
REPO ?= .

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
run: ## Run the TUI (GOAL="..." REPO=/path)
	$(CARGO) run -- "$(GOAL)" --repo "$(REPO)"

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

.PHONY: clean
clean: ## Remove build artifacts
	$(CARGO) clean
