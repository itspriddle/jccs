SHELL := /bin/bash
PREFIX ?= /usr/local

.DEFAULT_GOAL := help

CARGO := cargo

##@ Quality

.PHONY: check
check: lint test ## Run all linters and unit tests

.PHONY: lint
lint: fmt ## Format-check and lint
	$(CARGO) clippy --all-targets --locked -- -D warnings

.PHONY: vuln
vuln: ## Audit dependencies: advisories, licenses and sources
	$(CARGO) deny check

.PHONY: fmt
fmt: ## Format code
	$(CARGO) fmt

.PHONY: test
test: ## Run test suite
	$(CARGO) test --locked

##@ Release

.PHONY: release
release: ## Build optimized release binary
	$(CARGO) build --release

##@ Setup

.PHONY: install
install: ## Install to PREFIX (/usr/local by default, requires sudo)
	@test -x target/release/jccs || { echo "Run 'make release' first (without sudo)"; exit 1; }
	@install -d -m 755 "$(DESTDIR)$(PREFIX)/bin"
	@install -m 755 target/release/jccs "$(DESTDIR)$(PREFIX)/bin/jccs"
	@echo "Done. Run \`$(PREFIX)/bin/jccs install' to finish setup."

.PHONY: uninstall
uninstall: ## Remove jccs from PREFIX
	@rm -f "$(DESTDIR)$(PREFIX)/bin/jccs"

##@ Build

.PHONY: build
build: ## Build debug binary for current platform
	$(CARGO) build

.PHONY: build-macos
build-macos: ## Build debug binary for macOS
	$(CARGO) build --target aarch64-apple-darwin

.PHONY: build-linux-intel
build-linux-intel: ## Build debug binary for Linux x86_64
	$(CARGO) build --target x86_64-unknown-linux-musl

.PHONY: build-linux-arm
build-linux-arm: ## Build debug binary for Linux ARM
	$(CARGO) build --target aarch64-unknown-linux-musl

.PHONY: clean
clean: clean-rs clean-worktrees ## Clean up all build artifacts and worktrees

.PHONY: clean-rs
clean-rs: ## Clean up Rust build artifacts
	$(CARGO) clean

.PHONY: clean-worktrees
clean-worktrees: ## Remove linked Git worktrees (skips dirty or locked ones)
	@git worktree list --porcelain \
		| awk 'NR > 1 && /^worktree / { sub(/^worktree /, ""); print }' \
		| while IFS= read -r wt; do git worktree remove "$$wt" && echo "removed $$wt" || true; done
	@git worktree prune
	@git for-each-ref --format='%(refname:short)' 'refs/heads/worktree-agent-*' \
		| while IFS= read -r br; do git branch -d "$$br" 2>/dev/null || echo "kept $$br (unmerged or checked out)"; done

.PHONY: help
help: ## Show this help
	@awk 'BEGIN {FS = ":.*##"} /^##@/ {printf "\n\033[1m%s\033[0m\n", substr($$0, 5)} /^[a-zA-Z0-9_-]+:.*##/ {printf "  \033[36m%-17s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)
