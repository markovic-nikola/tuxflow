.PHONY: help dev run run-mcp build build-release test fmt clippy lint deb install uninstall clean release

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*##' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

dev: ## Live reload with cargo-watch (debug build)
	@command -v cargo-watch >/dev/null 2>&1 || { echo "Install cargo-watch first: cargo install cargo-watch"; exit 1; }
	cargo watch -x run

run: ## Run the app, release (debug misrepresents terminal latency)
	cargo run --release

run-mcp: ## Run MCP server binary
	cargo run --bin tuxflow-mcp

build: ## Debug build of the app (tuxflow + tuxflow-mcp)
	cargo build -p tuxflow

build-release: ## Release build of the app
	cargo build --release -p tuxflow

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

release: ## Bump patch version, tag, and push
	./scripts/release.sh

clean: ## Clean build artifacts
	cargo clean
