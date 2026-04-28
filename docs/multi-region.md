# Multi-Region Readiness Audit

Status: analysis only. Nothing in this document is on a roadmap. The point is to surface the design before the next traffic spike forces a hurried decision.

## TL;DR

- Today Keplor runs as a single node in Mumbai (`asia-south1`); every primitive in the codebase assumes one writer per data directory.
- The proven failure mode is cross-region latency: Cloudflare Workers in EU/US PoPs add ~115-135 ms (EU) and ~210-240 ms (US) RTT to every ingest.
- The cheapest fix is read replicas (active-passive) — primary keeps writing in Mumbai, a follower in EU or US serves dashboards and exports.
- The blocking primitive is a WAL tail API. It does not exist. The atomic checkpoint shipped in `ops/checkpoint.rs` is the right foundation but only delivers point-in-time copies, not continuous replication.
- Active-active is out of scope. KeplorDB has no conflict resolution, no consensus, no per-event vector clock — and likely never will, given the append-only single-writer design.

## 1. Why multi-region matters

Obol's frontend and Workers run on Cloudflare's edge (~300 PoPs). Every ingest call from a Worker today crosses the public internet to a single VM in Mumbai. Approximate RTTs from a Cloudflare PoP near each region to `asia-south1`:

| User region | RTT to Mumbai (typical) | Notes |
|---|---|---|
| India / SE Asia | 5-40 ms | Current happy path |
| Europe (LON/FRA) | 115-135 ms | Submarine cables via Suez |
| US East (IAD) | 210-230 ms | Two transcontinental hops |
| US West (PDX/SFO) | 220-240 ms | Pacific or trans-Atlantic+US |
| Australia / NZ | 130-160 ms | Direct cable, but variable |

Numbers above are cable-route-dependent and should be measured before any architectural commitment is made. The shape, not the digits, is what matters: EU users pay an extra ~120 ms p50 over the in-region case; US users pay ~220 ms. Both are larger than Keplor's own ingest service time (single-digit ms per `keplor_ingest_duration_seconds` in production), so latency is dominated entirely by the long-haul leg.

For an SDK that does fire-and-forget ingest this is harmless. For a synchronous proxy path where the user waits on the ingest call, it is the entire latency budget.

## 2. Current architecture constraints

The code makes single-node assumptions in several places. Every one of them would need to change or be redesigned around for a multi-writer setup.

- **Process-level data dir lock.** `crates/keplordb/src/storage/lock.rs` acquires an exclusive `flock` on `{data_dir}/.kdb.lock` in `Engine::open` (`DataDirLock::try_acquire`). A second engine on the same dir returns `DbError::AlreadyOpen` by design. This is a correctness guard against silent manifest corruption — not a bug, a feature.
- **WAL is single-writer per shard.** `crates/keplordb/src/write/wal.rs` shards the WAL N ways for write parallelism, but each shard is a `Mutex<WalShard>` owned by the local engine. There is no shared journal, no replication endpoint, no append-from-network path.
- **Segment manifest is in-process.** `Engine` holds the manifest as `ArcSwap<SegmentIndex>` (`engine.rs:198`). Reads are lock-free atomic loads, writes clone-and-swap. Manifest state is not persisted as a separate file — it is reconstructed by scanning `.kseg` files at startup. Two processes scanning the same dir would each build their own manifest with no cross-process invalidation.
- **No replication primitives.** Grep of the keplordb crate finds no `tail`, `follow`, `replicat*`, or `follower` symbols. The repo has zero machinery for streaming events out to a peer.
- **SIGHUP reload is per-process.** `sighup_reload_loop` (`server.rs:595`) re-parses the local TOML and atomically swaps the in-memory `ApiKeySet`. It does not coordinate with peers. Two nodes accepting writes against the same logical service would need a config-distribution story (file sync, control plane, k/v store) that does not exist.
- **Auth identity is local.** `ApiKeySet` (`auth.rs:31`) is built once at boot and reloaded from disk on SIGHUP. There is no shared key store; every node would need the same TOML.
- **No internal RPC.** The server exposes only the public HTTP API on `/v1/*`, `/health`, `/metrics`. There is no separate listener for inter-node traffic.

