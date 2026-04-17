# Phase 6 — CLI, config, and the MVP binary

**Status:** not started
**Depends on:** phases 2, 3, 4, 5
**Unlocks:** `v0.1.0-alpha.1` release, then phases 7+

## Goal

Wire the MVP binary. `keplor run` should be usable for real traffic against OpenAI + Anthropic with SQLite storage and Prometheus metrics.

## Prompt

### 1. `keplor-cli/src/main.rs` — clap derive with subcommands

```
keplor run [--config PATH]
keplor check [--config PATH]               # validate config, don't start
keplor migrate [--db PATH]
keplor stats [--user U] [--key K] [--model M] [--last DURATION]
keplor query "SELECT ..."                   # raw SQLite read-only query
keplor export --format parquet|jsonl|csv --out PATH [--from T] [--to T]
keplor replay --event-id E                  # re-execute captured request
keplor keys create --user U --budget NANODOLLARS [--model-allow ...]
keplor keys list
keplor keys revoke ID
keplor db {vacuum|backup PATH|restore PATH}
keplor catalog {show|refresh}
keplor version
```

### 2. Config file (TOML, figment-loaded, env overrides with `KEPLOR_` prefix)

```toml
[server]
listen = "0.0.0.0:8443"
tls_cert = "/etc/keplor/cert.pem"
tls_key = "/etc/keplor/key.pem"
max_body_size_bytes = 16_777_216          # 16 MiB request cap

[store]
backend = "sqlite"                         # "sqlite" | "sqlite+s3" | "clickhouse"
sqlite_path = "/var/lib/keplor/keplor.db"
retain_bodies_days = 7
retain_events_days = 365
compress_level = 3

[observability]
prometheus_listen = "127.0.0.1:9090"
otlp_endpoint = ""                         # empty = disabled
log_level = "info"

[[routes]]
name = "openai"
match = { host = "api.openai.com" }
upstream = "https://api.openai.com"
provider = "openai"

[[routes]]
name = "anthropic"
match = { host = "api.anthropic.com" }
upstream = "https://api.anthropic.com"
provider = "anthropic"

# base-url override pattern (client points to keplor with a provider prefix):
[[routes]]
name = "openai-baseurl"
match = { path = "^/openai/" }
upstream = "https://api.openai.com"
path_strip_prefix = "/openai"
provider = "openai"
```

### 3. Startup sequence

1. Load config, validate.
2. Open `Store`, migrate.
3. Load pricing `Catalog` (bundled + optional override path).
4. Build `RouteTable`.
5. Register providers (OpenAI, Anthropic).
6. Construct `AggregateCaptureSink` that wires reassembler output → cost compute → `Store::append_event`.
7. Start Prometheus `/metrics` server.
8. Start OTLP exporter if configured.
9. Start proxy server.
10. Install SIGTERM/SIGINT handlers for graceful shutdown.

### 4. `keplor stats` output (plain text table)

```
User: alice
Range: 2026-04-10 to 2026-04-17
┌────────────────────────┬──────┬─────────┬──────────┬──────────┐
│ Model                  │ Reqs │ In tok  │ Out tok  │ Cost ($) │
├────────────────────────┼──────┼─────────┼──────────┼──────────┤
│ gpt-4o-2024-08-06      │ 1234 │ 456,789 │  123,456 │  12.3456 │
│ claude-opus-4-6        │   56 │  78,901 │   23,456 │   5.6789 │
└────────────────────────┴──────┴─────────┴──────────┴──────────┘
Total: 1,290 reqs — $18.0245
```

### 5. Docker image

- Multi-stage Dockerfile
- Build stage: `rust:1.XX-alpine` with musl-dev
- Runtime: `gcr.io/distroless/static-debian12:nonroot`
- Non-root UID 65532
- Final image < 15 MB

### 6. systemd unit

`Type=notify` (use `sd-notify`), `Restart=on-failure`, `ProtectSystem=strict`, `NoNewPrivileges=yes`, `LimitNOFILE=1048576`.

### 7. End-to-end smoke test (`tests/e2e/mvp.rs`)

- Start keplor with test config
- Fire 10 OpenAI requests (streaming + non-streaming, tools + no tools) against a wiremock upstream
- Fire 10 Anthropic requests (same matrix)
- Assert: all 20 events in Store, costs match expected, `keplor stats` output matches golden
- Shutdown cleanly within 25 s

## Acceptance criteria

- [ ] Users can run `keplor run --config keplor.toml`, point their OpenAI SDK at `https://localhost:8443` (with `base_url` override or DNS), and see accurate cost + usage in `keplor stats` and on Prometheus
- [ ] End-to-end smoke test green
- [ ] Docker image built and tagged
- [ ] Binary size < 12 MB static musl stripped
- [ ] Tag `v0.1.0-alpha.1`
- [ ] `docs/progress.md` updated with MVP milestone retrospective
