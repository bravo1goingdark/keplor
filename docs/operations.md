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
- [ ] Verify binary size: `ls -lh target/x86_64-unknown-linux-musl/release/keplor` (must be <12 MB)
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
- [ ] Dedicated data directory with sufficient disk space for the SQLite DB + WAL
- [ ] Use `--json-logs` for structured log aggregation (Datadog, Loki, etc.)
- [ ] Set `KEPLOR_LOG=info` (or `RUST_LOG=info`) — avoid `debug`/`trace` in production

### Monitoring

- [ ] Scrape `/metrics` with Prometheus (see Prometheus metrics table below)
- [ ] Alert on `keplor_batch_flush_errors_total` increasing (DB write failures)
- [ ] Alert on `queue_utilization_pct > 80` in `/health` (back-pressure)
- [ ] Alert on `keplor_auth_failures_total` spikes (credential stuffing)
- [ ] Monitor disk usage — SQLite + WAL can grow between GC runs

### Backup

- [ ] Schedule daily backups: `sqlite3 keplor.db ".backup /backups/keplor-$(date +%Y%m%d).db"`
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

### Backup

Keplor uses SQLite with WAL mode. To create a consistent backup:

**Option 1: Online backup via CLI (recommended)**

```bash
# Checkpoint WAL first, then copy
keplor gc --older-than-days 99999 --db keplor.db  # no-op GC forces WAL checkpoint
cp keplor.db keplor.db.backup
```

**Option 2: Stop and copy**

```bash
# Stop Keplor (graceful shutdown checkpoints WAL automatically)
docker compose stop keplor

# Copy the database file
cp /var/lib/keplor/keplor.db /backups/keplor-$(date +%Y%m%d).db

# Restart
docker compose start keplor
```

**Option 3: sqlite3 .backup command**

```bash
sqlite3 /var/lib/keplor/keplor.db ".backup /backups/keplor-$(date +%Y%m%d).db"
```

This is safe to run while Keplor is writing — SQLite's `.backup` command handles WAL correctly.

### Restore

```bash
docker compose stop keplor
cp /backups/keplor-20260418.db /var/lib/keplor/keplor.db
docker compose start keplor
```

### Scheduled backups

```bash
# crontab: daily backup at 03:00, retain 7 days
0 3 * * * sqlite3 /var/lib/keplor/keplor.db ".backup /backups/keplor-$(date +\%Y\%m\%d).db" && find /backups -name 'keplor-*.db' -mtime +7 -delete
```

## Garbage Collection

Automatic GC runs hourly when `storage.gc_interval_secs > 0` (default: 3600). It runs one pass per configured retention tier, deleting events older than each tier's retention window.

Manual GC:

```bash
keplor gc --older-than-days 30 --db keplor.db
```

## Upgrading

1. Back up the database (see above)
2. Build or pull the new binary/image
3. Run migrations: `keplor migrate --db keplor.db`
4. Restart the server

Migrations are idempotent — running them multiple times is safe. The server also applies migrations automatically on startup.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `503 Service Unavailable` on ingest | Batch writer queue full | Reduce ingestion rate or increase `pipeline.batch_size` |
| `408 Request Timeout` | Request exceeded `server.request_timeout_secs` | Increase timeout or reduce batch size |
| Database file growing unbounded | `retention_days = 0` (GC disabled) | Set `retention_days` to a positive value |
| WAL file very large | Checkpoint interval too long or heavy write load | Decrease `wal_checkpoint_secs` |
| High memory usage | Large batch writer queue | Reduce `channel_capacity` in batch config |
