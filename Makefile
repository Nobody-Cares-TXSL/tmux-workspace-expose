.PHONY: all build build-release install clean test check fmt fmt-check clippy lint plugin-check run help

# Default target
all: check build

# Show available targets
help:
	@printf "Targets:\n"
	@printf "  make build            Build debug binary\n"
	@printf "  make build-release    Build release binary\n"
	@printf "  make install          Install from local checkout\n"
	@printf "  make clean            Remove build artifacts\n"
	@printf "  make test             Run tests\n"
	@printf "  make check            Run fmt, tests, clippy, and plugin checks\n"
	@printf "  make fmt              Format source\n"
	@printf "  make fmt-check        Check formatting\n"
	@printf "  make clippy           Run clippy with warnings denied\n"
	@printf "  make plugin-check     Check tmux plugin entrypoint\n"
	@printf "  make run ARGS=...     Run tmux-expose with arguments\n"

# Build debug version
build:
	cargo build

# Build release version
build-release:
	cargo build --release

# Install to ~/.cargo/bin
# --force: the bin name tmux-expose collides with the upstream release, so a
# plain install fails when an older binary already occupies ~/.cargo/bin.
install:
	cargo install --force --path . --locked

# Clean build artifacts
clean:
	cargo clean

# Run all tests
test:
	cargo test

# Run all local checks
check: fmt-check test clippy plugin-check

# Format code
fmt:
	cargo fmt

# Check formatting
fmt-check:
	cargo fmt --check

# Run clippy
clippy:
	cargo clippy --all-targets --all-features -- -D warnings

# Lint code
lint: fmt-check clippy

# Check tmux plugin entrypoint
plugin-check: build
	test -x tmux.expose.tmux
	bash -n tmux.expose.tmux
	bash tests/plugin_entrypoint_test.sh

# Run with arguments (usage: make run ARGS="--help")
run:
	cargo run -- $(ARGS)
