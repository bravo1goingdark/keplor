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

## 2026-04-17 — Phase 1 complete

### What was built

`keplor-core` filled in, 10 modules:

| Module         | Public items                                                                             |
|----------------|------------------------------------------------------------------------------------------|
| `id`           | `EventId(Ulid)`, `UserId`, `ApiKeyId`, `OrgId`, `ProjectId`, `RouteId`, `ProviderId`     |
| `provider`     | `Provider` enum + `canonical_host`, `id_key`, `auth_header_name`, `from_host_path`       |
| `usage`        | `Usage` + `merge`, `total_billable_input_tokens`, `total_output_tokens`                  |
| `cost`         | `Cost(i64)` nanodollars + `Display`, `Add`/`Sub`/`AddAssign`/`SubAssign`/`Neg`/`Sum`     |
| `error`        | `CoreError`, `ProviderError` + `from_provider_response`                                  |
| `payload_ref`  | `PayloadRef`, `PayloadStorage`, `Compression`, `BlobId`, `DictId`                        |
| `flags`        | `EventFlags` bitflags                                                                    |
| `sanitize`     | `sanitize_headers` (whitelist + hard denylist)                                           |
| `event`        | `LlmEvent`, `Latencies`, `TraceId` (32-hex-char serde)                                   |
| `lib`          | Flat re-exports; `#![deny(missing_docs)]`                                                |

### Acceptance

- `cargo test -p keplor-core --locked` → **68 passed**, 0 failed.
- `cargo clippy -p keplor-core --all-targets --locked -- -D warnings` → green.
- `cargo fmt --all -- --check` → green.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` → green (stub crates untouched).
- No other crate modified.

### Test coverage by required area

| Required                                     | Tests |
|----------------------------------------------|-------|
| `Usage::merge` (saturating, delta-accum)     | 3     |
| `Usage::total_billable_input_tokens`         | 4 (OpenAI/Azure, Anthropic/Bedrock, Gemini+Vertex, Ollama + 5 others) |
| `ProviderError::from_provider_response`      | 11 (OpenAI 429/400/context/filter, Anthropic nested, overloaded, Bedrock `__type`, Gemini `status`, Cohere text, Ollama, non-JSON, UTF-8 truncation) |
| `Provider::from_host_path` battery           | 1 table-test × 17 hosts                                              |
| `sanitize_headers` battery                   | 5 (strips auth/keys/cookies/SigV4, preserves whitelist, rejects unknown, multi-value preserved, denylist self-check) |

Plus rail tests for `Cost` display / arithmetic / saturation (10), `TraceId`
round-trip (5), `EventFlags` (4), `PayloadRef` (4), ID round-trips (6),
`Latencies` (2), `LlmEvent` clone smoke (1).

### Deviations / notes

- **Promoted `serde_json` from dev-deps to runtime deps** of `keplor-core`
  for `ProviderError::from_provider_response`'s best-effort JSON error
  parser. The phase spec implies this without saying it explicitly. The
  normaliser is pure logic — no I/O — and fits the "anchor of the
  dependency graph" constraint.
- **`LlmEvent` is not serde-derived.** `http::Method` has no stable serde
  impl; adding one would couple the wire format to an upstream crate
  version. Phase 3 (storage) adds a dedicated `StoredEvent` wire type
  that maps method → `SmolStr`.
- **Four deps kept crate-local** (not promoted to `[workspace.dependencies]`,
  per CLAUDE.md rule): `http`, `smol_str`, `bitflags`, `url`, `hex`. They
  will move to workspace-level in a future phase once a second crate
  uses them — at which point they need the user's sign-off.
- **`ProviderId` interpretation.** The phase spec lists it as an ID type,
  which is ambiguous given the existing `Provider` enum. Implemented as a
  stable `SmolStr` storage key (`"openai"`, `"anthropic"`, …) so
  historical events keep a stable join key even if new variants land in
  the `Provider` enum.
- **`cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))`** at
  the crate root — the workspace lints ban `.unwrap()` in runtime code,
  but tests are allowed to panic freely. Applied via `cfg_attr` so it
  does not affect the production build.
- **`#![deny(missing_docs)]`** at crate root enforces every public item
  has rustdoc.

### Deferred to later phases

- The `sanitize_headers` whitelist is a safe starter set; provider-specific
  response headers (full OpenAI `openai-*` response series, full Groq
  `x-groq-*`, Azure `x-ms-*`) will be extended in phase 5 alongside the
  provider adapters that surface them.
- `ProviderError::from_provider_response` has a best-effort `context_limit`
  extractor but doesn't yet try every provider's quirky limit-reporting
  path — good enough for phase 2's cost tests, extended per-provider in
  phase 5/7.

---

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

