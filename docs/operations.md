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
- [ ] Verify binary size: `ls -lh target/x86_64-unknown-linux-musl/release/keplor` (must be <10 MB on a default build, <12 MB with `migrate-from-sqlite`)
- [ ] **Do not** enable `--features migrate-from-sqlite` on production builds unless you actually need to import a legacy SQLite db — it links `rusqlite` (~2 MB) and adds the `keplor migrate-from-sqlite` subcommand.
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

### Alerting recommendations

- `keplor_batch_flush_errors_total` increasing: database write failures (check disk space)
- `queue_utilization_pct > 80` in health check: back-pressure, ingestion exceeding write throughput
- `keplor_auth_failures_total` spike: possible credential stuffing

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

**Option 2: Stop and copy**

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

## Upgrading

1. Back up the data directory (see above)
2. Build or pull the new binary/image
3. Verify the data directory: `keplor migrate --data-dir /var/lib/keplor` (idempotent — opens the store, refuses to mount a directory written under a mismatched `SCHEMA_ID`)
4. Restart the server

The server also opens the store on startup, which performs the same
schema-id check.

### Importing an old SQLite-backed store

If you're upgrading from a release older than the KeplorDB cutover (the
SQLite era), you need a binary built with `--features migrate-from-sqlite`
to run the one-shot import:

```bash
# Build with the migration feature on (NOT for production runtime use):
cargo build --release -p keplor-cli --features migrate-from-sqlite

# Migrate the old keplor.db into a fresh KeplorDB data dir.
# Resumable: a checkpoint is written after each batch.
./target/release/keplor migrate-from-sqlite \
  --source /var/lib/keplor.db \
  --dest /var/lib/keplor_data \
  --batch-size 10000

# Point the server's storage.data_dir at the new directory and restart.
# You can keep the source SQLite file around as a rollback for as long as
# disk space allows; the migration is non-destructive.
```

Then deploy the **default** (non-feature) build for ongoing runtime —
shedding the `rusqlite` dep keeps the binary under 10 MB.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `503 Service Unavailable` on ingest | Batch writer queue full | Reduce ingestion rate or increase `pipeline.batch_size` |
| `408 Request Timeout` | Request exceeded `server.request_timeout_secs` | Increase timeout or reduce batch size |
| Data directory growing unbounded | `retention_days = 0` (GC disabled) | Set `retention_days` to a positive value |
| Many tiny segment files | Default 50 ms BatchWriter cadence | Tune `pipeline.flush_interval_ms` upward; segment GC reclaims them on the retention schedule |
| `migrate-from-sqlite` subcommand "not found" | Default binary doesn't include it | Rebuild with `--features migrate-from-sqlite` |
| High memory usage | Large batch writer queue | Reduce `channel_capacity` in batch config |
