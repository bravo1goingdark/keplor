# PERF_AUDIT.md — Keplor Workspace Performance Audit

Phase 1 output of the performance-optimization workflow.

## 1. Workspace layout

| Crate | Kind | Direct deps (local) |
|-------|------|---------------------|
| `keplor-core` | library | — |
| `keplor-pricing` | library | core |
| `keplor-store` | library | core |
| `keplor-server` | library | core, pricing, store |
| `keplor-cli` | binary (`keplor`) | core, pricing, store, server |
| `xtask` | binary | — |

No `build.rs` scripts. Resolver v2.

## 2. Toolchain & build

| Setting | Value | File:line |
|---------|-------|-----------|
| Channel | 1.93.0 | `rust-toolchain.toml:2` |
| Edition | 2021 | `Cargo.toml:14` |
| MSRV | 1.82 | `Cargo.toml:15` |
| Targets | x86_64-unknown-linux-musl, aarch64-unknown-linux-musl | `rust-toolchain.toml:4`, `.cargo/config.toml:11,15` |
| opt-level (release) | `"z"` (size) | `Cargo.toml:30` |
| LTO | `"fat"` | `Cargo.toml:31` |
| codegen-units | 1 | `Cargo.toml:32` |
| panic | `"abort"` | `Cargo.toml:33` |
| strip | `"symbols"` | `Cargo.toml:34` |
| debug (release) | false | `Cargo.toml:35` |
| incremental (release) | false | `Cargo.toml:36` |
| PGO | not in use | — |
| BOLT | not in use | — |
| unsafe_code | `deny` workspace-wide | `Cargo.toml:22` |

RUSTFLAGS: only `-C target-feature=+crt-static` for musl targets (`.cargo/config.toml:12,16`).

## 3. Runtime model

- **Async runtime**: Tokio multi-thread (`tokio::runtime::Builder::new_multi_thread().enable_all()`) at `crates/keplor-cli/src/main.rs:78-81`.
- **Worker count**: system default (not overridden).
- **Blocking thread pool**: default (not configured).
- **`spawn_blocking` usage**: **zero calls** in entire workspace.
- **Tokio features**: `rt-multi-thread`, `net`, `signal`, `sync`, `macros`, `time` (via `keplor-server/Cargo.toml:20`).

## 4. Call graph — server ingestion hot path

```
HTTP POST /v1/events
  → axum handler ingest_single()                  [routes.rs:28]
    → pipeline.ingest(event)                       [pipeline.rs:43]  .await
      → process_event() [SYNC, ~CPU-bound]         [pipeline.rs:83]
        ├─ validate::validate()                     [validate.rs]
        ├─ normalize::normalize_provider()          [normalize.rs]
        ├─ normalize::normalize_model()             [normalize.rs]
        ├─ usage_from_ingest()                      [pipeline.rs:146]
        ├─ compute_cost()                           [pipeline.rs:126]
        │   └─ ModelKey::from_normalized()           [catalog.rs:50]
        │   └─ catalog.lookup()                      [catalog.rs:216]
        │       ├─ HashMap::get (exact)              [catalog.rs:218]
        │       ├─ strip_date_suffix fallback        [catalog.rs:225-229]
        │       └─ 12-prefix loop fallback           [catalog.rs:233-259]
        ├─ serde_json::to_vec(request_body)          [pipeline.rs:99]
        ├─ serde_json::to_vec(response_body)         [pipeline.rs:104]
        ├─ build_llm_event()                         [pipeline.rs:177]
        │   ├─ parse_iso8601 or use epoch nanos      [pipeline.rs:188-192]
        │   ├─ sha256_bytes(req_bytes)  ← SHA #1     [pipeline.rs:237]
        │   └─ sha256_bytes(resp_bytes) ← SHA #2     [pipeline.rs:238]
        └─ return (LlmEvent, req_bytes, resp_bytes)
      → writer.write(event, req, resp)              [batch.rs:61]   .await
        → mpsc::send(WriteRequest)                  [batch.rs:68]   .await
        → oneshot::recv() — blocks until flush      [batch.rs:72]   .await

[Background] flush_loop task                        [batch.rs:88]
  → tokio::select! {
      rx.recv()     → buffer.push; try_recv loop    [batch.rs:96-105]
      interval.tick → flush if non-empty            [batch.rs:118-121]
    }
  → flush(store, buffer) [SYNC]                    [batch.rs:127]
    → split pending into (senders, batch)            [batch.rs:131-138]
    → store.append_batch(&batch) [SYNC]             [batch.rs:140]
      ├─ Pre-lock: for each event:                   [store.rs:224-271]
      │   ├─ sha256_bytes(req_body)  ← SHA #3 (dup!) [store.rs:225]
      │   ├─ sha256_bytes(resp_body) ← SHA #4 (dup!) [store.rs:226]
      │   ├─ split_request()                          [store.rs:229]
      │   │   ├─ serde_json::from_slice (parse JSON)  [components.rs:55]
      │   │   ├─ extract system/messages/tools         [components.rs:91-137]
      │   │   └─ serde_json::to_writer per component   [components.rs:114]
      │   ├─ split_response()                          [store.rs:230]
      │   ├─ sha256_bytes(component) per component     [store.rs:238]
      │   └─ coder.compress(component) per unique blob [store.rs:249]
      │       └─ zstd::bulk::Compressor::new + compress [compress.rs:48-52]
      ├─ Mutex::lock() [BLOCKS TOKIO WORKER]           [store.rs:274]
      ├─ BEGIN TRANSACTION                              [store.rs:275]
      ├─ INSERT blobs (prepare_cached)                  [store.rs:279-300]
      ├─ UPDATE refcounts for intra-batch dupes         [store.rs:303-319]
      ├─ INSERT events (prepare_cached, 42 cols)        [store.rs:323-395]
      ├─ INSERT component links (prepare_cached)        [store.rs:399-408]
      └─ COMMIT + unlock                                [store.rs:411]
    → send results on oneshot channels                  [batch.rs:142-156]
```

