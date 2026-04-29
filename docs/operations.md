# Operations Guide

## Deployment

### Docker (recommended)

```bash
# Copy and edit the example config
cp keplor.example.toml keplor.toml

# Build and start
docker compose up -d

# Verify
curl http://localhost:8080/health
```

### Binary

```bash
# Production build with mimalloc (recommended — +49% throughput over system malloc)
cargo build --release --target x86_64-unknown-linux-musl -p keplor-cli --features mimalloc

# Or without mimalloc (uses system allocator)
cargo build --release --target x86_64-unknown-linux-musl -p keplor-cli

./target/x86_64-unknown-linux-musl/release/keplor run --config keplor.toml
```

### Systemd

```ini
[Unit]
Description=Keplor LLM Log Ingestion Server
After=network.target

[Service]
Type=simple
User=keplor
ExecStart=/usr/local/bin/keplor run --config /etc/keplor/keplor.toml --json-logs
Restart=on-failure
RestartSec=5
LimitNOFILE=131072
MemoryMax=1G

[Install]
WantedBy=multi-user.target
```

## Pre-deploy Production Checklist

Run through this before every production deployment.

### Build

- [ ] Build with mimalloc: `--features mimalloc` (49% throughput gain)
- [ ] Build as static musl binary: `--target x86_64-unknown-linux-musl`
- [ ] Verify binary size: `ls -lh target/x86_64-unknown-linux-musl/release/keplor` (must be <10 MB)
- [ ] Run the acceptance gate: `just ci` (fmt, clippy, tests, supply-chain audit)

### Configuration

