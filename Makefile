CARGO ?= cargo

.PHONY: build test lint fmt

build:
	$(CARGO) build --release

test:
	$(CARGO) test --workspace

lint:
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings

fmt:
	$(CARGO) fmt --all
