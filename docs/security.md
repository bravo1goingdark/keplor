# Security Model

Operator-facing description of what Keplor's security posture actually
is today. Aspirational features are listed under
[Not implemented](#not-implemented). For deployment steps (TLS files,
systemd, backups), see [`operations.md`](operations.md).

## Threat model

Keplor is a server-to-server log-ingestion service. Three actors:

| Actor | Trust | What they can do |
|---|---|---|
| **Operator** | Trusted | Owns the host, the data directory, the TOML config, and any disk-level encryption. Has full read/write of stored events. |
| **API client** (gateway / SDK / app) | Untrusted at the API boundary | Authenticates with a bearer token. Can ingest, query, export, and delete events scoped to the configured key. |
| **Anonymous network attacker** | Untrusted | Cannot authenticate; sees only `/health` and `/metrics`. |

Keplor is **not** designed to defend a tenant from another tenant on
the same instance — there is no per-key authorization scope. Every key
that can read can read everything. Run separate instances if you need
hard isolation.

## Authentication

`Authorization: Bearer <secret>` on every request to a `/v1/*` route.
- Keys are configured under `[auth]` in `keplor.toml`. Two formats:
  - `api_keys = ["id:secret", "bare-secret"]` — simple list, default tier.
  - `[[auth.api_key_entries]]` — explicit `id`, `secret`, `tier` per key.
- Secret comparison uses `subtle::ConstantTimeEq` and **always scans
  every configured key**, so timing reveals neither key count nor the
  position of a match (`auth.rs::matched_key`).
- When `auth.api_keys` is empty *and* no `api_key_entries` are set, the
  server runs **open** — every endpoint accepts unauthenticated
  requests. Production deployments must configure at least one key.
- There is **no JWT**, no OAuth, no PKCE flow, no per-user identity.
  The bearer token *is* the identity. Per-user attribution comes from
  the `user_id` field that the client puts in each event payload.

### Hot reload via SIGHUP

Sending `SIGHUP` to the running process re-parses `keplor.toml`,
rebuilds `ApiKeySet`, and atomically swaps it through `arc_swap`. No
in-flight requests are dropped. **SIGHUP only reloads the API key
set** — TLS, listen address, rate limits, retention tiers, and CORS
origins all require a restart. (`server.rs::sighup_reload_loop`.)

## Transport

TLS termination is **in-process** when `[tls] cert_path` and `key_path`
are configured: rustls 1.3 is wired directly into the axum listener
(`server.rs`). With no `[tls]` section, the server speaks plain HTTP —
in that case TLS must be terminated at a reverse proxy (nginx, Caddy,
Cloudflare). The 10-second TLS handshake timeout protects the accept
loop from slow-loris connections.

Listen address defaults to `0.0.0.0:8080`; bind to `127.0.0.1` if you
want a reverse proxy in front.

## Rate limiting

When `[rate_limit] enabled = true`, every authenticated key gets an
independent token bucket (`requests_per_second`, `burst`). State is
sharded 16 ways across `Mutex<HashMap>` to keep contention bounded
under high concurrency (`rate_limit.rs::NUM_SHARDS`). Exhausted
buckets return `429` with a `Retry-After` header. Limits are
**in-process only** — multi-instance deployments share no state.

## Input validation

Hard-coded caps in `validate.rs`, applied before any storage write:

| Field | Cap |
|---|---|
| Request body | `pipeline.max_body_bytes` (default 10 MiB; ceiling 100 MiB) |
| Batch size | 10,000 events per request |
| Token counts (each) | 10,000,000 |
| `cost_nanodollars` | 1,000,000,000,000 (= USD 1,000) |
| `model` | 256 chars |
| `provider` | 128 chars |
| `user_id`, `api_key_id`, `org_id`, `project_id`, `route_id` | 256 chars each |
| `endpoint` | 512 chars |
| `metadata` (JSON) | 65,536 bytes |
| Timestamp | must be in `[2020-01-01, now+24h]` |

The HTTP status codes accepted in `http_status` are `u16`-bounded
only; the validator does not enforce a `[100, 599]` range.

## Storage at rest

Events live in a KeplorDB data directory (one append-only engine per
retention tier). **Keplor does not encrypt blobs at rest.** If the host
filesystem is compromised, every event is readable. Use OS-level disk
encryption (LUKS, dm-crypt, EBS encryption, GCP CMEK on the underlying
PD) when this matters. Backups inherit the same property — see
[`operations.md`](operations.md#backup-and-restore).

The active WAL is fsync'd at the end of each `BatchWriter` flush
(default 50 ms / 256 events), so a `kill -9` or power loss loses at
most one flush cycle of events on the fire-and-forget path.

## Audit logs

Two structured log lines are emitted under the `tracing` target
`audit`, both for `DELETE /v1/events`:

| Mode | Trigger | Fields |
|---|---|---|
| `older_than_days` | `?older_than_days=N` | `actor_key_id`, `older_than_days`, `events_deleted` |
| `user_id` | `?user_id=...` (GDPR erasure) | `actor_key_id`, `user_id`, `events_deleted` |

When the server is running open (no API keys configured), the
`actor_key_id` is recorded as `"anon"`. Single-event `DELETE
/v1/events/{id}` is **not** audit-logged today.

Auth failures, rate-limit rejections, and validation errors emit
plain `tracing::warn!` lines (no `audit` target). The `audit` target
is currently dedicated to bulk deletion only.

### Shipping audit logs

There is no built-in SIEM exporter. The deployment-recommended path:

1. Run with `--json-logs` so each line is a single JSON object.
2. Ship stderr to journald via systemd (the unit file in
   [`operations.md`](operations.md#systemd) does this by default).
3. From journald, forward to Loki / Splunk / CloudWatch / Datadog
   with the existing collector of choice.

Filter on `target=="audit"` at the collector to isolate sensitive
operations.

## Concurrency and back-pressure

`server.max_connections` (default 10,000) caps in-flight requests on
the authenticated routes only. `/health` and `/metrics` are not
subject to the limit, so observability remains reachable under
saturation. The per-request timeout (`server.request_timeout_secs`,
default 30 s) returns `408` for stalled requests. Both cap an
attacker's ability to hold connection slots.

## What server-side attribution does

`pipeline.rs::process_event` overwrites any client-provided
`api_key_id` with the authenticated key's ID before storage. A
malicious client cannot spoof attribution to another key.

It does **not** rewrite `user_id` — that field is whatever the client
sends. Treat `user_id` as advisory metadata, not as proof of the
calling user's identity.

## Not implemented

- **No IP allowlist / firewall** at the application layer. Use
  `iptables`, security groups, or a reverse proxy.
- **No mTLS / client cert auth.**
- **No per-key scopes** (read vs write vs delete). Every authenticated
  key has full access to every `/v1` endpoint.
- **No automatic key rotation.** Rotate by editing the TOML and
  sending `SIGHUP`; revoke by removing the key and `SIGHUP` again.
- **No CSRF protection.** The API is JSON-only with bearer tokens; no
  cookies, no form posts. Browser-origin requests must use the
  configured CORS allowlist.
- **No request signing** (HMAC body integrity, replay protection).
- **No envelope encryption** of stored blobs. Disk-level encryption
  only.
- **No tenant isolation.** Every key sees every event.

## Reporting issues

See `SECURITY.md` (root) when present, otherwise email the
maintainer listed in `Cargo.toml`.
