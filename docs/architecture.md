# Keplor Architecture

This document is the technical blueprint for Keplor. Read it before starting any phase of work. It is authoritative on design decisions; the phase prompts describe *what* to build, this document explains *why*.

---

## System diagram

```
Client ──HTTPS──▶ [Listener: axum + hyper 1.x, rustls + aws-lc-rs]
                    │
                    ├──▶ Request body tee (Bytes clone → bounded mpsc)
                    │                               │
                    │                               ▼
                    │                    [Capture task]
                    │                    ├─ Detect provider from Host + path
                    │                    ├─ Parse JSON header for model name
                    │                    ├─ Buffer for cost calculation
                    │                               │
                    ▼                               │
              Upstream HTTPS client                 │
             (hyper-rustls + reqwest)               │
                    │                               │
                    ▼                               │
              Provider (OpenAI / Anthropic / …)     │
                    │                               │
                    ▼                               │
             Response body tee ◀─────────────────┐  │
                    │                            │  │
                    ▼                            ▼  ▼
              Client ◀──────   [SSE / AWS-ES / NDJSON reassembler]
                                    ├─ Normalize to UsageEvent
                                    ├─ Compute cost (LiteLLM catalog)
                                    ├─ Zstd-dict encode raw bytes
                                    ├─ Persist to SQLite / S3
                                    └─ Emit OTLP span (GenAI + OpenInference)
```

The **bounded mpsc backpressure on the capture side** keeps memory flat under burst; the **separate reassembler task per stream** enables per-provider parsing without blocking the forwarder; the **zstd-dict encoder** turns multi-GB/day raw traffic into tens of MB of durable storage.

---

## Provider wire formats (the core correctness problem)

A transparent proxy's correctness is defined entirely by how faithfully it reassembles each provider's streaming grammar. The grammars diverge enough that a one-size-fits-all SSE parser will silently corrupt usage accounting. Concrete specifics follow.

### OpenAI

Two surfaces:

**Chat Completions** (`POST /v1/chat/completions`) streams SSE frames with only a `data:` line per chunk and a terminal **`data: [DONE]`** sentinel that is **not JSON** — the parser must byte-match before `serde_json::from_str`. Usage arrives only in a trailing chunk with empty `choices` if the client sets `stream_options.include_usage: true`; the field shape is:

```
usage.{prompt_tokens, completion_tokens,
       prompt_tokens_details.cached_tokens,
       completion_tokens_details.reasoning_tokens}
```

**Responses API** (`POST /v1/responses`) uses named SSE events:

```
event: response.output_text.delta
event: response.reasoning_summary_text.delta
event: response.completed
...
```

Each carries a matching `type` field and monotonic `sequence_number`. There is **no `[DONE]`** — completion is signaled by `response.completed`. Usage is renamed to `input_tokens` / `output_tokens` with `input_tokens_details.cached_tokens` and `output_tokens_details.reasoning_tokens`. Reasoning item content is either a summary or an opaque `encrypted_content` blob when the client opts in — round-trip byte-exact.

### Anthropic Messages

`POST /v1/messages` uses `event:` + `data:` framing with six event types:

- `message_start`
- `content_block_start`
- `content_block_delta` with subtypes: `text_delta`, `input_json_delta`, `thinking_delta`, `signature_delta`, `citations_delta`
- `content_block_stop`
- `message_delta`
- `message_stop`

Plus `ping` and `error`.

**Critical gotchas** (test explicitly):

- The `message_delta.usage` object is *cumulative totals*, not a delta — naïvely summing with `message_start.usage` double-counts. This bug is present in many SDKs.
- `input_tokens` excludes cached tokens; the true prompt size is `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`.
- Thinking blocks must be round-tripped byte-exact, including the `signature` field, or subsequent tool calls fail.

### Google Gemini

The most error-prone wire contract.

- `streamGenerateContent` defaults to a **single progressive JSON array** (`Content-Type: application/json`, elements separated by `\n,\n`) rather than NDJSON or SSE.
- Appending `?alt=sse` switches to standard `data: {...}\n\n` SSE with no `[DONE]`.
- `usageMetadata` is cumulative and carries provider-specific quirks:
  - On **Vertex AI**: `totalTokenCount = promptTokenCount + candidatesTokenCount + thoughtsTokenCount + toolUsePromptTokenCount`
  - On the **public AI Studio API**: `candidatesTokenCount` already *includes* thoughts.

Normalize defensively by detecting the host, or bill customers wrong.

### AWS Bedrock ConverseStream

AWS event-stream binary framing:

- 12-byte prelude: `total_length` u32 BE, `headers_length` u32 BE, prelude `crc32`
- Typed headers: `:message-type`, `:event-type`, `:content-type`
- JSON payload
- Trailing `crc32` over the whole message

