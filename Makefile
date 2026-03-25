.PHONY: help dev run run-mcp build build-release test fmt clippy lint deb install uninstall clean release

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*##' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

dev: ## Live reload with cargo-watch
	@command -v cargo-watch >/dev/null 2>&1 || { echo "Install cargo-watch first: cargo install cargo-watch"; exit 1; }
	cargo watch -x run

run: ## Run debug build
	cargo run

run-mcp: ## Run MCP server binary
	cargo run --bin tuxflow-mcp

build: ## Debug build
	cargo build

build-release: ## Release build
	cargo build --release

test: ## Run all tests
	cargo test --all

fmt: ## Format code
	cargo fmt --all

clippy: ## Run clippy lints
	cargo clippy --all-targets -- -W clippy::all

lint: ## Run all checks (same as CI)
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -W clippy::all
	cargo test --all

deb: build-release ## Build .deb package
	cargo deb --no-build

install: ## Install to /usr/local
	./scripts/install.sh

uninstall: ## Uninstall from /usr/local
	./scripts/install.sh --uninstall

release: ## Bump version, tag, and push (usage: make release v=0.2.0)
	@test -n "$(v)" || { echo "Usage: make release v=0.2.0"; exit 1; }
	./scripts/release.sh $(v)

clean: ## Clean build artifacts
	cargo clean
