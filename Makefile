.PHONY: all build release webui webui-release test test-api test-server lint fmt fmt-check check run clean cdn-snapshot

CARGO ?= cargo
TRUNK ?= trunk
WEBUI_DIR := r3v3rs3-webui

all: build

build:
	$(CARGO) build --workspace --all-targets --all-features

release: webui-release
	$(CARGO) build --bin r3v3rs3 --release

webui:
	cd $(WEBUI_DIR) && $(TRUNK) build

webui-release:
	cd $(WEBUI_DIR) && $(TRUNK) build --cargo-profile web-release --release

test:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 -p r3v3rs3-api --all-features --no-fail-fast

test-api:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3-api --all-features

test-server:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 --all-features

lint:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

check: fmt-check lint test webui

run:
	$(CARGO) run --bin r3v3rs3 -- start

# Downloads the CDN IP ranges into r3v3rs3/data/cdn-ranges.json.
cdn-snapshot:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 --lib cdn::fetch::tests::update_embedded_snapshot -- --ignored --exact

clean:
	$(CARGO) clean