Event types mirror Anthropic semantics: `messageStart`, `contentBlockDelta` (`text` / `toolUse` / `reasoningContent` subtypes), `contentBlockStop`, `messageStop`, `metadata`.

Usage in the terminal `metadata` frame: `inputTokens`, `outputTokens`, `cacheReadInputTokens`, `cacheWriteInputTokens`.

`InvokeModelWithResponseStream` wraps model-native chunks as base64 inside `{"chunk": {"bytes": "..."}}` frames, so the proxy must double-decode for Anthropic-on-Bedrock and Claude-native-on-direct to share reassembly logic. Factor common code.

### Other providers

- **Azure OpenAI** — OpenAI-compatible but uses `api-key` header plus `api-version` query param (or the newer v1 unversioned path); adds `content_filter_results`.
- **Cohere v2** — ends the stream with `event: message-end` (no `[DONE]`); splits token counts into `usage.billed_units` vs `usage.tokens`. Bill from `billed_units`.
- **Mistral**, **Groq**, **xAI Grok**, **DeepSeek** — OpenAI-compatible with provider-specific extensions:
  - Groq stashes timing under `x_groq.usage`.
  - Grok exposes `message.reasoning_content`.
  - DeepSeek streams `delta.reasoning_content` and will **400** if the client echoes `reasoning_content` back in the next turn's history.
- **Ollama** — NDJSON with `done: true` terminating; fields `prompt_eval_count` / `eval_count` / `*_duration` (ns).

### The key architectural decision

**Normalize these 11 wire formats into one internal event model while preserving the original bytes for replay.** Store raw bytes verbatim (for compliance, replay, training export), and emit a normalized `UsageEvent { provider, model, input_tokens, output_tokens, cached_in, cached_write, reasoning_tokens, ttft_ms, total_ms }` for cost accounting and OTel export.

---

## Rust crate stack

| Purpose | Crate | Version | Notes |
|---|---|---|---|
| Async runtime | `tokio` | 1.x | Multi-thread; only ecosystem with hyper, rdkafka, aws-sdk-s3 |
| HTTP server | `hyper` + `axum` | 1.x / 0.8 | Ingress + `/admin` + `/metrics` |
| HTTP client | `reqwest` | 0.12 | `default-features = false`, `features = ["rustls-tls","stream","json"]` |
| Low-level proxy | `hyper-rustls` | 0.27 | For upstream client pool |
| TLS | `rustls` + `aws-lc-rs` | 0.23 | Default provider; FIPS-ready, faster than ring |
| Dev-only cert gen | `rcgen` | 0.13 | Self-signed dev mode |
| Zero-copy bytes | `bytes` | 1.x | Refcounted slice — mandatory for the tee pattern |
| Body helpers | `http-body-util` | 0.1 | Stream adapters |
| SSE parsing | `eventsource-stream` | 0.2 | Grammar; per-provider logic on top |
| AWS event-stream | `aws-smithy-eventstream` | 0.60 | Standalone decoder, no full SDK needed |
| JSON fast-path | `sonic-rs` | 0.5 | AVX-512 where available; fallback `serde_json` |
| JSON stable | `serde_json` | 1.x | Config, slow paths |
| Compression | `zstd` | 0.13 | libzstd 1.5.x, dict support |
| Columnar archive | `parquet` (arrow-rs) | 55.x | For cold export only |
| Storage (local) | `rusqlite` | 0.32 | `bundled` feature — SQLite 3.46+ statically linked |
| Analytics (optional) | `duckdb` | 1.x | For ad-hoc queries |
| ClickHouse sink | `clickhouse` | official 0.14 | Native protocol, LZ4 on wire |
| S3 sink | `aws-sdk-s3` | 1.x | Multipart upload |
| Postgres (optional) | `sqlx` | 0.8 | rustls only |
| Kafka (optional) | `rdkafka` | 0.36 | librdkafka FFI |
| Manifest KV | `redb` | 3.x | Pure-Rust ACID B-tree |
| Spool/WAL | `fjall` | 2.x | LSM KV, built-in WAL, LZ4 |
| Tokenizers | `tiktoken-rs` | 0.6 | cl100k_base, o200k_base, o200k_harmony |
| HF tokenizers | `tokenizers` | 0.20 | Llama, Qwen, DeepSeek |
| Observability | `tracing`, `metrics`, `opentelemetry` | 0.1 / 0.24 / 0.27 | Standard stack |
| OTLP export | `opentelemetry-otlp` | 0.27 | `features = ["http-proto","reqwest-client"]` |
| Metrics | `metrics-exporter-prometheus` | 0.16 | `/metrics` endpoint |
| Config | `figment` + `serde` + `toml` | 0.10 / 1 / 0.8 | Layered file + env + CLI |
| CLI | `clap` | 4.5 | Derive |
| Errors | `thiserror` + `anyhow` | 2 / 1 | Libs / top-level |
| Secrets | `secrecy` + `zeroize` | 0.10 / 1 | Wipe on drop |
| Graceful shutdown | `tokio-graceful-shutdown` + `CancellationToken` | current | Subsystem pattern |
| Hot reload | `arc-swap` + `notify` | current / 8 | RCU config swap |
| systemd notify | `sd-notify` | current | `Type=notify` |
| File watcher | `notify` | 8.2 | Cross-platform fs events |
| Hashing | `sha2` + `xxhash-rust` | 0.10 / current | Content hashing |
| Compact strings | `smol_str` | 0.3 | 24 B inline, no alloc under 23 chars |
| Interning | `lasso` | 0.7 | var-dict backing |
| Arenas | `bumpalo` | 3.x | Per-segment scratch |
| IDs | `ulid` | 1.x | Time-sortable |
| Testing | `wiremock` + `insta` + `criterion` | 0.6 / 1 / 0.5 | HTTP mocks, snapshots, benches |
| Property testing | `proptest` | 1.x | For parsers and cost engine |
| Allocator | `tikv-jemallocator` | 0.6 | Only if bench shows a win under musl |