The good news: the storage layer is append-only with content-addressed segments and a `schema_id` in `EngineConfig` (`engine.rs:156`). Append-only is the easy case for replication; there is no in-place mutation and no row-level update conflict.

## 3. Read-replica path (active-passive)

The minimum viable multi-region story is one writable primary in Mumbai plus one or more read-only followers in EU/US. Followers serve dashboards, exports, and rollup queries; writes always cross-region back to Mumbai.

Sketch of how it would work:

- Primary writes WAL + segments to local disk exactly as today.
- Follower runs the same `keplor` binary against its own data directory. It boots from a checkpoint shipped from the primary (`Engine::checkpoint(target_dir)` already produces a coherent point-in-time copy).
- Follower polls a primary endpoint that streams the WAL tail since a known offset, then replays records into its local engine using the existing recovery path (`write/recovery.rs`).
- Follower must run with the same `schema_id` as the primary; the `checkpoint_manifest.json` shipped during bootstrap records this and can be verified on follower startup.
- Reads on the follower are eventually consistent — typically lagging the primary by the poll interval plus replay time.
- Failover is operator-driven: stop the primary, promote a follower by changing its config to point at a writable role and restarting, then update DNS. No automatic election, no split-brain protection beyond the data dir lock.

What is missing for this to work, in order of how much new code each implies:

- **WAL tail API.** A read-side endpoint on the primary that streams WAL records starting from a given (shard_id, offset) cursor. The recovery format already exists (`write/recovery.rs` reads framed records); exposing it over HTTP/gRPC is the new surface area.
- **Follower state machine.** A loop that maintains per-shard cursors, polls the primary, calls into the engine to replay a batch, and persists the cursor durably so it can resume after a follower crash.
- **Authentication and authorization for the replication channel.** Distinct from public API keys; followers need read-everything privileges that public clients should not have.
- **Snapshot bootstrap protocol.** Either ship a checkpoint manually on first start or expose a "fetch checkpoint" endpoint that streams the tarball. Today the operator copies the dir by hand.
- **Observability.** Replication lag (records or seconds), bytes shipped, follower last-applied cursor — none of these metrics exist.
- **Read-after-write semantics.** Today every read sees the writer's manifest. With a follower, callers that just wrote and immediately read may see stale data unless they explicitly hit the primary. This is a routing question, not a storage question, but it has to be decided before the dashboards talk to followers.

## 4. Active-active

Not viable today and probably not a goal. KeplorDB is a single-writer columnar log. There is no conflict-resolution model, no consensus protocol, no per-event vector clock or hybrid logical clock, and no logical-segment ID that two writers could safely allocate without coordination. EventIDs are generated locally (`EventId::new`) and are unique under the assumption of one writer; segment counters are per-shard atomics (`engine.rs`). Making this safe under two concurrent writers means designing all of those: an ID allocation scheme that does not collide, a merge story for segments produced in two places, a way to detect divergence when partitions heal, and a rollup accumulator that can absorb out-of-order multi-source input. That is a different database. If we ever need active-active we should look at running an external system (a real distributed log, or a managed service) rather than retrofitting one.

## 5. Cross-region WAL shipping options

Once the WAL tail API exists, three rough shapes for actually moving the bytes:

| Option | How | Lag | Cost | Complexity |
|---|---|---|---|---|
| Pull-based | Follower polls primary `/v1/replicate?after=cursor` | Poll interval (1-10 s) | Egress bytes only | Lowest. Stateless on primary side. |
| Push-based | Primary streams WAL records to a follower socket | Sub-second | Egress + persistent connection | Primary must track follower health, retry, and back-pressure. |
| Object-store-mediated | Primary uploads WAL chunks to R2; follower polls R2 | Chunk interval (10-60 s) | Egress + R2 PUT/GET | Decouples primary from follower count; single failure domain on R2. |

