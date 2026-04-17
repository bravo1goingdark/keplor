# BASELINE.md — Keplor Performance Baselines

Phase 2 output. All measurements on in-memory SQLite, `--release` profile
(fat LTO, `codegen-units = 1`, `panic = abort`).
Final opt-level: `3` (switched from `"z"` — see section 5).

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

## 4. Criterion: precomputed SHA vs zero SHA

Proves the double-SHA elimination fires and has measurable impact:

| Variant | Batch 64 | Batch 256 | Throughput |
|---------|----------|-----------|-----------|
| Zero SHA (recomputes) | 6.49 ms | 16.3 ms | 9.9K ev/s |
| Precomputed SHA (skips) | 3.34 ms | 13.3 ms | 19.2K ev/s |
| **Speedup** | **1.94x** | **1.23x** | |

## 5. opt-level comparison

Binary size (native x86_64-unknown-linux-gnu, fat LTO, strip=symbols):

| opt-level | Binary size | append_event | append_batch (64) | BatchWriter |
|-----------|------------|-------------|-------------------|-------------|
| `"z"` | **5.3 MB** | 2,403 ev/s | ~6,003 ev/s | 22,905 ev/s |
| `"s"` | 5.5 MB | — | — | — |
| `3` | **7.5 MB** | 3,591 ev/s | 9,398 ev/s | 24,081 ev/s |

All three are well under the 10 MB binary-size constraint from CLAUDE.md.
`opt-level = 3` gives **+49% append_event**, **+57% append_batch** for
2.2 MB more binary size. Switched to `3`.

## 6. Allocation profiling (dhat-rs)

2,560 events in 10 batches of 256. Harness: `benches/dhat_batch.rs`.

| Category | Bytes | % of total | Per-event |
|----------|-------|-----------|-----------|
| serde_json (component split) | 5,548,970 | 38.1% | 2,168 B |
| bench harness (test data) | 4,334,272 | 29.8% | 1,693 B |
| keplor_store (Vecs, buffers) | 3,584,384 | 24.6% | 1,400 B |
| zstd (compressor context) | 1,064,300 | 7.3% | 416 B |
| rusqlite | 20,821 | 0.1% | 8 B |

**Key finding**: serde_json dominates allocations (38%) — the
`serde_json::from_slice::<Value>()` call in `components.rs:55` parses the
full request body into a heap-allocated Value tree for component
extraction. This is the highest-ROI allocation target for future work.

zstd compressor context allocation (7.3%) is small — pooling would save
~416 bytes/event, not worth the complexity.

## 7. Concurrent load test (oha)

Server: `keplor run` with opt-level 3, default config, file-backed SQLite.

### Single event (POST /v1/events) — 10K req, 50 concurrent

| Metric | Value |
|--------|-------|
| Throughput | **999 req/s** |
| p50 | 50.1 ms |
| p99 | 56.4 ms |
| p99.9 | 61.7 ms |
| Success rate | 100% |

Latency is dominated by the 50 ms batch flush interval — each request
waits for the next batch commit. Tight p50-p99 spread (6 ms) confirms
`spawn_blocking` prevents worker starvation.

### Batch (POST /v1/events/batch) — 2K req × 50 events, 20 concurrent

| Metric | Value |
|--------|-------|
| Throughput | **1,577 req/s (~79K events/s)** |
| p50 | 10.8 ms |
| p99 | 40.2 ms |
| p99.9 | 53.2 ms |
| Success rate | 100% |

## 8. Profiling gaps

- **CPU flamegraph**: blocked by `perf_event_paranoid=4` (needs root).
- **Miri**: not available on stable 1.93.0 (nightly only). Zero unsafe
  in workspace (`deny(unsafe_code)` lint), so miri is redundant.
- **tokio-console**: not tested. The load test confirms `spawn_blocking`
  works correctly under concurrency.