- [ ] Set API keys — **never deploy with empty `auth.api_keys`** (open access)
- [ ] Set `storage.retention_days` (default 90 — `0` disables GC, DB grows unbounded)
- [ ] Set `storage.max_db_size_mb` to prevent disk exhaustion (returns 507 when exceeded)
- [ ] Review `pipeline.batch_size` (default 64 — increase to 256 for high-throughput deployments)
- [ ] Review `pipeline.max_body_bytes` (default 10 MB — lower if you don't store bodies)
- [ ] Set `server.max_connections` appropriately for your load balancer/proxy
- [ ] Enable TLS via `tls.cert_path` / `tls.key_path` or terminate at a reverse proxy

### System

- [ ] File descriptor limit: `LimitNOFILE=65535` in systemd (or `ulimit -n 65535`)
- [ ] Dedicated data directory with sufficient disk space for the KeplorDB segments + WAL shards
- [ ] Use `--json-logs` for structured log aggregation (Datadog, Loki, etc.)
- [ ] Set `KEPLOR_LOG=info` (or `RUST_LOG=info`) — avoid `debug`/`trace` in production

### Monitoring

- [ ] Scrape `/metrics` with Prometheus (see Prometheus metrics table below)
- [ ] Alert on `keplor_batch_flush_errors_total` increasing (DB write failures)
- [ ] Alert on `queue_utilization_pct > 80` in `/health` (back-pressure)
- [ ] Alert on `keplor_auth_failures_total` spikes (credential stuffing)
- [ ] Monitor disk usage — KeplorDB segments accumulate between GC runs

### Backup

- [ ] Schedule daily snapshots of the KeplorDB data dir (rsync / filesystem snapshot — see Backup section below)
- [ ] Test restore procedure at least once before go-live

### Smoke test

```bash
# Health
curl -sf http://localhost:8080/health | jq .

# Ingest a test event
curl -X POST http://localhost:8080/v1/events \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <your-api-key>" \
  -d '{"model":"gpt-4o","provider":"openai","usage":{"input_tokens":10,"output_tokens":5}}'

# Verify it was stored
curl http://localhost:8080/v1/events?limit=1 \
  -H "Authorization: Bearer <your-api-key>" | jq .
```

## Configuration

See `keplor.example.toml` for all available options. Every field can be overridden via environment variables using the `KEPLOR_<SECTION>_<FIELD>` pattern:

```bash
KEPLOR_STORAGE_RETENTION_DAYS=30
KEPLOR_SERVER_LISTEN_ADDR=0.0.0.0:9090
KEPLOR_AUTH_API_KEYS='["prod:sk-abc123"]'
```

## Durability guarantees

The default ingest path (`POST /v1/events`, `POST /v1/events/batch`, both
synchronous *and* fire-and-forget) routes through `BatchWriter`, which
**fsyncs every flush cycle** — by default every 50 ms or 256 events,
whichever comes first. After the response to a `POST /v1/events` returns
(or after the flush following a fire-and-forget batch), the events are
durably on disk; a `kill -9` or power loss will not lose them.

The crash-loss window for a fire-and-forget batch (`POST /v1/events/batch`
without `X-Keplor-Durable: true`) is **at most one flush cycle** of the
events queued before the crash — typically <50 ms. For zero-window
durability, set `X-Keplor-Durable: true` on the batch request: the call
will wait for the next flush before returning.

The `wal_sync_interval` and `wal_sync_bytes` storage knobs only affect
direct (non-BatchWriter) writes, which the server itself never issues.
Do not lower them in production thinking it will improve durability of
the ingest path — it won't.

## Monitoring

### Health check

```bash
curl http://localhost:8080/health
# {"status":"ok","version":"0.1.0","db":"connected","queue_depth":0,"queue_capacity":32768,"queue_utilization_pct":0}
```

Returns `503 Service Unavailable` if the database is unreachable.

### Prometheus metrics

Scrape `GET /metrics` for:

| Metric | Type | Description |
|--------|------|-------------|
| `keplor_events_ingested_total` | counter | Events ingested (by provider) |
| `keplor_events_errors_total` | counter | Errors (by stage: validation, store, queue_full) |
| `keplor_ingest_duration_seconds` | histogram | End-to-end ingest latency |
| `keplor_auth_successes_total` | counter | Successful authentications |
| `keplor_auth_failures_total` | counter | Failed authentications |
| `keplor_batch_flushes_total` | counter | Batch flush operations |
| `keplor_batch_events_flushed_total` | counter | Events flushed to DB |
| `keplor_batch_flush_errors_total` | counter | Failed batch flushes |
| `keplor_storage_bytes{tier}` | gauge | Bytes on disk across this tier's segments. Sampled every 10 s. |
| `keplor_segments_total{tier}` | gauge | Closed segment-file count for this tier. |
| `keplor_wal_events{tier}` | gauge | Events buffered in the active WAL, not yet rotated. |
| `keplor_storage_events{tier}` | gauge | Total events across segments + WAL for this tier. |
| `keplor_pricing_catalog_refresh_total{result}` | counter | Pricing catalog refresh cycles. `result=ok` or `error`. |
| `keplor_pricing_catalog_age_seconds` | gauge | Seconds since the in-memory pricing catalog was last refreshed. Alert when this exceeds 2× `pricing.refresh_interval_secs`. |

### Alerting recommendations

- `keplor_batch_flush_errors_total` increasing: database write failures (check disk space)
- `queue_utilization_pct > 80` in health check: back-pressure, ingestion exceeding write throughput
- `keplor_auth_failures_total` spike: possible credential stuffing
- HTTP 422 spike from clients on `POST /v1/events`: the wire schema is `deny_unknown_fields`; a client started sending a field the server doesn't recognise. Check the client's payload against `IngestEvent`.

## Backup and Restore

Keplor stores everything in a single data directory (default
`./keplor_data` or `storage.data_dir` in the config). The directory
contains per-tier KeplorDB engines (sharded WALs + immutable `.kdb`
segments) and the `manifests.jsonl` archive sidecar.

### Backup

**Option 1: Filesystem snapshot (recommended)**

If your filesystem supports atomic snapshots (ZFS, Btrfs, LVM), snapshot
the data directory while Keplor runs. KeplorDB segments are immutable
once rotated, and active WAL shards are crash-safe (their headers and
records are fsync'd on the durable write paths), so a snapshot is a
valid restore source.

```bash
# ZFS example
zfs snapshot tank/keplor@daily-$(date +%Y%m%d)
zfs send tank/keplor@daily-$(date +%Y%m%d) | ssh backup-host "zfs recv ..."
```

**Option 2: Online tar via systemd timer (provided)**

The repo ships `deploy/keplor-backup.service` + `deploy/keplor-backup.timer`.
Install both alongside `keplor.service` and enable the timer:

```bash
sudo cp deploy/keplor-backup.service deploy/keplor-backup.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now keplor-backup.timer

# Verify next run
systemctl list-timers keplor-backup.timer
```

The timer fires daily at 02:30 UTC (with a 30 min random jitter), tar+zstd
compresses `/var/lib/keplor` to `/var/backups/keplor/`, retains 7 days, and
runs at low I/O priority so it doesn't disturb live ingest. The active WAL
shards are crash-safe (every BatchWriter flush fsyncs), so this snapshot
taken with keplor running is a valid restore source. RPO: 1 day.

**Option 3: Stop and copy**

```bash
# Graceful shutdown drains the BatchWriter and runs a final wal_checkpoint.
docker compose stop keplor

# Copy the data directory
tar -C /var/lib -czf /backups/keplor-$(date +%Y%m%d).tar.gz keplor

docker compose start keplor
```

**Option 3: Online rsync after a checkpoint**

```bash
# Force a WAL checkpoint (rotates active WAL into a sealed segment).
keplor rollup --data-dir /var/lib/keplor

# Rsync the (mostly immutable) data directory.
rsync -a --inplace /var/lib/keplor/ /backups/keplor-$(date +%Y%m%d)/
```

Concurrent writes during the rsync may leave the trailing WAL shards
inconsistent at the byte level; KeplorDB's recovery code tolerates a
truncated trailing record on reopen, so the restore will simply lose
events that hadn't yet rotated to a segment.

### Restore

```bash
docker compose stop keplor
rm -rf /var/lib/keplor
tar -C /var/lib -xzf /backups/keplor-20260418.tar.gz
docker compose start keplor
```

### Scheduled backups

```bash
# crontab: daily tarball at 03:00, retain 7 days
0 3 * * * tar -C /var/lib -czf /backups/keplor-$(date +\%Y\%m\%d).tar.gz keplor && find /backups -name 'keplor-*.tar.gz' -mtime +7 -delete
```

## Garbage Collection

Automatic GC runs hourly when `storage.gc_interval_secs > 0` (default: 3600). It runs one pass per configured retention tier, dropping segments whose events are entirely older than the tier's retention window.

Manual GC:

```bash
keplor gc --older-than-days 30 --data-dir /var/lib/keplor
```

## Load testing

`cargo xtask loadtest` drives sustained `POST /v1/events` traffic and
reports a percentile breakdown. Use it to establish a baseline before
shipping a perf-sensitive change.

```bash
# 5000 req/s for 30s against a localhost server; report p50/p95/p99.
cargo xtask loadtest \
  --rate 5000 \
  --duration 30s \
  --concurrency 64 \
  --target http://127.0.0.1:8080
```

Baseline gate (CI):

```bash
# First run: writes the baseline file if absent.
cargo xtask loadtest --rate 5000 --duration 30s --concurrency 64 \
  --target http://127.0.0.1:8080 \
  --baseline xtask/baselines/loadtest.json

# Subsequent runs: compare p99 against the saved baseline. The
# command exits non-zero (CI fails) if p99 is >20% slower than the
# saved value.
```

Achieved throughput is bounded by `concurrency × p99` for the durable
write path. To probe sustained capacity, raise `--concurrency`; for
back-pressure observation, exceed the steady-state rate and watch
`queue_depth_max` in the report.

## Upgrading

1. Back up the data directory (see above)
2. Build or pull the new binary/image
3. Verify the data directory: `keplor migrate --data-dir /var/lib/keplor` (idempotent — opens the store, refuses to mount a directory written under a mismatched `SCHEMA_ID`)
4. Restart the server

The server also opens the store on startup, which performs the same
schema-id check.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `503 Service Unavailable` on ingest | Batch writer queue full | Reduce ingestion rate or increase `pipeline.batch_size` |
| `408 Request Timeout` | Request exceeded `server.request_timeout_secs` | Increase timeout or reduce batch size |
| Data directory growing unbounded | `retention_days = 0` (GC disabled) | Set `retention_days` to a positive value |
| Many tiny segment files | Default `pipeline.flush_interval_ms = 50` produces ~1200 segments/min/tier at idle | Raise `pipeline.flush_interval_ms` (e.g. 250) to trade read freshness for fewer segments. See "Segment count blowing up" in the runbook below if GC can't keep up. |
| High memory usage | Large batch writer queue | Reduce `channel_capacity` in batch config |

## Incident runbook

Sketches for the incidents most likely to page someone. Keplor is
backed by a pre-1.0 KeplorDB engine — assume things will surprise you
and bias toward stopping the writer over speculative recovery.

### The store fails to open at startup

Symptom: `keplor run` exits immediately with `failed to open data dir`
or `schema_id mismatch`.

1. **`Permission denied`** — `chown -R keplor:keplor /var/lib/keplor`
   and re-check the systemd `User=` directive.
2. **`schema_id mismatch`** — somebody pointed `storage.data_dir` at a
   directory written by a different release. Either point at the
   correct dir or migrate. Never delete the directory to "fix" it.
3. **`UnexpectedEof` on a WAL shard** — KeplorDB recovery tolerates a
   truncated trailing record, so this is rare; if it persists, the
   shard file was corrupted. Move the offending `wal-N` file aside,
   restart, and accept the loss of events that hadn't yet rotated to
   a segment. File a keplordb issue with the moved file attached.

### Ingestion is returning 503 / 507 in volume

- **`503 queue full`** — the BatchWriter mpsc channel is saturated.
  First check `/health` for `queue_utilization_pct`; if pinned at 100,
  either ingestion exceeds write throughput (bigger problem) or the
  flush task is stuck. Scrape `/metrics` and look for
  `keplor_batch_flush_errors_total` increasing — that points at the
  store path. If errors are zero, raise `pipeline.channel_capacity`
  and `pipeline.batch_size` and watch again.
- **`507 Insufficient Storage`** — `storage.max_db_size_mb` exceeded.
  Run `keplor gc --older-than-days <N>` to reclaim, lower retention,
  or grow the disk. Do **not** raise `max_db_size_mb` past your real
  disk free space — `df` won't lie.

### Segment count blowing up / disk growing fast

The BatchWriter `wal_checkpoint`s after every flush, so at the
hard-coded 50 ms cadence each tier produces ~1200 small segments/min
at idle. This is intentional — segment GC reclaims them on the
retention schedule. If the segment count is unbounded:

1. `du -sh /var/lib/keplor/<tier>/*` — find the offending tier.
2. Check `keplor stats --data-dir /var/lib/keplor` for events-per-tier.
3. Confirm `storage.gc_interval_secs > 0` and `retention_days > 0`
   for the affected tier.
4. Run `keplor gc --older-than-days <N> --data-dir /var/lib/keplor`
   manually to force a sweep; if it returns quickly but segment count
   doesn't drop, the problem is segments straddling the retention
   boundary (segment GC is segment-granular, not row-granular). Drop
   `retention_days` until the boundary moves past the affected
   segments, sweep, then raise it back.

The 50 ms BatchWriter cadence is currently not exposed in
`pipeline.*` — if you need to slow it, the change is a one-line
`BatchConfig::default()` edit followed by a redeploy.

### A tier engine starts OOMing

Per-tier engines hold their own mmap LRU and rollup state. If RSS for
the keplor process climbs without bound:

1. `keplor archive-status --data-dir /var/lib/keplor` — large unarchived
   ranges keep the rollup-replay window hot.
2. Lower `storage.mmap_cache_capacity` (default 256) — trades a little
   read latency for a tighter cap.
3. Lower `storage.rollup_replay_days` (default 7) if a single tier
   carries many days of segments.
4. If a single tier is responsible, archive that tier aggressively —
   the per-tier engines mean isolated GC is cheap.

### Archive cycles failing

Per-chunk error isolation means individual S3 failures **never** delete
events from KeplorDB; they retry next cycle. There is no dedicated
metric for archive failures yet — each failed chunk emits a
`tracing::warn!` "archive chunk failed" log line. Page only when:

- the warn-line rate stays elevated across multiple cycles (real S3
  outage or credential rotation), or
- `keplor archive-status --data-dir /var/lib/keplor` shows the same
  un-archived ranges persisting for >24h.

Manual one-shot (requires a binary built with `--features s3`):

```
keplor archive --config keplor.toml --older-than-days <N>
```

Run with `KEPLOR_LOG=info` to see the underlying error per chunk.

### Auth failures spike

`keplor_auth_failures_total` rising fast usually means credential
stuffing or a deployment that forgot to update its API key. Cross-check
`/var/log/keplor/access` (or your log aggregator) for source IPs;
single-IP bursts are scanners — block at the proxy, do not raise
`rate_limit.requests_per_second` to "absorb" them.

### Graceful shutdown taking too long

`server.shutdown_timeout_secs` (default 25s) bounds the BatchWriter
drain. If shutdowns hit the timeout:

1. Check the queue depth on `/health` before sending SIGTERM next time.
2. If the queue is consistently >10K at shutdown, either pre-drain
   (stop the load balancer first, wait 5s, then SIGTERM the process)
   or raise `shutdown_timeout_secs` until the deploy reliably finishes
   under the limit.