**Dead / avoid list**: `rusoto`, `async-std`, `sled`, `arrow2`/`parquet2`, `okaywal`, `opentelemetry-prometheus`, bare `jemallocator` (use `tikv-jemallocator`), `openssl-sys` (use rustls).

---

## The tee pattern (core proxy mechanic)

In hyper 1.x, bodies are `http_body::Body` streams of `Frame<Bytes>`, not buffered `Vec<u8>`. The correct pattern wraps the incoming body in a `Stream` adapter that forwards each `Bytes` chunk to the upstream request while cloning it (cheap — `Bytes` is refcounted) into a bounded `tokio::sync::mpsc` channel consumed by a background capture task. The same pattern mirrors onto the response body.

### Invariants

- The forwarded body must be a **stream**, never `.collect()`-ed.
- Backpressure propagates: if the capture sink can't keep up, the mpsc fills and the tee starts **dropping capture** (recording `keplor_capture_dropped_total{stage}`) rather than dropping the forwarded byte. **Client correctness is never sacrificed for observability.**
- Preserve HTTP/2 semantics: don't change `content-length` or `transfer-encoding`.
- Strip only hop-by-hop headers: `connection`, `keep-alive`, `proxy-authenticate`, `proxy-authorization`, `te`, `trailer`, `transfer-encoding`, `upgrade`, plus `host` (rewritten). Auth headers (`authorization`, `x-api-key`, `api-key`, `x-goog-api-key`, AWS SigV4 headers) pass through verbatim.

### Header sanitization for storage

When storing the captured request/response for later display, strip all auth headers, cookies, and `set-cookie` before persistence. Whitelist-based, not blacklist, to avoid leaks from new provider headers we haven't seen.

---

## Storage + compression design

### Schema

One fact table, `llm_events`, keyed by ULID. Separate content-addressed `payload_blobs` table for bodies. Separate `zstd_dicts` table for trained dictionaries (referenced by blobs forever; never deleted).

### Component-level deduplication

For each captured payload, split into components before hashing and storing:

- `system_prompt` (single component if present)
- `tools` / tool-schema array (single component per request)
- `messages` (single component — don't dedupe per-message, that's overfitting)
- `response_text` / `response_content`

Each component is sha256-hashed and stored once in `payload_blobs` with a `refcount`. System prompts repeat across thousands of requests — this is where the single biggest compression win lives.

### Trained zstd dictionaries

Background trainer task wakes every 6 hours (or when sample buffer exceeds 4096 samples for a given `(provider, component_type)` key):

1. Reservoir-sample up to 2048 recent blobs.
2. Call `zstd::dict::from_samples` with `target_size = 112_640` bytes (fastcover default).
3. Evaluate on a held-out 256-sample set. Adopt only if size-ratio improves ≥ 8%.
4. Insert into `zstd_dicts`. Swap in memory via `ArcSwap`.
5. Old dicts remain in the table forever — existing blobs reference them by id.

Per-`(provider, component_type)` dicts means: one dict for OpenAI system prompts, another for Anthropic tool schemas, another for Bedrock response JSON, etc. Far better than a single global dict.

### Expected ratios