Recommendation: pull-based first. It is the simplest to reason about, has no in-flight state on the primary, fails closed (a down follower is just a stalled cursor), and reuses the existing public HTTPS surface. Move to object-store-mediated only if we need to support more than one or two followers, or if we want primary failure to be survivable without follower coordination changes. Push-based is the most operationally fragile of the three and should be skipped.

A note on the "what state replicates" question: only the WAL needs to ship. Segments are derived state — the follower's engine produces its own segments from the replayed WAL via the same `wal_max_events` rotation logic. Tombstones are written by GC, which the follower runs locally on the same retention config. Daily rollups are recomputed on replay. The replication payload is therefore exactly the framed records `write/recovery.rs` already understands.

## 6. DNS and load-balancer failover

For active-passive, DNS is the failover lever:

- Primary lives at a hostname like `ingest.keplor.useobol.com`. Operator-driven failover means flipping that A/AAAA record from the Mumbai IP to the promoted follower's IP. TTL must be low (60 s or less) to bound the staleness window.
- A health-checked load balancer (Cloudflare Load Balancer, Route 53 health checks) can automate the flip. Health check should hit `/health` and verify both that the response is 200 and that the WAL is fresh enough — a follower that has been promoted but is still catching up should not advertise as healthy.
- Read replicas are best fronted by a separate hostname, e.g. `read.keplor.useobol.com`, so callers can opt into eventual-consistency reads. The dashboard backend, exports, and analytics queries are good candidates. The proxy hot path should stay on the primary hostname.
- TLS coverage: a single wildcard `*.keplor.useobol.com` cert covers both names. No new cert work.

Split-brain risk under automated DNS failover is real. If the network partition is between regions and not on the primary itself, a Route 53 / CF health check from one vantage point may mark Mumbai down while it is still happily accepting writes from clients on its own side of the partition. The data dir lock prevents two engines on the same dir, but if the promoted follower in EU starts taking writes while Mumbai is still up, they diverge silently. Mitigation: gate failover on operator confirmation, or accept manual failover for the foreseeable future.

## 7. Schema migration coordination

`schema_id` is part of `EngineConfig` (`engine.rs:156`) and is stamped into every segment header. A primary and follower running with different `schema_id` values cannot replicate to each other safely.

Today's procedure for schema changes is stop-the-world: bring down the primary, run the migration, restart. With a follower, the procedure becomes:

1. Drain in-flight writes on the primary (stop accepting new ingest).
2. Wait for the follower's last-applied cursor to reach the primary's WAL head.
3. Stop both engines.
4. Run the migration on the primary (changes `schema_id`).
5. Take a checkpoint of the migrated primary.
6. Ship the checkpoint to the follower; verify `checkpoint_manifest.json` (`ops/checkpoint.rs`) records the new `schema_id`.
7. Start primary, then follower; replication resumes against the new schema.

A future rolling-upgrade story (where primary and follower can run different schema versions briefly) requires backwards-compatible schema changes — additive columns only, never deletions or type changes. That is a bigger commitment than we should make now.

## 8. Cost / complexity verdict

Single-region single-node is fine to roughly 10k events/sec sustained on the e2-micro Mumbai box from the deployment plan, assuming current event sizes and the existing WAL shard count. That ceiling moves up, not down, with mimalloc and tuned batch sizes — see the operations guide. The latency penalty for non-Mumbai users is real but is paid by the SDK / Worker, not by Keplor itself, so it does not threaten the box. Multi-region is operationally expensive: a second VM, replication monitoring, failover runbooks, schema-coordination procedures, and a non-trivial chunk of new code for the WAL tail API and follower loop. None of that pays for itself today. The recommended trigger is one of: sustained ingest above ~5k events/sec where the e2-micro headroom is gone and we are about to pay for a bigger machine anyway; or the first customer who asks for an SLA with a regional latency guarantee that Mumbai-only cannot meet. Until one of those happens, the right move is to keep this document warm, measure cross-region p99 from real Worker traces, and not build.
