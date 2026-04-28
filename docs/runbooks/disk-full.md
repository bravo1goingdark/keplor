# Disk Full — Segments + WAL

KeplorDB stores per-tier WAL shards plus rotated segment files under
`{data_dir}/tier=<name>/`. Storage growth is bounded by GC (drops
whole segments past retention) and, optionally, archival to S3/R2
(uploads zstd-compressed JSONL, deletes from local store on confirmed
upload). This runbook covers what to do when the disk fills despite
those.

## Trigger

- Ingest endpoints return `507 Insufficient Storage` — pipeline gate
  fired because `db_size_bytes >= storage.max_db_size_mb`. The hot path
  is `Pipeline::check_db_size` in `crates/keplor-server/src/pipeline.rs`.
  Counter: `keplor_events_errors_total{stage="storage_full"}`.
- Filesystem hits ENOSPC — appears as
  `keplor_batch_flush_errors_total` increasing alongside log lines from
  `BatchWriter::flush`: `batch flush failed`, with `error="...No space
  left on device..."`.
- Disk-utilization alert (node_exporter): mount holding `data_dir` is
  >90% full.
- Sustained high `keplor_batch_queue_depth` — back-pressure into the
  ingest channel because flushes are stalling on disk.

## Verify

1. Filesystem-level check:
   ```
   df -h /var/lib/keplor
   du -sh /var/lib/keplor/data/tier=*
   ```
2. Per-tier segment + WAL accounting:
   ```
   keplor stats -d /var/lib/keplor/data
   # Reports total segments, total bytes, per-tier engine state.
   ```
3. Confirm the GC and archive loops are even running:
   ```
   journalctl -u keplor --since "1 hour ago" -g "tiered gc completed"
   journalctl -u keplor --since "1 hour ago" -g "archive cycle completed"
   ```
   No GC lines in an hour with `gc_interval_secs=300` → the loop is
   stalled or never ran. No archive lines and `[archive]` is configured
   → S3 connectivity is broken (look for `S3 connectivity check failed`
   from the startup probe).
4. Manifest sidecar size — it is append-only and never compacted by GC:
   ```
   wc -c /var/lib/keplor/data/archive_manifests.jsonl
   ```
   Pathological growth (multi-GB) is rare but possible after years of
   archival; not the usual culprit.

## Fix

Order operations from cheapest-to-do to most disruptive.

1. **Run GC by hand to reclaim segments past retention.** This bypasses
   the periodic `gc_loop` / `gc_tiered_loop` and runs immediately:
   ```
   keplor gc --older-than-days 30 -d /var/lib/keplor/data
   ```
   Per-tier GC is preferred when retention tiers differ — the in-process
   loop calls `gc_tier(name, cutoff)` per configured tier in
   `[retention.tiers]`. From the CLI, just lower
   `--older-than-days` until disk pressure clears.
2. **Trigger an archive cycle** (only if `[archive]` is configured and
   the binary was built with `--features s3`). Old segments → R2/S3,
   then deleted from local store after upload confirmation. Per-chunk
   error isolation means a bad upload leaves events behind for retry,
   so failures here do not delete data:
   ```
   keplor archive --config /etc/keplor/keplor.toml --older-than-days 7
   ```
   Watch progress:
   ```
   journalctl -u keplor -f -g "archive"
   ```
   Inspect the manifest summary:
   ```
   keplor archive-status -d /var/lib/keplor/data
   ```
3. **Tighten retention or raise the cap** — both require a restart
   today (SIGHUP only reloads `[auth]`; see `key-rotation.md`). Edit
   `keplor.toml`, then `sudo systemctl restart keplor`. The next
   `gc_tiered_loop` tick (default 300s) drops newly-out-of-retention
   segments. The on-shutdown WAL checkpoint also flushes any in-WAL
   bytes, so a restart often reclaims whatever the periodic
   `wal_checkpoint_loop` hadn't gotten to.
   ```toml
   [[retention.tiers]]
   name = "free"
   days = 7              # was 30
   [storage]
   max_db_size_mb = 8192  # raise only if you have actual headroom
   ```
4. **Add disk** if the above fail. Segment files are immutable mmap'd
   files; moving `data_dir` to a larger volume is `systemctl stop` →
   `rsync -a` → start. `data_dir_lock` prevents a half-copied dir from
   being mounted, but only if the source is stopped before the final
   rsync.

### What NOT to do

- Do not delete files manually from `data_dir/tier=*/` or remove
  `archive_manifests.jsonl`. Segment removal must go through
  `Engine::gc()` (keeps tombstones + rollups consistent); the manifest
  is the only index from day-keys to S3 object paths.
- Do not edit `seg_*.kseg` headers to bypass `schema_id` validation.

## Post-mortem template

1. Timeline (UTC) — first 507, GC run, archive run, mitigation done
2. Detection: 507 alert / disk-utilisation alert / customer report
3. Customer impact: rejected events, dashboard gaps, retention loss
4. Root cause: retention / archive misconfiguration, ingest spike,
   undersized disk
5. Resolution: which step cleared it; how much disk reclaimed
6. Action items: tighten retention defaults, alert at
   `db_size_bytes / max_db_size_mb > 0.7`, validate archive credentials
   in CI, capacity-plan for 2x peak ingest