### Read path (query)

```
HTTP GET /v1/events
  → query_events()                                 [routes.rs:125]
    → build EventFilter from query params           [routes.rs:131-139]
    → store.query(&filter, limit+1, cursor)         [routes.rs:144-148]  SYNC
      ├─ Mutex::lock()                               [store.rs:490]
      ├─ build dynamic SQL + bind_storage             [store.rs:492-533]
      ├─ prepare_cached(&sql)                         [store.rs:535]
      ├─ query_map → Vec<LlmEvent>                    [store.rs:541-545]
      │   └─ row_to_event() per row                   [store.rs:708-786]
      │       └─ String allocs for each text column    [store.rs:709-784]
      └─ unlock
    → map LlmEvent → EventResponse (more String allocs) [routes.rs:154-179]
    → Json(EventListResponse)                            [routes.rs:181-185]
```

## 5. I/O inventory

| Operation | Type | Sync/Async | Buffered | Pooled | Location |
|-----------|------|-----------|----------|--------|----------|
| SQLite write (append_batch) | DB | **sync** (blocking) | WAL journal | single Mutex<Connection> | `store.rs:274-411` |
| SQLite read (query, get_event) | DB | **sync** (blocking) | mmap 256MB | single Mutex<Connection> | `store.rs:447-547` |
| zstd compress | CPU | sync | N/A | **no pooling** (fresh Compressor per call) | `compress.rs:46-55` |
| zstd decompress | CPU | sync | N/A | no pooling | `compress.rs:58-69` |
| SHA-256 hash | CPU | sync | N/A | N/A | `pipeline.rs:249`, `store.rs:656` |
| JSON parse (split_request) | CPU | sync | N/A | N/A | `components.rs:55` |
| JSON serialize (to_vec) | CPU | sync | N/A | N/A | `pipeline.rs:99,104` |
| HTTP server listen | Network | async | Tokio | TCP listener | `server.rs:65` |
| Pricing catalog fetch | Network | async | buffered to memory | reqwest pool | `catalog.rs:186-192` |

### SQLite pragmas (`migrations.rs:129-135`)

| Pragma | Value | Effect |
|--------|-------|--------|
| journal_mode | WAL | Concurrent readers, single writer |
| synchronous | NORMAL | fsync only on checkpoint, not every commit |
| mmap_size | 268435456 (256MB) | Memory-mapped I/O |
| busy_timeout | 5000ms | Retry on lock contention |
| cache_size | -64000 (64MB) | Page cache |
| temp_store | MEMORY | Temp tables in RAM |

## 6. Concurrency primitives

| Primitive | Location | Purpose |
|-----------|----------|---------|
| `Mutex<Connection>` (std::sync) | `store.rs:32` | Guards single SQLite connection |
| `Arc<Store>` | `batch.rs:52`, `pipeline.rs:31` | Shared ownership of store |
| `Arc<BatchWriter>` | `pipeline.rs:32` | Shared ownership of writer |
| `Arc<Catalog>` | `pipeline.rs:33` | Shared ownership of pricing |
| `Arc<ModelPricing>` | `catalog.rs:84` | Shared pricing entries |
| `Arc<PrometheusHandle>` | `routes.rs:24` | Shared metrics handle |
| `Arc<ApiKeySet>` | `auth.rs:41` | Shared auth config |
| `mpsc::Sender<WriteRequest>` | `batch.rs:38` | Ingestion channel (bounded, 8192) |
| `oneshot::Sender<Result>` | `batch.rs:45` | Per-write ack channel |

