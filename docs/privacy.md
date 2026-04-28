# Data Handling and PII

Description of what Keplor stores per ingested event, how long it
keeps it, and how to erase it. Covers what's actually in the code
today; aspirational features are explicitly absent or labelled.

For the security model (auth, transport, audit logs), see
[`security.md`](security.md). For storage/backup mechanics, see
[`operations.md`](operations.md).

## What we store per event

Keplor's canonical record is `LlmEvent` (`keplor-core/src/event.rs`).
Every accepted event populates these fields:

### Identifiers / attribution

| Field | Type | Source | PII relevant |
|---|---|---|---|
| `id` | ULID | server-generated | No |
| `ts_ns`, `ingested_at` | i64 (epoch ns) | client / server clock | No |
| `user_id` | string (≤256 chars) | client payload | **Yes** — caller-defined |
| `api_key_id` | string | server overrides client value with authenticated key id | Pseudonym |
| `org_id`, `project_id`, `route_id` | string | client payload | Indirect |
| `tier` | string | derived from authenticated key | No |
| `source` | string | client (e.g. `"obol"`, `"litellm"`) | No |

### Request shape

| Field | Type | Notes |
|---|---|---|
| `provider` | enum | normalised |
| `model`, `model_family` | string | normalised lowercase |
| `endpoint`, `method` | string | path + HTTP verb |
| `http_status` | u16 | upstream response status |
| `flags` | bitflags | `streaming`, `tool_calls`, `reasoning`, `stream_incomplete`, `cache_used` |
| `error.kind`, `error.message` | string | upstream error if any — message can contain free text |
| `usage.*` | u32 counters | input / output / cache / reasoning / audio / image / tool tokens |
| `cost_nanodollars` | i64 | computed from catalog or client-supplied |
| `latency.{ttft,total,time_to_close}_ms` | u32 | timing |

### Observability fields (only stored if the client passes them)

| Field | Source | PII relevant |
|---|---|---|
| `client_ip` | client payload only | **Yes** — IP address |
| `user_agent` | client payload only | Possible fingerprinting |
| `request_id` | client payload (provider-returned id) | Pseudonym |
| `trace_id` | client payload (W3C trace context) | No |
| `metadata` | client payload (free-form JSON, ≤64 KiB) | **Yes if you put PII in it** |

Keplor never extracts `client_ip` from the TCP socket and never reads
`X-Forwarded-For`. If your gateway doesn't put an IP in the payload,
none is stored. (`pipeline.rs::build_llm_event`.)

### Fields that look like they're stored but aren't

- **`request_body` and `response_body`** — the wire schema
  (`schema.rs`) accepts these as raw JSON, but the pipeline does
  **not** persist them. They're deserialised, then dropped on the
  floor in `build_llm_event`. The `LlmEvent` type has no field for
  them. The README and `integration.md:395-396` claim "stored
  compressed" — that's stale documentation; the body-storage
  subsystem was removed. Do not rely on bodies being recoverable
  from Keplor.
- **`request_sha256` / `response_sha256`** — vestigial columns on
  `LlmEvent`, always written as 32 zero bytes. They survive only to
  preserve column indexes for old SQLite migrations.

### What we never capture automatically

- No HTTP-level IP capture, no `X-Forwarded-For` parsing.
- No browser cookies (the API is bearer-token only, no cookies).
- No fingerprinting beyond the optional `user_agent` field clients
  may pass.
- No request/response body indexing or content extraction.

## Retention

Retention is configured per-tier under `[retention.tiers]` in
`keplor.toml`. The default config (`free=7d`, `pro=90d`) is purely
illustrative — operators set whatever durations match their policy.
A tier with `days = 0` is "keep forever".

Garbage collection runs every `storage.gc_interval_secs` (default
3600 s = 1 h). One pass per tier; KeplorDB's GC is **segment-granular**
— a segment is dropped only when *all* events inside it are older
than the cutoff. This means actual deletion lags the configured
boundary by up to one segment's age range.

For optional R2 / S3 archival lifecycle, see
[`operations.md`](operations.md) and the `[archive]` block in
`keplor.example.toml`.

## Deletion

Three deletion paths:

1. `DELETE /v1/events/{id}` — single event, tombstoned.
2. `DELETE /v1/events?older_than_days=N` — bulk by age. Calls
   `gc_expired(now - N*86400e9)` and audit-logs.
3. `DELETE /v1/events?user_id=alice` — **GDPR right-to-erasure**.
   Loops over the user's events in batches of 1,000, tombstones
   each batch, audit-logs the total. Both modes write to the
   `audit` tracing target (`routes.rs::delete_events_bulk`).

### Tombstones vs reclamation

`delete_event` and `delete_events_by_ids` mark KeplorDB tombstones.
Tombstones make events invisible to queries immediately. The
**physical bytes on disk are reclaimed only when segment GC drops
the whole segment** containing the tombstoned ids. For GDPR
compliance, the deletion is *logical* on the API and *physical* by
the next GC sweep (and archive expiry, if applicable).

### Visibility caveat

The `?user_id=...` path uses the standard read query, which only
sees events that have rotated from the active WAL into a segment.
Events ingested within the last `BatchWriter` flush cycle (≤ ~50 ms
by default, before `wal_checkpoint`) may not be visible to a
just-issued erasure call. If exact zero-event-remaining is
required, run a second call after that interval, or run
`keplor gc` with the same cutoff.

### Archived data

When R2/S3 archival is enabled, archived events are **not** removed
by `?user_id=...` — the GDPR loop only traverses live KeplorDB
engines. Archived JSONL files remain on object storage until
explicitly deleted by lifecycle rules or manual cleanup. If you
enable archival and you are subject to GDPR, you must implement an
out-of-band erasure on the bucket (e.g. server-side filtering on
read, or a periodic re-archive job that drops erased rows). This
gap is on the roadmap; until then, retention via bucket lifecycle
is the recommended mitigation.

## Operator obligations (data controller vs processor)

Keplor is software, not a service. The legal role you play depends
on your deployment:

- **You operate Keplor for your own product**: you are the
  *controller* for your end-users' data. Keplor authors are not in
  the data path. Your privacy notice, DPIA, and erasure SLA are
  yours.
- **You operate Keplor on behalf of a customer**: you are a
  *processor* under that customer's instructions. Their DPA governs
  your handling.

Either way, the operator is responsible for:

- Configuring retention to match policy and law.
- Exposing erasure (the `?user_id=...` endpoint) to the controller.
- Restricting who holds API keys and rotating them on personnel
  change.
- Encrypting the disk hosting the data directory if local laws
  treat unencrypted-at-rest as a breach risk.
- Maintaining an audit-log destination and retention period
  consistent with their accountability obligations
  (see [`security.md`](security.md#audit-logs)).

## Sub-processors

Keplor itself contacts no external service in its default
configuration. The only optional outbound integration is **R2 / S3
archival** (`[archive]` block, requires `--features s3`). When
enabled, the configured object store becomes a sub-processor:

- Cloudflare R2 (typical config) — events stored as zstd-compressed
  JSONL under your bucket prefix, partitioned by `(user_id, day)`.
- Any S3-compatible target works (AWS S3, MinIO, Backblaze B2). The
  operator chooses.

Document the chosen object-storage provider as a sub-processor in
your privacy notice and DPA. Lifecycle rules on the bucket are the
operator's responsibility.

## What we explicitly don't do

- **No PII redaction** of `metadata`, `error.message`, or other
  free-text. SSNs or emails put into metadata are stored verbatim.
- **No hashing of identifiers** — `user_id` is stored as sent.
- **No analytics, telemetry, or callbacks** — no code path phones home.
- **No cookies / client-side state** — pure JSON-over-HTTP with
  bearer auth.

## Right-of-access export

`GET /v1/events?user_id=alice` returns up to 1,000 events as JSON
per call (cursor pagination). `GET /v1/events/export?user_id=alice`
streams *all* matching events as NDJSON. Combined they satisfy
GDPR Article 15 export for a given user.

## Roadmap (not implemented)

- Erasure that propagates into archived JSONL.
- PII auto-redaction hooks on the metadata field.
- Per-record encryption with operator-supplied keys.
- Single-event `DELETE` audit log entry.
