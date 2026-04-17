# Phase 11 — Production hardening, benchmarks, release

**Status:** not started
**Depends on:** all prior phases
**Unlocks:** `v1.0.0`

## Goal

Harden, benchmark publicly, ship 1.0.

## Prompt

### 1. Observability of self

- Full `/metrics` surface audit against Prometheus best practices (naming, units, label cardinality).
- `tracing` spans for every stage (request receive, parse, forward, response receive, reassemble, persist) with consistent span names and attributes.
- Panic hook that logs the panic + exits clean (no aborts leaving SQLite corrupt).
- `pprof` on a debug-only admin endpoint for CPU/heap profiling.

### 2. Chaos test suite (`tests/chaos/`)

- `kill -9` mid-stream: verify SQLite WAL recovers, no partial event in table, `stream_incomplete=1` set on next startup scrub.
- Disk full: verify backpressure propagates, new requests 503, in-flight finish cleanly.
- Upstream times out: verify client gets a proper error, event logged with `error_type`.
- Clock skew (server monotonic resets): timestamps stay monotonic via ULID's monotonic factory.
- 5 GiB body upload: verify we never buffer; memory stays flat.
- Malformed SSE (random-byte fuzz): reassembler never panics, `stream_incomplete=1`.
- TLS handshake storm: 5k concurrent TLS opens, verify rustls holds up.

### 3. Public benchmark rig (`bench/`)

Docker-compose with keplor + a mock-provider + `wrk2`/`vegeta`.

Scenarios:
- **(a)** non-streaming chat @ 10k req/s — report p50/p99 overhead
- **(b)** streaming chat @ 2k concurrent streams — report per-chunk overhead, TTFT overhead
- **(c)** mixed workload @ 5k req/s sustained 10 minutes — report memory, CPU, compression ratio

Comparison runs against LiteLLM Gateway, Helicone self-host, Portkey Gateway (where possible). Publish results in `BENCHMARKS.md` with methodology + reproducer scripts.

### 4. Fuzz targets (`fuzz/`)

Targets:
- OpenAI SSE reassembler
- Anthropic reassembler
- Gemini progressive JSON decoder
- AWS event-stream decoder
- Config parser

Run in CI for 5 min per PR; overnight for 8h on main.

### 5. Security

- Threat model doc (`docs/threat-model.md`) covering: proxy as MITM, virtual-key theft, body-capture of secrets, log injection, SSRF via arbitrary upstream override.
- Dependency audit via `cargo-deny` + `cargo-audit`, wired into CI.
- Third-party code review checklist doc.
- `SECURITY.md` with reporting process.

### 6. Docs site

- `mdbook` in `docs/` with:
  - getting started
  - config reference
  - provider matrix
  - OTel integration
  - admin UI
  - virtual keys
  - deployment (Docker, systemd, Helm chart)
  - migration from Helicone / LiteLLM / Langfuse
  - FAQ
  - architecture deep-dive
- Helm chart in `deploy/helm/keplor/` with ServiceMonitor for Prometheus.
- Homebrew formula.
- Debian/Ubuntu `.deb` via `cargo-deb`.
- RPM via `cargo-generate-rpm`.

### 7. Release automation

- `cargo-release` for version bumps.
- GitHub Actions release workflow: tag → build musl + darwin (x86_64 + aarch64) + windows (optional) → publish to crates.io + ghcr.io + GitHub Releases with SBOM (`syft`) + sigstore signatures.
- Changelog follows Keep-a-Changelog.

### 8. Launch checklist

- `README.md` polished with quickstart (`docker run keplor/keplor` → proxy at `:8443` → `curl stats` in 60 seconds).
- `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, PR template, issue templates.
- Public benchmarks page.
- Blog post draft (`docs/launch-post.md`): positioning vs. Helicone, LiteLLM, Langfuse; the "pure-observational, single-binary" pitch.

## Acceptance criteria

- [ ] Tag `v1.0.0`
- [ ] Binary size audit: CI fails if > 10 MB static musl
- [ ] Memory at 5k rps sustained: CI fails if > 150 MB RSS
- [ ] All chaos tests green
- [ ] All fuzz targets run clean for ≥ 8 h on main
- [ ] `docs/` site builds and deploys
- [ ] Helm chart installs cleanly against a kind cluster
- [ ] Release artifacts signed with sigstore
- [ ] SBOM attached to release
- [ ] `docs/progress.md` — final retrospective for the 1.0 milestone