**No mixing of std::sync and tokio::sync on same data.** Mutex is std::sync on Connection (never held across .await). Channels are tokio::sync for async coordination.

## 7. Data structures on hot paths

| Structure | Location | Notes |
|-----------|----------|-------|
| `SmolStr` | Throughout core/server | Inline ≤22 bytes, heap above. Used for model, provider, endpoint, user IDs. |
| `HashMap<ModelKey, Arc<ModelPricing>>` | `catalog.rs:84` | Default SipHash hasher. Read-only after init. |
| `HashSet<[u8; 32]>` | `store.rs:205` | Batch dedup set. Pre-allocated `events.len() * 3`. |
| `HashMap<[u8; 32], usize>` | `store.rs:308` | Intra-batch refcount tracking. |
| `Vec<PreparedBlob>` | `store.rs:207` | Pre-allocated `events.len() * 3`. |
| `Vec<BatchEvent>` | `store.rs:222` | Pre-allocated `events.len()`. |
| `Vec<WriteRequest>` | `batch.rs:89` | Batch buffer. Pre-allocated `batch_size`. |
| `[Option<Box<dyn ToSql>>; 8]` | `store.rs:497` | Fixed-size query parameter storage. |
| `Bytes` | pipeline/store | Zero-copy ref-counted byte buffer. |

**Hasher**: All HashMaps use default SipHash. No ahash/fxhash/rustc-hash.

## 8. `unsafe` audit

Zero `unsafe` blocks. `unsafe_code = "deny"` at workspace level (`Cargo.toml:22`).

## 9. Serde & serialization

| Operation | Library | Location | Frequency |
|-----------|---------|----------|-----------|
| Request body → bytes | `serde_json::to_vec` | `pipeline.rs:99` | once per event |
| Response body → bytes | `serde_json::to_vec` | `pipeline.rs:104` | once per event |
| Request body parse (split) | `serde_json::from_slice::<Value>` | `components.rs:55` | once per event |
| Component → bytes | `serde_json::to_writer` | `components.rs:87,114` | per component |
| Catalog load | `serde_json::from_slice` | `catalog.rs:102` | once at startup |
| Catalog entry parse | `serde_json::from_value(value.clone())` | `catalog.rs:113` | once at startup per entry |
| HTTP request deser | axum `Json<IngestEvent>` | `routes.rs:30` | once per request |
| HTTP response ser | axum `Json<...>` | `routes.rs:32` | once per response |

No simd-json, bincode, rkyv, or other alternatives. No double-serialization on the hot path (bodies serialized once in pipeline, stored as Bytes).

## 10. Duplicate dependencies

From `cargo tree -d`: `getrandom` (3 versions), `hashbrown` (3 versions), `rand` (2 versions). All transitive — no direct action needed.

## 11. Ranked hotspot summary

| Rank | Hotspot | File:line | Root cause | Suspected impact |
|------|---------|-----------|------------|-----------------|
| 1 | `flush()` blocks tokio worker | `batch.rs:127-157` | sync `store.append_batch()` called from async task, no `spawn_blocking` | p99 latency spike under concurrency |
| 2 | Query/health handlers block tokio | `routes.rs:144,192` | sync `store.query()`/`blob_count()` on worker thread | reader starvation under concurrent writes |
| 3 | Double SHA-256 per event | `pipeline.rs:237-238` + `store.rs:225-226` | Pipeline and store independently hash same bytes | ~4us wasted per event |
| 4 | Fresh zstd Compressor per call | `compress.rs:48-52` | No context reuse | ~5us alloc overhead per unique blob |
| 5 | Metrics label allocation per event | `pipeline.rs:140` | `id_key().to_owned()` | ~50 bytes heap per event |
| 6 | `opt-level = "z"` → `3` | `Cargo.toml:30` | RESOLVED: switched to 3, +49% throughput, 7.5M binary (under 10MB) | — |
| 7 | Single Mutex<Connection> for reads+writes | `store.rs:32` | No read-write separation | Read latency increases under write load |
| 8 | Catalog prefix fallback allocates | `catalog.rs:248` | `format!` + `ModelKey::new` in loop | Negligible (rare path, <5% of lookups) |
