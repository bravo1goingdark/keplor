# Phase 8 — Heavy compression: trained dicts and deduplication

**Status:** not started
**Depends on:** phase 3, phase 5 (real traffic to train on)
**Unlocks:** phase 11 (the compression differentiator story)

## Goal

Turn the compression story into a real differentiator. Target: 30–80× ratio vs raw NDJSON on a 7-day rolling traffic window.

## Prompt

### 1. Component-level deduplication (`keplor-store`)

Implement the splitter from phase 3 more intelligently:

- For OpenAI/Anthropic-style chat:
  - extract system prompt separately
  - extract tools array separately
  - user/assistant messages as one component
- For Responses API: extract `instructions` field separately.
- Hash each component with sha256, upsert into `payload_blobs` with refcount++.
- Add `payload_blobs.hit_count` (cumulative hits) and log dedup ratio as metric `keplor_dedup_hit_ratio`.
- Build a small in-memory LRU cache (1000 entries) of recent sha256 → blob_id to skip the INSERT round-trip on hot system prompts.

### 2. Dictionary training pipeline

Background task (`Trainer`):

- Wakes every `compression.trainer_interval_hours` (default 6h), or when the sample buffer for a `(provider, component_type)` key exceeds 4096 samples.
- Reservoir-samples up to 2048 recent blobs of that key.
- Calls `zstd::dict::from_samples` with `target_size = 112_640` bytes.
- **Eval**: recompress a held-out 256 sample set; compute size-with-new-dict / size-with-current-dict. Adopt if ratio ≤ 0.92.
- Atomic dict swap via `arc-swap::ArcSwap`.
- Old dicts stay in `zstd_dicts` table (referenced by existing blobs) forever.

### 3. Per-component encoder selection

`EncoderRegistry` picks `(dict_id, level)` by `(provider, component_type, size_bucket)`:

- Small blobs (<1 KiB) → no dict (dict overhead > content).
- Large structured blobs (tools schemas, system prompts) → dict + level 6.
- Response bodies → default level 3.

### 4. Re-compress-on-migration

Background sweeper walks old blobs compressed with the "none" dict:

- If the new size is smaller by ≥ 15% with the now-trained dict, recompress.
- Update the row. Decrement the old compressed-bytes counter.

### 5. Benchmarks (criterion)

- 10k fixture OpenAI responses → measure total bytes raw, gzip-6, zstd-3, zstd-3+dict, zstd-6+dict. Assert zstd-3+dict ≥ 4× gzip-6.
- Hot-path encoder throughput: MB/s single-threaded. Target > 200 MB/s for zstd-3 on a modern x86.
- Dedup hit ratio on a fixture trace of 1k requests with a shared system prompt: target ≥ 95% component hits after the first request.

### 6. `keplor stats --compression` subcommand

```
Raw bytes (bodies uncompressed):           4.23 GB
Stored bytes (compressed + deduped):      67.4 MB
Overall ratio:                             62.8×
By component:
  system_prompt:  4200 hits / 2 blobs     (99.95% dedup)
  tools:          3821 hits / 17 blobs
  messages:         12.1 MB raw / 482 KB stored (25.7×)
  response:         31.9 MB raw / 2.1 MB stored (15.2×)
```

### 7. Documentation

Document the pipeline in `docs/compression.md` with diagrams.

## Acceptance criteria

- [ ] The stats subcommand reports ≥ 30× on the provided real-world trace fixtures in `tests/fixtures/traces/`
- [ ] Add these fixture traces:
  - one conversational chat trace
  - one RAG trace with tools
  - one code-gen trace with long system prompt
- [ ] Benchmark report added to `docs/progress.md`
- [ ] `docs/compression.md` written
- [ ] `cargo test -p keplor-store` green
