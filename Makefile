.PHONY: all build release webui webui-release test test-api test-server test-webui test-acme-pebble test-discovery-e2e test-runtime-docker lint fmt fmt-check check check-fast run clean cdn-snapshot

CARGO ?= cargo
TRUNK ?= trunk
WEBUI_DIR := r3v3rs3-webui
PEBBLE_COMPOSE := r3v3rs3/tests/pebble/docker-compose.yml
PEBBLE_CA := target/pebble/pebble.minica.pem
DISCOVERY_COMPOSE := docker compose -f r3v3rs3/tests/discovery/docker-compose.yml
DISCOVERY_DIR := target/discovery

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
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 -p r3v3rs3-api -p r3v3rs3-webui --all-features --no-fail-fast

test-api:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3-api --all-features

test-server:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 --all-features

test-webui:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3-webui --all-features

# Runs the ACME DNS-01 and TLS-ALPN-01 end-to-end tests against Pebble in Docker, then removes the
# containers. Pebble validates TLS-ALPN-01 on port 5001 of the Docker host address.
test-acme-pebble:
	docker compose -f $(PEBBLE_COMPOSE) up -d
	mkdir -p $(dir $(PEBBLE_CA))
	docker compose -f $(PEBBLE_COMPOSE) cp pebble:/test/certs/pebble.minica.pem $(PEBBLE_CA)
	PEBBLE_HOST_IP=$$(docker run --rm --add-host=host.docker.internal:host-gateway busybox:latest awk '/host.docker.internal/{print $$1}' /etc/hosts) \
		SSL_CERT_FILE=$(CURDIR)/$(PEBBLE_CA) CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 --test acme_pebble_test -- --ignored; \
		status=$$?; docker compose -f $(PEBBLE_COMPOSE) down; exit $$status

# Runs the service discovery end-to-end tests against Consul and etcd in Docker and against
# the local Docker engine, then removes the containers.
test-discovery-e2e:
	$(DISCOVERY_COMPOSE) up -d --wait && \
		CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 --test discovery_e2e_test -- --ignored; \
		status=$$?; $(DISCOVERY_COMPOSE) down -v; exit $$status

# Runs the Docker runtime and the deployment platform against the local Docker Engine. The tests
# remove their containers, networks and images.
test-runtime-docker:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 --test runtime_docker_test --test platform_deploy_test -- --ignored
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 --lib on_the_docker_engine -- --ignored

lint:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

check: fmt-check lint test webui

# The fast local loop: type-checks every target without codegen and runs no tests. Use it
# between edits; run the full `make check` before a push.
check-fast:
	$(CARGO) check --workspace --all-targets --all-features

run:
	$(CARGO) run --bin r3v3rs3 -- start

# Downloads the CDN IP ranges into r3v3rs3/data/cdn-ranges.json.
cdn-snapshot:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 --lib cdn::fetch::tests::update_embedded_snapshot -- --ignored --exact

clean:
	$(CARGO) clean
