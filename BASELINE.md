# BASELINE.md — Keplor Performance Baselines

Phase 2 output. All measurements on in-memory SQLite, `--release` profile
(`opt-level = "z"`, fat LTO, `codegen-units = 1`, `panic = abort`).

## 1. Throughput tests (pre-optimization)

Source: `crates/keplor-store/tests/integration.rs`

| Test | Events | Elapsed | Rate | Notes |
|------|--------|---------|------|-------|
| `append_event` (serial, single) | 1,000 | 416ms | **2,403 ev/s** | Compression inside lock |
| `append_batch` (64-event chunks) | 10,000 | 2.19s | **4,571 ev/s** | Pre-lock compression |
| `BatchWriter` (fire-and-forget) | 10,000 | 2.46s | **4,059 ev/s** | Async channel + batch |

## 2. Criterion benchmarks (baseline, pre-optimization)

### SHA-256

| Input size | Time | Throughput |
|-----------|------|-----------|
| 256 B | 1.54 µs | 159 MiB/s |
| 1 KB | 4.69 µs | 208 MiB/s |
| 4 KB | 17.4 µs | 224 MiB/s |
| 16 KB | 70.2 µs | 223 MiB/s |

Typical LLM request body: ~430 bytes. Double SHA = ~3.1 µs wasted per event.

### Zstd compression (level 3)

| Input | Time | Throughput |
|-------|------|-----------|
| Request body (~430 B) | 14.3 µs | 30 MiB/s |
| Response body (~260 B) | 12.3 µs | 20 MiB/s |

### Component splitting

| Operation | Time |
|-----------|------|
| `split_request` (OpenAI) | 3.6 µs |

### Batch write (in-memory SQLite)

| Batch size | Time/batch | Throughput |
|-----------|-----------|-----------|
| 32 | ~2 ms | ~16K ev/s |
| 64 | 4.07 ms | 15.7K ev/s |
| 128 | 7.63 ms | 16.8K ev/s |
| 256 | 16.3 ms | 15.8K ev/s |

### Single-event write

| Operation | Time |
|-----------|------|
| `append_event` | 291 µs |

### Query (1000 events seeded)

| Filter | Limit | Time |
|--------|-------|------|
| None | 50 | 343 µs |
| user_id | 50 | 364 µs |

### Catalog lookup

| Path | Time |
|------|------|
| Exact match | 89 ns |
| Date-suffix fallback | 86 ns |
| Prefix fallback | 85 ns |
| Complete miss (worst case) | 7.5 µs |
| `ModelKey::new` | 135 ns |
| `Catalog::load_bundled` | 31 ms |

## 3. Throughput tests (post-optimization)

After applying: `spawn_blocking` for batch flush, double-SHA elimination,
static metrics labels.

| Test | Events | Elapsed | Rate | Change |
|------|--------|---------|------|--------|
| `append_batch` (64-event chunks) | 10,000 | 1.67s | 6,003 ev/s | within noise (single wall-clock run) |
| `BatchWriter` (fire-and-forget) | 10,000 | 437ms | **22,905 ev/s** | **+464%** |

**Note on `append_batch`**: The 4,571 → 6,003 difference is likely
run-to-run variance, not a real improvement. The double-SHA elimination
does not fire in this test because `make_event()` sets
`request_sha256 = [0u8; 32]`, which triggers the fallback path. A true
measurement requires events with pre-computed SHAs (as the pipeline
produces). The theoretical savings are ~3 µs/event based on the SHA-256
criterion benchmark.

The `BatchWriter` improvement is real and structural: `spawn_blocking`
frees the tokio worker during SQLite I/O, allowing the channel receiver to
continue buffering events instead of being blocked. This leads to larger
effective batches and better dedup (blobs went from 20,007 to 2,311 for
10K events).

## 4. Profiling notes

- **CPU profiling**: not performed (no `cargo flamegraph` in this session).
  Criterion results point to zstd compression as the dominant cost in the
  batch write path (~14 µs per blob vs ~1.5 µs for SHA-256).
- **Allocation profiling**: not performed (`dhat-rs` not integrated).
  Recommended for future work.
- **Async runtime**: `tokio-console` not tested. The `spawn_blocking` fix
  addresses the theoretical blocking concern; production load testing
  would confirm p99 improvement.
