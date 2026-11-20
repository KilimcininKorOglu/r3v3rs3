.PHONY: all build release webui webui-release test test-api test-server test-webui test-acme-pebble test-discovery-e2e test-runtime-docker test-tproxy lint fmt fmt-check complexity check check-fast deny run clean cdn-snapshot

CARGO ?= cargo
TRUNK ?= trunk
WEBUI_DIR := r3v3rs3-webui
PEBBLE_COMPOSE := r3v3rs3/tests/pebble/docker-compose.yml
PEBBLE_CA := target/pebble/pebble.minica.pem
DISCOVERY_COMPOSE := docker compose -f r3v3rs3/tests/discovery/docker-compose.yml
DISCOVERY_DIR := target/discovery
# The server packages: the r3v3rs3 facade with the binary and the integration tests, and the crates
# that hold the server code.
SERVER_PACKAGES := -p r3v3rs3 -p r3v3rs3-core -p r3v3rs3-proxy -p r3v3rs3-platform -p r3v3rs3-server -p r3v3rs3-admin
SERVER_SOURCES := r3v3rs3/src r3v3rs3/tests r3v3rs3-core/src r3v3rs3-proxy/src r3v3rs3-platform/src r3v3rs3-server/src r3v3rs3-admin/src

all: build

build:
	$(CARGO) build --workspace --all-targets --all-features

release: webui-release
	$(CARGO) build --bin r3-server --release

webui:
	cd $(WEBUI_DIR) && $(TRUNK) build

webui-release:
	cd $(WEBUI_DIR) && $(TRUNK) build --cargo-profile web-release --release

test:
	CARGO_INCREMENTAL=0 $(CARGO) test $(SERVER_PACKAGES) -p r3v3rs3-api -p r3v3rs3-webui --all-features --no-fail-fast

test-api:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3-api --all-features

test-server:
	CARGO_INCREMENTAL=0 $(CARGO) test $(SERVER_PACKAGES) --all-features

test-webui:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3-webui --all-features

# Runs the ACME DNS-01 and TLS-ALPN-01 end-to-end tests against Pebble in Docker, then removes the
# containers. Pebble validates TLS-ALPN-01 on port 5001 of the Docker host address.
test-acme-pebble:
	docker compose -f $(PEBBLE_COMPOSE) up -d
	mkdir -p $(dir $(PEBBLE_CA))
	docker compose -f $(PEBBLE_COMPOSE) cp pebble:/test/certs/pebble.minica.pem $(PEBBLE_CA)
	PEBBLE_HOST_IP=$$(docker run --rm --add-host=host.docker.internal:host-gateway busybox:latest awk '/host.docker.internal/{print $$1}' /etc/hosts) \
		SSL_CERT_FILE=$(CURDIR)/$(PEBBLE_CA) CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 --test it -- --ignored acme_pebble_test::; \
		status=$$?; docker compose -f $(PEBBLE_COMPOSE) down; exit $$status

# Runs the service discovery end-to-end tests against etcd in Docker and against
# the local Docker engine, then removes the containers.
test-discovery-e2e:
	$(DISCOVERY_COMPOSE) up -d --wait && \
		CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 --test it -- --ignored discovery_e2e_test::; \
		status=$$?; $(DISCOVERY_COMPOSE) down -v; exit $$status

# Runs the Docker runtime and the deployment platform against the local Docker Engine. The tests
# remove their containers, networks and images.
test-runtime-docker:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3 --test it -- --ignored runtime_docker_test:: platform_deploy_test::
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3-platform --lib on_the_docker_engine -- --ignored

# Runs the TPROXY range port tests as root in a privileged Linux container, because they install
# an nftables table, a policy route and a network namespace (r3v3rs3/tests/tproxy/run.sh). They
# share that table, so they run one at a time.
TPROXY_TARGET ?= x86_64-unknown-linux-gnu
test-tproxy:
	bin=$$(CARGO_INCREMENTAL=0 $(CARGO) zigbuild -p r3v3rs3 --test it --target $(TPROXY_TARGET) --message-format json \
		| sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | tail -1) && test -n "$$bin" && \
	docker run --rm --privileged --platform linux/amd64 -v "$$bin":/it:ro \
		-v $(CURDIR)/r3v3rs3/tests/tproxy/run.sh:/run.sh:ro debian:stable-slim sh /run.sh /it

lint:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

# The CI complexity gate: every function, the integration tests included, stays at or below
# a cyclomatic complexity of 10.
complexity:
	lizard -l rust -C 10 -w $(SERVER_SOURCES) r3v3rs3-api/src r3v3rs3-cli/src r3v3rs3-webui/src

check: fmt-check lint complexity test webui

# The fast local loop: type-checks every target without codegen and runs no tests. Use it
# between edits; run the full `make check` before a push.
check-fast:
	$(CARGO) check --workspace --all-targets --all-features

deny:
	$(CARGO) deny check

run:
	$(CARGO) run --bin r3-server -- start

# Downloads the CDN IP ranges into r3v3rs3/data/cdn-ranges.json.
cdn-snapshot:
	CARGO_INCREMENTAL=0 $(CARGO) test -p r3v3rs3-core --lib cdn::fetch::tests::update_embedded_snapshot -- --ignored --exact

clean:
	$(CARGO) clean
