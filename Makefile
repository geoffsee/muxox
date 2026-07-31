.PHONY: all build test lint run run-tui run-raw demo fmt stage stage-verify clean help

# Default target
all: build

build: ## Build: Build the muxox binary
	./scripts/build.sh

test: ## Test: Run all tests
	./scripts/test.sh

lint: ## Lint: Run clippy and format checks
	./scripts/lint.sh

run: ## Run: Run muxox with the example configuration
	./scripts/run-example.sh

run-tui: ## Run-tui: Run muxox in TUI mode
	./scripts/run-example.sh --tui

run-raw: ## Run-raw: Run muxox in raw (non-interactive) mode
	./scripts/run-example.sh --raw

demo: ## Demo: Run the example config in TUI mode (showcases env_file, interactive, etc.)
	./scripts/run-example.sh --tui

fmt: ## Fmt: Run cargo fmt --all
	cargo fmt --all

stage: ## Stage: Flatten the workspace into the single publishable crate at publish/muxox
	./scripts/stage-crates-io.sh

stage-verify: stage ## Stage-verify: Stage, then test and package the publishable crate
	cargo test --manifest-path publish/muxox/Cargo.toml
	cargo package --manifest-path publish/muxox/Cargo.toml

clean: ## Clean: Run cargo clean
	cargo clean
	rm -rf publish

help: ## Help: Show this help message
	@echo "Usage: make [target]"
	@echo ""
	@echo "Targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "  %-10s %s\n", $$1, $$2}'
