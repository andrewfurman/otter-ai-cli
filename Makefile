.PHONY: build install test fmt lint

build:
	cargo build --release --manifest-path rust-cli/Cargo.toml

install:
	cargo install --path rust-cli/otter

test:
	cargo test --manifest-path rust-cli/Cargo.toml

fmt:
	cargo fmt --all --manifest-path rust-cli/Cargo.toml

lint:
	cargo clippy --all-targets --manifest-path rust-cli/Cargo.toml
