.PHONY: help run dev run-mcp build build-release test fmt clippy lint deb install uninstall release clean

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*##' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

run: ## Run the app, release (a debug build misrepresents terminal latency)
	cargo run --release

dev: ## Live reload with cargo-watch (debug build)
	@command -v cargo-watch >/dev/null 2>&1 || { echo "Install cargo-watch first: cargo install cargo-watch"; exit 1; }
	cargo watch -x run

run-mcp: ## Run the tuxflow-mcp client against a running app (stdio <-> project socket)
	cargo run --bin tuxflow-mcp

build: ## Debug build (tuxflow + tuxflow-mcp)
	cargo build

build-release: ## Release build
	cargo build --release

test: ## Run every test in the workspace
	cargo test --all

fmt: ## Format code
	cargo fmt --all

clippy: ## Run clippy lints
	cargo clippy --all-targets -- -W clippy::all

lint: ## Run all checks (same as CI)
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -W clippy::all
	cargo test --all

deb: build-release ## Build the .deb package (needs cargo-deb)
	cargo deb --no-build

install: build-release ## Install the release build to /usr/local (PREFIX=... to change)
	./scripts/install.sh $(if $(PREFIX),--prefix $(PREFIX),)

uninstall: ## Remove the installed files
	./scripts/install.sh --uninstall $(if $(PREFIX),--prefix $(PREFIX),)

release: ## Bump patch, tag and push; V=0.3.0 for an explicit version
	./scripts/release.sh $(V)

clean: ## Clean build artifacts
	cargo clean
