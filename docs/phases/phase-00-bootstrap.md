# Phase 0 — Workspace bootstrap

**Status:** not started
**Depends on:** nothing
**Unlocks:** all subsequent phases

## Goal

Stand up the Cargo workspace, release profile, CI, and the crate skeleton so every later phase has a known-good starting point.

## Prompt

Create the Cargo workspace for Keplor. Deliverables:

1. Workspace layout:
   ```
   keplor/
     Cargo.toml                  (workspace root, resolver = "2")
     rust-toolchain.toml         (channel = "stable", pinned)
     .cargo/config.toml          (release profile tuned for size)
     rustfmt.toml, clippy.toml
     deny.toml                   (cargo-deny config)
     .gitignore, LICENSE (Apache-2.0), README.md (skeleton)
     crates/
       keplor-core/              types: Event, Provider, Usage, Cost, errors
       keplor-providers/         provider adapters (one module per provider)
       keplor-proxy/             hyper + axum reverse proxy, body tee
       keplor-store/             SQLite + payload-blob store, zstd compression
       keplor-pricing/           LiteLLM catalog loader, cost computation
       keplor-telemetry/         OTel GenAI + OpenInference dual emission
       keplor-cli/               the binary: `keplor` with subcommands
     xtask/                      custom build tasks (catalog refresh, size audit)
   ```

2. Release profile in workspace `Cargo.toml`:
   ```
   opt-level = "z", lto = "fat", codegen-units = 1, panic = "abort",
   strip = "symbols", debug = false
   ```

3. Pinned dependency versions (`workspace.dependencies` table):
   ```
   tokio = "1", hyper = "1", axum = "0.8", tower = "0.5",
   rustls = "0.23" with aws-lc-rs provider, hyper-rustls = "0.27",
   reqwest = "0.12" default-features = false, features = ["rustls-tls","stream","json"],
   bytes = "1", http-body-util = "0.1", eventsource-stream = "0.2",
   rusqlite = "0.32" features = ["bundled"],
   zstd = "0.13", serde = "1", serde_json = "1", sonic-rs = "0.5",
   tracing = "0.1", tracing-subscriber = "0.3", metrics = "0.24",
   metrics-exporter-prometheus = "0.16", opentelemetry = "0.27",
   opentelemetry-otlp = "0.27" features = ["http-proto","reqwest-client"],
   clap = "4.5" features = ["derive","env"], figment = "0.10" features = ["toml","env"],
   thiserror = "2", anyhow = "1", ulid = "1", sha2 = "0.10",
   aws-smithy-eventstream = "0.60", tiktoken-rs = "0.6", tokenizers = "0.20",
   secrecy = "0.10", zeroize = "1",
   [dev-dependencies] wiremock = "0.6", insta = "1", criterion = "0.5"
   ```

4. GitHub Actions CI (`.github/workflows/ci.yml`):
   - fmt check
   - clippy `-D warnings`
   - test
   - cargo-deny check (licenses, advisories)
   - musl static build + binary-size gate (fail if `keplor` binary > 12 MB — we'll tighten to 10 MB at phase 11)

5. A `make bootstrap` target (or `justfile`) that installs rustup components, cargo-deny, cargo-nextest, cargo-bloat, and verifies toolchain.

6. `README.md` already exists — leave it.

## Acceptance criteria

- [ ] `cargo check --workspace` green
- [ ] `cargo fmt --check` green
- [ ] `cargo clippy -- -D warnings` green (crates are empty stubs with doc comments; lints should not fire)
- [ ] CI workflow file exists and would pass on push (not yet run against GitHub)
- [ ] Binary-size sanity check documented in `docs/progress.md`

Before writing files, show me the full directory tree you plan to create. After files are written, run `cargo check --workspace` and paste the output.
