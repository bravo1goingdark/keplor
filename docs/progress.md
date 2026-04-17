# Progress log

This file is append-only. Claude writes a retrospective at the end of every phase. Read this file at the start of every session to know where we are.

Format for each entry:

```
## YYYY-MM-DD — Phase N complete

- What was built (modules, crates, tests).
- Test count and pass rate.
- Binary size (static musl, stripped).
- Compression ratio observed (from phase 3 onward).
- Anything deferred to a later phase.
- Any deviations from the phase prompt, with reasoning.
```

---

<!-- Append entries below this line. Most recent on top. -->

## 2026-04-17 — Phase 0 complete

### What was built
- Cargo workspace (`resolver = "2"`) with 7 library crates + 1 binary + `xtask/`:
  `keplor-core`, `keplor-providers`, `keplor-proxy`, `keplor-store`,
  `keplor-pricing`, `keplor-telemetry`, `keplor-cli` (binary `keplor`),
  and `xtask` for project automation.
- Release profile tuned for size: `opt-level = "z"`, `lto = "fat"`,
  `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"`.
- Workspace-level dependency pinning for every crate named in the
  phase-0 spec (tokio 1, hyper 1, axum 0.8, rustls 0.23 + aws-lc-rs,
  hyper-rustls 0.27, reqwest 0.12 rustls-tls, rusqlite 0.32 bundled,
  zstd 0.13, sonic-rs 0.5, opentelemetry 0.27 + otlp http-proto, etc.).
- Workspace-level lints: `unsafe_code = "deny"`,
  `clippy::unwrap_used`/`expect_used = "warn"`.
- Toolchain pinned to `1.93.0` via `rust-toolchain.toml`; musl target
  pre-fetched; `rustfmt`, `clippy` components required.
- `.cargo/config.toml` pins `crt-static` for both musl targets and adds
  convenience aliases (`cargo xtask`, `cargo ci-check`, `cargo ci-clippy`).
- `justfile` with `bootstrap`, `fmt`, `fmt-check`, `lint`, `check`, `test`,
  `ci`, `deny`, `build-musl`, `size`, `bloat` recipes.
- GitHub Actions CI (`.github/workflows/ci.yml`): fmt → clippy+check →
  nextest → cargo-deny → musl build + 12 MB size gate, with artifact upload.
- `deny.toml` with licence allow-list (Apache-2.0 / MIT / BSD / ISC /
  MPL-2.0 / Unicode-3.0 / Zlib / CC0-1.0) and bans on `openssl`,
  `openssl-sys`, `async-std`, `rusoto_core`, and bare `ring`-under-`rustls`.
- `rustfmt.toml` and `clippy.toml` (MSRV 1.82, provider/product
  identifiers whitelisted for doc lint).

### Acceptance checks
- `cargo fmt --all -- --check` → **OK**
- `cargo check --workspace --all-targets --locked` → **OK** (0.01 s hot)
- `cargo clippy --workspace --all-targets --locked -- -D warnings` → **OK**
- `cargo test --workspace --locked --no-run` → **OK** (8 empty binaries linked)
- `cargo build --release --locked --target x86_64-unknown-linux-musl -p keplor-cli` → **OK**

### Binary size (baseline)
- `target/x86_64-unknown-linux-musl/release/keplor`: **381 464 bytes (373 KB)**,
  static-pie linked, stripped.
- Phase-0 gate (12 MB): **PASS** with 32× headroom.
- This is a stub that prints a single line; it's a floor, not a ceiling.
  Real growth starts with phase 2 (pricing catalogue) and phase 4 (proxy
  + rustls + reqwest). Phase-11 tightens the gate to 10 MB.

### Deferred
- Workspace lints kept conservative (no `pedantic` / `nursery`) — revisit
  once there's real code to vet; opening them now would only flag stubs.
- `cargo-deny check` not yet run locally (no `cargo-deny` binary installed
  on dev machine); CI will run it on first push. `just bootstrap`
  installs it.
- Nightly `-Z build-std` size-tuned build is documented in
  `docs/architecture.md` but not wired into CI — defer to phase 11.

### Deviations from the phase prompt
- Added a `[workspace.lints]` block (not in the prompt) so
  `[lints] workspace = true` in each member crate has something to
  inherit. Rules chosen match CLAUDE.md's code-quality bar.
- Toolchain pinned to `1.93.0` (current stable on this machine) rather
  than the more generic `"stable"`; prompt said "pinned" — picking an
  exact version is the strictest reading.
- `justfile` chosen over `make bootstrap` (prompt allowed either).

