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
    cargo install --locked cargo-nextest || true
    cargo install --locked cargo-deny    || true
    cargo install --locked cargo-bloat   || true
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
