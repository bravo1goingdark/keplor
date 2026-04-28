# Failover — Primary → Follower Promotion

**Status: stub — depends on WAL tail API and follower script.**
Keplor today is a single-node deployment. There is no streaming
replication, no follower process, no WAL tail RPC. Per the deployment
plan (`/home/bravo1goingdark/.claude/plans/fluffy-leaping-meadow.md`),
production runs as a single GCP `e2-micro` instance in `asia-south1`.

This document is a **procedure stub** — it captures the failover design
we will adopt the moment WAL streaming and a follower binary land, so
that when those features ship, no design work is needed under
incident pressure. The manual workaround until then is **restore from
the most recent online checkpoint** (see `Engine::checkpoint` in
`keplordb/src/ops/checkpoint.rs`).

## Trigger

When implemented, invoke when ALL of: primary `/health` non-200 for
>2 min or unreachable; follower lag within RPO budget (target ≤ 5 min);
primary recovery slower than follower promotion.

For the manual (today) workaround the trigger is just: primary
unrecoverable.

## Verify

Future automated path:

1. Primary actually down — not a transient blip:
   ```
   curl -m 5 -sf https://$PRIMARY_HOST/health || echo DOWN
   gcloud compute instances describe $PRIMARY --zone $ZONE \
     --format='value(status)'
   ```
2. Follower healthy and caught up (**TODO: requires follower /health
   to expose `replication.lag_ms`**).
3. Decide: lag < RPO budget, primary recovery > MTTR budget → promote.

Manual checkpoint-restore path today:

1. Confirm primary is down (as above).
2. Confirm the last checkpoint is complete. Presence of
   `checkpoint_manifest.json` in the target dir means coherent;
   absence means partial/aborted (see `keplordb/src/ops/checkpoint.rs`
   line 49):
   ```
   ls -lh /backup/keplor/checkpoints/
   cat   /backup/keplor/checkpoints/latest/checkpoint_manifest.json
   ```
3. Confirm the checkpoint's `schema_id` matches the binary you're
   about to start (else see `schema-migration.md`).

## Fix

### Future automated path (when WAL tail + follower binary ship)

1. **TODO: requires `keplor follower stop-tail`** — stop the WAL
   tailing loop on the follower.
2. **TODO: requires `keplor follower promote --confirm`** — flips mode
   to read/write primary.
3. Repoint clients (DNS / LB) at the new primary — this is the actual
   cutover.
4. Verify health and that writes land:
   `curl -sf https://$NEW_PRIMARY_HOST/health`
5. **TODO: requires `keplor follower bootstrap --from-primary`** —
   when the old primary returns, bootstrap it as a fresh follower
   (segment manifest has diverged).

### Manual checkpoint-restore (operational today)

1. Provision a replacement instance (same image, same `keplor.toml`).
2. Copy the latest checkpoint into the new instance's `data_dir`:
   ```
   gcloud compute scp --recurse \
     /backup/keplor/checkpoints/latest \
     new-primary:/var/lib/keplor/data
   ```
3. Sanity-check `checkpoint_manifest.json` on the new host:
   ```
   cat /var/lib/keplor/data/checkpoint_manifest.json
   ```
4. Start keplor — engine open will run WAL replay over the copied
   shards exactly like a fresh cold start:
   ```
   sudo systemctl start keplor
   curl -sf http://localhost:8080/health
   ```
5. Repoint clients at the new instance. Document the **data loss
   window** = checkpoint timestamp → primary failure timestamp. Events
   written in that window are lost; this is the RPO under the manual
   procedure and is the primary motivation for shipping WAL streaming.

### Implementation gaps to close

- **WAL tail API** — streaming protocol on the primary, anchored on
  `BatchWriter::flush` / segment rotation in `kdb_store.rs`.
- **Follower binary** — read-only mode that tails WAL and applies
  via `Engine::append`.
- **Promotion CLI** — `keplor follower promote` (flips mode, closes
  tail, opens listen socket).
- **Bootstrap flow** — `keplor follower bootstrap --from-primary
  --base-checkpoint=...` (copy checkpoint, then tail from manifest
  `created_at_secs`).
- **Health surface** — `/health.replication.lag_ms` for RPO alerts.

## Post-mortem template

1. Timeline (UTC) — primary fail, alert fired, follower promoted,
   clients cut over
2. Detection: how the failure was noticed
3. Customer impact: ingest gap, query gap, dollar magnitude if
   billing-affecting
4. Root cause: hardware, software, network, operator
5. Resolution: which procedure (automated promotion vs. manual
   checkpoint-restore), data-loss window
6. Action items: shorten the implementation-gap list, increase
   checkpoint cadence, alert earlier on primary-health degradation
