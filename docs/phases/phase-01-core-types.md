# Phase 1 — Core domain types

**Status:** not started
**Depends on:** phase 0
**Unlocks:** phases 2, 3, 4, 5

## Goal

Implement the typed backbone every other crate depends on. No I/O in this crate. No async. Pure data + logic.

## Prompt

Implement `keplor-core`. Modules to create in `crates/keplor-core/src/`:

1. **`lib.rs`** — re-exports, crate docs.

2. **`id.rs`** — newtype IDs: `EventId(Ulid)`, `UserId`, `ApiKeyId`, `OrgId`, `ProjectId`, `RouteId`, `ProviderId`. All `Copy` where possible. `Display` + `FromStr` impls.

3. **`provider.rs`** — `Provider` enum:
   ```
   OpenAI, Anthropic, Gemini, GeminiVertex, Bedrock, AzureOpenAI,
   Mistral, Groq, XAi, DeepSeek, Cohere, Ollama,
   OpenAICompatible { base_url: Arc<str> }
   ```
   Methods:
   - `fn canonical_host(&self) -> &str`
   - `fn from_host_path(host: &str, path: &str) -> Option<Provider>`
   - `fn auth_header_name(&self) -> &str`

4. **`event.rs`** — the canonical event record:
   ```rust
   pub struct LlmEvent {
       pub id: EventId,
       pub ts_ns: i64,
       pub user_id: Option<UserId>,
       pub api_key_id: Option<ApiKeyId>,
       pub org_id: Option<OrgId>,
       pub project_id: Option<ProjectId>,
       pub route_id: RouteId,
       pub provider: Provider,
       pub model: SmolStr,
       pub model_family: Option<SmolStr>,
       pub endpoint: SmolStr,
       pub method: http::Method,
       pub http_status: Option<u16>,
       pub usage: Usage,
       pub cost_nanodollars: i64,
       pub latency: Latencies,          // ttft_ms, total_ms, time_to_close_ms
       pub flags: EventFlags,           // streaming, tool_calls, reasoning, stream_incomplete
       pub error: Option<ProviderError>,
       pub request_ref: PayloadRef,
       pub response_ref: PayloadRef,
       pub request_sha256: [u8; 32],
       pub response_sha256: [u8; 32],
       pub client_ip: Option<IpAddr>,
       pub user_agent: Option<SmolStr>,
       pub request_id: Option<SmolStr>,
       pub trace_id: Option<TraceId>,
   }
   ```

5. **`usage.rs`** — `Usage` struct with every token dimension from the research:
   ```rust
   pub struct Usage {
       pub input_tokens: u32,
       pub output_tokens: u32,
       pub cache_read_input_tokens: u32,
       pub cache_creation_input_tokens: u32,
       pub reasoning_tokens: u32,
       pub audio_input_tokens: u32,
       pub audio_output_tokens: u32,
       pub image_tokens: u32,
       pub video_seconds: u32,
       pub tool_use_tokens: u32,
       pub search_queries: u32,
   }
   ```
   All `Default = 0`, saturating_add helpers. Include a `merge(other: &Usage)` for combining stream deltas. Include a `total_billable_input_tokens(provider: Provider) -> u32` that correctly reflects each provider's semantics — Anthropic: `input_tokens + cache_creation + cache_read`; OpenAI: `input_tokens` already includes cached; Gemini: host-dependent.

6. **`cost.rs`** — `Cost` newtype over `i64` nanodollars. `Display = "$0.00000000"`. Arithmetic ops. `fn to_dollars_f64() -> f64` for UI only.

7. **`error.rs`** — `thiserror` enums:
   ```rust
   pub enum CoreError { ... }

   pub enum ProviderError {
       RateLimited { retry_after: Option<Duration> },
       InvalidRequest(String),
       AuthFailed,
       ContextLengthExceeded { limit: u32 },
       ContentFiltered { reason: SmolStr },
       UpstreamTimeout,
       UpstreamUnavailable,
       Other { status: u16, message: SmolStr },
   }
   ```
   With a `from_provider_response(provider: Provider, status: u16, body: &[u8]) -> ProviderError` normalizer.

8. **`payload_ref.rs`** — how we point at stored request/response bodies:
   ```rust
   pub struct PayloadRef {
       pub sha256: [u8; 32],
       pub storage: PayloadStorage,        // Inline(Bytes) | Blob(BlobId) | External(Url)
       pub compression: Compression,       // None | ZstdRaw | ZstdDict(DictId)
       pub uncompressed_size: u32,
       pub compressed_size: u32,
   }
   ```

9. **`flags.rs`** — `bitflags!` `EventFlags` covering streaming / tool_calls / reasoning / stream_incomplete / cached_used / budget_blocked.

10. **`sanitize.rs`** — header-sanitization: a function that takes a `http::HeaderMap` and returns a cloned `HeaderMap` with `authorization`, `x-api-key`, `api-key`, `x-goog-api-key`, `aws-*` (SigV4 headers), cookies, and `set-cookie` stripped. Whitelist-based preferred over blacklist.

## Acceptance criteria

- [ ] All modules with rustdoc on every public item
- [ ] Unit tests for:
  - `Usage::merge` (streaming delta accumulation)
  - `Usage::total_billable_input_tokens` (provider-specific semantics, with fixtures)
  - `ProviderError::from_provider_response` (fixture bodies from every provider)
  - `Provider::from_host_path` (host matching battery)
  - `sanitize_headers` (battery of real header names)
- [ ] `cargo test -p keplor-core` green
- [ ] No other crate touched
- [ ] `cargo clippy -p keplor-core -- -D warnings` green

Show me the module-by-module file list before you start.
