# Schema Migration

KeplorDB's column layout is **positional** — dim, counter, and label
indices are part of the on-disk segment format. Reordering or
reassigning any of them silently corrupts queries unless `SCHEMA_ID` is
bumped. The engine refuses to open a data directory whose segment
headers carry a `schema_id` that doesn't match
`EngineConfig.schema_id` (see `crates/keplordb/src/engine.rs` lines
318-325) — that's the safety net.

This runbook covers what to do when keplor-store's `SCHEMA_ID`
(currently `1`, declared in `crates/keplor-store/src/mapping.rs`) needs
to change.

## Trigger

- A code change adds, removes, or reorders dims / counters / labels in
  `crates/keplor-store/src/mapping.rs` — the test
  `mapping::tests::on_disk_layout_constants_unchanged` will fail in CI.
- New keplor binary refuses to start with the log line:
  ```
  segment "<path>" was written with schema_id=1 but engine configured with schema_id=2
  ```
- Operator deliberately rebuilding from scratch to reclaim disk after a
  schema flag-day.

## Verify

1. Confirm the schema id encoded in existing segments:
   ```
   xxd -l 16 $DATA_DIR/tier=pro/seg_*.kseg | head
   # Segment header layout is documented alongside `meta::scan_segments`.
   # The byte we care about is the schema_id field — match it against
   # the `SCHEMA_ID` constant the binary was built with.
   ```
2. Confirm the binary's `SCHEMA_ID`:
   ```
   strings $(which keplor) | grep -c keplor_store    # sanity
   grep '^pub const SCHEMA_ID' crates/keplor-store/src/mapping.rs
   ```
3. Confirm refusal-to-mount is what's failing:
   ```
   journalctl -u keplor -g "schema_id" --since "10 minutes ago"
   ```

## Fix

You have two strategies. **Pick one before touching production.**

### A. Rebuild from scratch (lossy, simple)

Use when historical data is replayable from upstream (LiteLLM, gateway,
client SDK) or when the affected window is short enough to write off.

1. Drain the running process so no further writes hit the old segments:
   ```
   sudo systemctl stop keplor
   ```
2. Optional: archive what's there before deletion (segments stay
   readable by the OLD binary — keep a copy until you're sure):
   ```
   tar -czf /backup/keplor-pre-migration-$(date +%F).tar.gz \
     -C /var/lib/keplor data/
   ```
3. Wipe the data dir:
   ```
   sudo rm -rf /var/lib/keplor/data/tier=*
   sudo rm -f  /var/lib/keplor/data/archive_manifests.jsonl
   ```
4. Deploy the new binary (with bumped `SCHEMA_ID`) and start:
   ```
   sudo systemctl start keplor
   ```
5. Engine cold-starts: empty data dir → eager tiers spawned with the
   new schema id → first writes land cleanly. Watch for clean startup:
   ```
   journalctl -u keplor -f -g "keplor server listening"
   ```

### B. Side-by-side replay (lossless, manual)

Use when you must preserve history and the change is non-trivial. There
is **no built-in dual-write path** — you wire it at the ingest source.

1. Bring up a second keplor on a different port + data dir, built with
   the new `SCHEMA_ID` (e.g. `:8090`, `data-v2`).
2. Configure the upstream ingest source (LiteLLM proxy, gateway,
   forwarder) to dual-write to both `:8080` (old) and `:8090` (new).
3. Backfill history. **TODO: requires a replay tool.** No
   `keplor replay --from $OLD --to $NEW` exists today. Options: (a)
   accept the cutover-window data loss and keep the old dir read-only
   for archival queries, or (b) write a one-off binary that opens the
   old `KdbStore`, calls `query_events_for_archive(i64::MAX)`, and
   feeds events into the new instance via `POST /v1/events/batch`.
4. Cut reads to the new process (LB / dashboard / SDK).
5. Stop the old process. Retain its data dir for ≥1 retention cycle.

### Tradeoff summary

| Aspect             | Rebuild from scratch | Side-by-side          |
|--------------------|----------------------|-----------------------|
| Historical events  | Lost                 | Preserved             |
| Operator effort    | Minutes              | Hours + custom tooling|
| Downtime           | Restart window only  | Zero (if dual-write)  |
| Disk required      | 1x current           | 2x during overlap     |

Rebuild is the right call for anything that isn't billing-grade
historical retention — keplor's S3/R2 archival path already preserves
old events independent of `SCHEMA_ID` (queries hit
`Archiver::fetch_archived_events`, not the engine).

## Post-mortem template

1. Timeline (UTC) — schema bump committed, deploy started, cutover done
2. Detection: planned migration vs. failed deploy
3. Customer impact: dashboard gap, query downtime, data loss window
4. Root cause: what the column reshape was for
5. Resolution: which strategy, when complete
6. Action items: ship a `keplor replay` tool, codify column-add
   discipline, update CI gate that fails on `SCHEMA_ID`-changing PRs