Target 30–80× versus raw NDJSON on conversational traffic. System-prompt dedup alone typically saves 40–60% of total storage on real workloads. Response bodies dominate what remains and compress 15–25× with zstd-3 + dict.

---

## OTel + OpenInference dual emission

Emit **both** schemas on the same span. Costs ~400 bytes per span. Makes the output ingestable by Langfuse, Phoenix, LangSmith, Datadog, Dynatrace, Honeycomb, Grafana Tempo, and New Relic without reconfiguration.

### OTel GenAI attributes (1.37 draft, stable in practice)

```
gen_ai.system
gen_ai.provider.name
gen_ai.operation.name                   (e.g. "chat", "generate_content")
gen_ai.request.model
gen_ai.response.model
gen_ai.request.{temperature, top_p, max_tokens}
gen_ai.usage.input_tokens               (INCLUDES cached, per spec)
gen_ai.usage.output_tokens
gen_ai.usage.cache_read.input_tokens
gen_ai.usage.cache_creation.input_tokens
gen_ai.response.finish_reasons
gen_ai.response.id
gen_ai.system_instructions              (if content capture enabled)
gen_ai.input.messages                   (if content capture enabled)
gen_ai.output.messages                  (if content capture enabled)
```

Content capture is gated by `observability.capture_messages_in_otlp = false` (default) and the standard env var `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true`.

### OpenInference attributes (same span)

```
openinference.span.kind = "LLM"
llm.model_name
llm.provider
llm.system
llm.token_count.prompt
llm.token_count.completion
llm.token_count.total
llm.token_count.prompt_details.cache_read
llm.token_count.completion_details.reasoning
llm.input_messages.{i}.message.role
llm.input_messages.{i}.message.content
llm.output_messages.{i}.message.role
llm.output_messages.{i}.message.content
input.value, output.value               (raw JSON strings)
```

---

## Binary size recipe (<10 MB static musl)

```toml
# workspace Cargo.toml
[profile.release]
opt-level = "z"        # size over speed
lto = "fat"            # whole-program optimization
codegen-units = 1      # blocks cross-unit optimization otherwise
panic = "abort"        # strips unwinding
strip = "symbols"
debug = false
```

Build:

```
cargo +nightly build \
  -Z build-std=std,panic_abort \
  -Z build-std-features="optimize_for_size" \
  --target x86_64-unknown-linux-musl \
  --release
```

With `RUSTFLAGS="-Zlocation-detail=none -Zfmt-debug=none"`.

Disable default features everywhere:

- `rusqlite = { version = "0.32", default-features = false, features = ["bundled"] }`
- `reqwest = { version = "0.12", default-features = false, features = ["rustls-tls","stream","json"] }` (prevents OpenSSL linkage)
- `tokio = { version = "1", default-features = false, features = ["rt-multi-thread","net","io-util","sync","macros","signal","time","fs"] }`

Prefer `opentelemetry-otlp` with HTTP/protobuf (`features = ["http-proto","reqwest-client"]`) over the gRPC/tonic variant — saves ~1 MB.

Avoid both `tikv-jemallocator` and `mimalloc` unless benchmarking shows a win — system allocator on musl is fine and saves ~400 KB.

Skip UPX unless distributing to edge environments.

---

## Threading model

- One **tokio multi-thread runtime**, sized to physical cores − 1.
- A separate **`rayon` thread pool** sized to physical cores for CPU-bound work: zstd encoding, dictionary training, Parquet export, cost computation batches. Bridge via `tokio::task::spawn_blocking`.
- A dedicated OS thread for SQLite checkpoint / fsync if bench shows contention; otherwise a single `Mutex<Connection>` is fine at 10k req/s.
- Source tasks stay fully async; they only `send().await` on bounded channels.

---

## Backpressure strategy

1. Per-connection semaphore (`tokio::sync::Semaphore`) on the axum listener capped at `server.max_concurrent_requests`.
2. Ingress → upstream: stream-to-stream with hyper; hyper's internal flow control is the backpressure.
3. Tee → capture: bounded mpsc channel; full channel drops **capture** with a metric, never drops the byte on the wire.
4. Capture → store: bounded mpsc; full channel applies backpressure to the capture task (not the proxy).
5. Store → sinks: per-sink bounded queue, with `when_full` policy per sink: `block | drop_newest | spill_to_disk`.

---

## Conclusion

The highest-leverage insight: a proxy is a mechanism, not an opinion. Every feature that mainstream gateways bundle (routing, fallbacks, caching, guardrails, virtual keys, budgets) is a design choice the customer may not share. By keeping Keplor's core a pure observational tee with a great storage and telemetry layer, we earn the right to add those features later as optional modules, and we leave an uncontested niche that no Python, TypeScript, or Go tool currently occupies.
