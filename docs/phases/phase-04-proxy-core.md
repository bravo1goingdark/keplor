# Phase 4 — Reverse proxy core (no providers yet)

**Status:** not started
**Depends on:** phases 1, 3
**Unlocks:** phases 5, 6

## Goal

A working HTTPS-terminating reverse proxy that forwards to a configured upstream URL, tees request and response bodies without buffering, and exposes the captured streams to a trait-based sink. No provider parsing yet.

## Prompt

Implement `keplor-proxy`.

### Crate structure

```
keplor-proxy/src/
  lib.rs
  server.rs         — axum app, TLS listener, graceful shutdown
  upstream.rs       — per-provider hyper::Client pool (rustls, h2+h1)
  tee.rs            — Body tee streams (request and response)
  capture.rs        — CaptureSink trait, bounded-mpsc plumbing
  route.rs          — RouteTable: match Host+path → Route { provider, upstream_url }
  headers.rs        — hop-by-hop stripping, auth forwarding, sanitization
  limits.rs         — per-connection and per-route limits
  config.rs         — ProxyConfig loaded via figment
```

### Key invariants (enforce in code + comments)

- Request body forwarded as a **stream**. Never `.collect()` into `Vec<u8>`.
- Tee = each `Bytes` frame cloned (refcount bump) into a `tokio::sync::mpsc::channel(CAP)`. If the capture task can't keep up and the channel fills, the tee **drops capture** (records `keplor_capture_dropped_total{stage}` metric) and **never drops the forwarded byte**.
- Response body streamed back to the client chunk-by-chunk. TTFT recorded on first response body frame. TTLT recorded on last frame / stream end.
- Only these request headers stripped before forwarding: `connection`, `keep-alive`, `proxy-authenticate`, `proxy-authorization`, `te`, `trailer`, `transfer-encoding`, `upgrade`, `host` (rewritten). Everything else — including the provider's auth header — forwarded verbatim.
- Response headers streamed back unchanged except hop-by-hop.

### CaptureSink trait

```rust
#[async_trait]
pub trait CaptureSink: Send + Sync {
    async fn on_request_start(&self, ctx: RequestCtx);
    async fn on_request_chunk(&self, id: EventId, chunk: Bytes);
    async fn on_request_end(&self, id: EventId);
    async fn on_response_status(&self, id: EventId, status: u16, headers: &HeaderMap);
    async fn on_response_chunk(&self, id: EventId, chunk: Bytes);
    async fn on_response_end(&self, id: EventId, outcome: StreamOutcome);
}
```

Provide a `NullSink` for tests and a `ChannelSink` that ships to a `tokio::sync::mpsc::UnboundedReceiver` for assembly tests.

### TLS

rustls 0.23 with aws-lc-rs provider. Load cert + key from config via `rustls-pemfile`. Optional: auto-generate a self-signed cert for dev mode using `rcgen` behind a `dev-tls` feature.

### Upstream client

Per-host `hyper::Client<HttpsConnector>` cached in an `ArcSwap<HashMap<Host, Client>>`. h2 enabled. Connection-idle-timeout 60s. `pool_max_idle_per_host = 32`.

### Graceful shutdown

SIGTERM → stop accepting new connections, drain in-flight with a 25-second deadline, call `CaptureSink::flush()`, exit.

### Metrics (`metrics` crate)

```
keplor_requests_total{route, method, status}
keplor_request_bytes_total{route}
keplor_response_bytes_total{route}
keplor_proxy_overhead_seconds_bucket     (histogram, measured as proxy
                                           processing time excluding upstream wait)
keplor_active_streams{route}
keplor_capture_dropped_total{stage}
```

## Acceptance criteria

- [ ] Integration test: spin up `wiremock` as the upstream, a `NullSink`, send a streaming NDJSON response. Assert:
  - client received the exact bytes wiremock sent
  - `ChannelSink` received the same bytes
  - proxy-overhead histogram bucket at p99 < 1 ms over 1000 requests
- [ ] Criterion benchmark: 16 KB non-streaming request/response round-trip. Report p50/p99 overhead. Target: p99 < 500 µs on a modern laptop.
- [ ] Working proxy binary stub: `cargo run -p keplor-proxy --example echo-proxy` forwards `https://localhost:8080/*` to a configurable upstream and logs captured bytes to stdout.
- [ ] `cargo test -p keplor-proxy` green
- [ ] `cargo clippy -p keplor-proxy -- -D warnings` green
