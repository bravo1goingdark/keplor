# Keplor developer tasks.  Run `just` (no args) for the list.
#
# Install just: `cargo install --locked just`  or  `brew install just`.

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

_default:
    @just --list

# ---- Bootstrap --------------------------------------------------------------

# Install rustup components and cargo tools the project expects.
bootstrap:
    rustup component add rustfmt clippy
    rustup target add x86_64-unknown-linux-musl
    @echo "Installing cargo tools (skip with --skip-install if already present)..."
    cargo install --locked cargo-nextest    || true
    cargo install --locked cargo-deny      || true
    cargo install --locked cargo-bloat     || true
    cargo install --locked cargo-flamegraph || true
    @rustc --version
    @cargo --version

# ---- Day-to-day -------------------------------------------------------------

# Format the workspace.
fmt:
    cargo fmt --all

# Fail if formatting would change anything.
fmt-check:
    cargo fmt --all -- --check

# Clippy at -D warnings across all targets.
lint:
    cargo clippy --workspace --all-targets --locked -- -D warnings

# Cheapest check that the workspace type-checks.
check:
    cargo check --workspace --all-targets --locked

# Run unit + integration tests with nextest if present, cargo test otherwise.
test:
    @if command -v cargo-nextest >/dev/null 2>&1; then \
        cargo nextest run --workspace --locked ; \
    else \
        cargo test --workspace --locked ; \
    fi

# Run the phase-0 acceptance gate locally.
ci: fmt-check lint check test deny

# Supply-chain + licence audit.
deny:
    cargo deny check

# ---- Benchmarks -------------------------------------------------------------

# Run all Criterion benchmarks across the workspace.
bench:
    cargo bench --workspace

# Run benchmarks and save a named baseline for later comparison.
bench-baseline NAME="main":
    cargo bench --workspace -- --save-baseline {{NAME}}

# Compare current benchmarks against a saved baseline.
bench-compare BASELINE="main":
    cargo bench --workspace -- --baseline {{BASELINE}}

# ---- Flamegraphs (requires `perf` + kernel.perf_event_paranoid <= 1) --------

# CPU flamegraph from a Criterion benchmark.
flamegraph-bench BENCH CRATE="keplor-store":
    cargo flamegraph --bench {{BENCH}} -p {{CRATE}} -- --bench

# CPU flamegraph of running server under load.
flamegraph-server DURATION="30":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --profile bench -p keplor-cli
    ./target/bench/keplor run --config tests/fixtures/bench-config.toml &
    SERVER_PID=$!; trap "kill $SERVER_PID 2>/dev/null" EXIT
    for i in $(seq 1 30); do curl -sf http://127.0.0.1:8080/health >/dev/null && break; sleep 0.1; done
    cargo flamegraph --pid $SERVER_PID -o flamegraph-server.svg &
    FLAME_PID=$!
    hey -z "{{DURATION}}s" -c 50 -m POST -H "Content-Type: application/json" \
        -D tests/fixtures/load/single-event.json http://127.0.0.1:8080/v1/events || true
    kill $FLAME_PID 2>/dev/null; wait $FLAME_PID 2>/dev/null || true
    kill $SERVER_PID; wait $SERVER_PID 2>/dev/null || true
    echo "=> flamegraph-server.svg"

# ---- Heap profiling ---------------------------------------------------------

# dhat allocation profiling on batch writer path.
heap-dhat:
    cargo bench --bench dhat_batch -p keplor-store

# heaptrack on server under load (requires heaptrack installed).
heap-track DURATION="10":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --profile bench -p keplor-cli
    heaptrack ./target/bench/keplor run --config tests/fixtures/bench-config.toml &
    sleep 2
    hey -z "{{DURATION}}s" -c 10 -m POST -H "Content-Type: application/json" \
        -D tests/fixtures/load/single-event.json http://127.0.0.1:8080/v1/events || true
    kill %1 2>/dev/null; wait 2>/dev/null || true
    echo "Open heaptrack.*.zst with heaptrack_gui"

# ---- mimalloc ---------------------------------------------------------------

# Build with mimalloc allocator (recommended for production).
build-mimalloc:
    cargo build --release -p keplor-cli --features mimalloc

# Build static musl binary with mimalloc.
build-musl-mimalloc:
    cargo build --release --locked --target x86_64-unknown-linux-musl -p keplor-cli --features mimalloc

# ---- Release artefacts ------------------------------------------------------

# Static musl build of the keplor binary.
build-musl:
    cargo build --release --locked --target x86_64-unknown-linux-musl -p keplor-cli

# Print the size of the static binary (phase-0 gate: 12 MB, phase-11: 10 MB).
size: build-musl
    @ls -lh target/x86_64-unknown-linux-musl/release/keplor | awk '{print $5, $9}'
    @size=$(stat -c '%s' target/x86_64-unknown-linux-musl/release/keplor); \
      echo "raw bytes: $size"; \
      if [ "$size" -gt $((12 * 1024 * 1024)) ]; then \
        echo "FAIL: binary exceeds 12 MB gate"; exit 1; \
      fi

# Show what's taking space in the release binary.
bloat:
    cargo bloat --release --crates -p keplor-cli --target x86_64-unknown-linux-musl

# ---- Load testing (requires `hey`) ------------------------------------------

# Run load test against the server (builds, starts, tests, stops).
load-test DURATION="30" CONNECTIONS="50":
    bash tests/load/run.sh {{DURATION}} {{CONNECTIONS}}

# Quick smoke test: 5 seconds, low concurrency.
load-test-smoke:
    just load-test 5 10

# Load test with mimalloc allocator (production config).
load-test-mimalloc DURATION="30" CONNECTIONS="50":
    bash tests/load/run.sh {{DURATION}} {{CONNECTIONS}} mimalloc
