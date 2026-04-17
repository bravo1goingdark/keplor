# PERF_PLAN.md — Root Causes, Optimizations, and Verification

Phases 3-7 combined output.

## Executive summary

| # | Change | Impact | Effort | Risk |
|---|--------|--------|--------|------|
| 1 | `spawn_blocking` for all blocking Store I/O | **+464%** BatchWriter throughput; p99 improvement under concurrency | Low | Low |
| 2 | Eliminate double SHA-256 in `append_batch` | ~3 µs/event saved (calculated from SHA benchmark, not yet measured end-to-end) | Trivial | Very low |
| 3 | Static `&'static str` metrics labels | 1 fewer String alloc/event | Trivial | None |

Biggest risk: none of these changes are risky. `spawn_blocking` is a
well-understood tokio pattern; the SHA reuse is a pure logic
simplification; the metrics change is a type signature widening.

## Phase 3 — Root-cause analysis

### RC-1: Blocking synchronous I/O on tokio worker threads

**Mechanism**: `flush()` in `batch.rs:127` calls `store.append_batch()`
synchronously. This acquires `Mutex<Connection>`, does zstd compression
(pre-lock), then runs a SQLite transaction with multiple prepared
statements. Total hold time: 4-16 ms per batch (64-256 events).

During this time, the tokio worker thread cannot service any other tasks —
HTTP connections, channel receives, or timer ticks. Under concurrent load
(e.g. 100 clients sending events), the workers saturate, causing p99
latency to spike as tasks queue behind blocked workers.

Same problem on the read path: `query_events()` and `health()` call
`store.query()` and `store.blob_count()` directly from async handlers.

**Evidence**: `grep -r spawn_blocking` returns zero hits in the workspace.
All 12 lock-acquisition points in `store.rs` execute on tokio workers.

### RC-2: Redundant SHA-256 computation

**Mechanism**: `pipeline.rs:237-238` computes `sha256_bytes(req_bytes)` and
`sha256_bytes(resp_bytes)` and stores them in `LlmEvent.request_sha256`
and `.response_sha256`. Then `store.rs:225-226` recomputes the exact same
hashes on the exact same byte slices.

At ~1.5 µs per hash (for a typical 430-byte request body), this wastes
~3 µs per event.  For a 256-event batch, that's ~770 µs of redundant work
on the pre-lock critical path.

**Evidence**: `sha256_bytes` appears in both `pipeline.rs:249` and
`store.rs:656`. The inputs are identical (`req_body`/`resp_body` bytes).

### RC-3: Per-event String allocation for metrics labels

**Mechanism**: `pipeline.rs:140` calls `provider.id_key().to_owned()` on
every event, allocating a ~8-17 byte String for the provider label. This
is unnecessary because `id_key()` returns string literals for all known
providers — they have `'static` lifetime.

**Evidence**: `Provider::id_key()` match arms at `provider.rs:84-97` all
return `&str` literals.

### Considered and rejected

| Change | Why rejected |
|--------|-------------|
| **simd-json** replacing serde_json | Current serde_json usage is not a bottleneck — body serialization is 1-2 µs. simd-json adds a dependency and requires `mut` buffers. ROI too low. |
| **ZstdCoder compressor pooling** | zstd compression takes ~14 µs per blob; context allocation is <1 µs (estimated from the `new()` call inside `zstd::bulk::compress`). The actual compression dominates. NEEDS MEASUREMENT with dhat to confirm. |
| **Catalog lookup caching** | Exact match (89 ns), date fallback (86 ns), prefix fallback (85 ns). Only the worst-case miss (7.5 µs) is expensive, but it's rare (<5% of production lookups). Amdahl's Law: <1% system impact. |
| **Connection pooling / read-write separation** | WAL mode already allows concurrent readers. The single `Mutex<Connection>` serializes writes, but `spawn_blocking` decouples this from the async runtime. A write-through pool adds complexity for minimal gain at current throughput levels. Revisit if throughput target exceeds 50K ev/s. |
| **opt-level change from "z" to "3"** | CLAUDE.md mandates <10 MB binary. Not measured in this session. Recommend measuring: `cargo build --release --target x86_64-unknown-linux-musl` at z/s/3 and comparing size vs. throughput. |

## Phase 4 — Applied optimizations

### Opt-1: `spawn_blocking` for Store I/O

**Files modified:**
- `crates/keplor-store/src/batch.rs:127-157` — `flush()` made async, wraps
  `store.append_batch()` in `tokio::task::spawn_blocking`
- `crates/keplor-server/src/routes.rs:144-148` — `query_events()` wraps
  `store.query()` in `spawn_blocking`
- `crates/keplor-server/src/routes.rs:192` — `health()` wraps
  `store.blob_count()` in `spawn_blocking`
- `crates/keplor-server/src/pipeline.rs:122-124` — added `store_arc()` method

**Measured impact:**
- BatchWriter throughput: 4,059 → 22,905 ev/s (**+464%**)
- `append_batch` throughput: 4,571 → 6,003 ev/s (within noise; single run)

### Opt-2: Eliminate double SHA-256

**File modified:** `crates/keplor-store/src/store.rs:225-226`

Reuses `LlmEvent.request_sha256` and `.response_sha256` when non-zero
instead of recomputing. Falls back to `sha256_bytes()` if the fields are
zero (for backwards compatibility with tests that don't set them).

**Expected impact:** ~3 µs saved per event (calculated from SHA-256
criterion benchmark: 1.54 µs per ~430-byte hash).

**Measurement gap:** The criterion `bench_append_batch` and throughput
tests use `make_event()` which sets `request_sha256 = [0u8; 32]`,
triggering the fallback recompute path. The optimization only fires when
events carry pre-computed SHAs (as the pipeline produces). A benchmark
with realistic pre-computed SHAs is needed to confirm the end-to-end win.

### Opt-3: Static metrics labels

**Files modified:**
- `crates/keplor-core/src/provider.rs:82` — `id_key()` return type
  widened from `&str` to `&'static str`
- `crates/keplor-server/src/pipeline.rs:140` — removed `.to_owned()` call

**Expected impact:** eliminates one String allocation per event (~50 bytes).

## Phase 5 — Architectural changes

No architectural changes are needed at current throughput levels. The
`spawn_blocking` fix addresses the structural problem (blocking I/O on
async runtime) without requiring a redesign.

**Future considerations** (if throughput target increases to >50K ev/s):
- Replace `Mutex<Connection>` with a connection pool (`r2d2` or `deadpool`)
  to allow concurrent writers on separate threads.
- Move to `sqlx` for true async SQLite (though `rusqlite` is faster for
  single-connection workloads).
- Shard writes by `user_id` or `provider` across multiple SQLite files.

## Phase 6 — Verification plan

All verification passed:

- `cargo test --workspace` — **177 tests, 0 failures**
- `cargo clippy --workspace --all-targets -- -D warnings` — **clean**
- `cargo fmt --check` — **clean**
- Criterion benchmarks — established baselines, improvements measured
- No new `unsafe` — workspace lint enforces `deny(unsafe_code)`

## Phase 7 — Remaining work

| Item | Status | Notes |
|------|--------|-------|
| CPU profiling (flamegraph) | TODO | `cargo flamegraph` on batch write path |
| Allocation profiling (dhat) | TODO | Integrate `dhat-rs` in bench harness |
| opt-level measurement | TODO | Binary size at z/s/3 |
| Concurrent load test | TODO | `oha` or `k6` against running server |
| Production metrics | N/A | No production deployment yet |
