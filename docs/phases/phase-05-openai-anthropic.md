# Phase 5 — OpenAI + Anthropic adapters (the MVP pair)

**Status:** not started
**Depends on:** phases 1, 4
**Unlocks:** phase 6

## Goal

First two providers end-to-end. Both providers must work for: non-streaming chat, streaming chat, tool calling, reasoning (o-series, Claude thinking), and usage extraction.

## Prompt

Implement `keplor-providers` with `ProviderAdapter` trait + OpenAI + Anthropic.

### 1. Trait (`keplor-providers/src/lib.rs`)

```rust
pub trait ProviderAdapter: Send + Sync + 'static {
    fn id(&self) -> Provider;
    fn matches(&self, host: &str, path: &str) -> bool;
    fn parse_request(&self, body: &[u8], headers: &HeaderMap) -> Result<ParsedRequest>;
    fn make_reassembler(&self, req: &ParsedRequest) -> Box<dyn StreamReassembler>;
}

pub trait StreamReassembler: Send {
    fn feed(&mut self, chunk: &[u8]) -> Result<()>;
    fn finish(self: Box<Self>) -> ReassembledResponse;  // works even on partial streams
}

pub struct ReassembledResponse {
    pub final_text: String,
    pub tool_calls: Vec<ToolCall>,
    pub reasoning: Option<String>,
    pub usage: Usage,
    pub finish_reason: Option<SmolStr>,
    pub stream_incomplete: bool,
    pub provider_meta: serde_json::Value,   // anything provider-specific
}
```

### 2. OpenAI adapter (`openai.rs`)

- Matches `api.openai.com`, any `*.openai.azure.com`, and configured OpenAI-compatible base URLs.
- Request parser handles both Chat Completions (`/v1/chat/completions`) and Responses API (`/v1/responses`); detect which by path.
- **Streaming reassembler**:
  - *Chat Completions*: SSE `data: {...}\n\n` frames, terminate on `data: [DONE]` (NOT valid JSON — byte-match first). Accumulate `choices[0].delta.content`, `tool_calls[i].function.arguments` fragments by index. Capture trailing `usage` chunk when present (client set `stream_options.include_usage=true`).
  - *Responses API*: named events (`response.output_text.delta`, `response.completed`, `response.reasoning_summary_text.delta`, etc.) with `type` + `sequence_number`. Terminate on `response.completed`. Map to `ReassembledResponse`. Preserve reasoning `encrypted_content` blob byte-exact in `provider_meta`.
- **Usage mapping**:
  - *Chat Completions*: `usage.prompt_tokens` → `input_tokens`; `usage.completion_tokens` → `output_tokens`; `usage.prompt_tokens_details.cached_tokens` → `cache_read_input_tokens`; `usage.completion_tokens_details.reasoning_tokens` → `reasoning_tokens`.
  - *Responses API*: `usage.input_tokens`, `usage.output_tokens`, `usage.input_tokens_details.cached_tokens`, `usage.output_tokens_details.reasoning_tokens`.
- Non-streaming: parse response JSON, done.

### 3. Anthropic adapter (`anthropic.rs`)

- Matches `api.anthropic.com/v1/messages`, `api.anthropic.com/v1/messages/count_tokens`.
- Streaming reassembler state machine for all event types: `message_start`, `content_block_start`, `content_block_delta` (subtypes: `text_delta`, `input_json_delta`, `thinking_delta`, `signature_delta`, `citations_delta`), `content_block_stop`, `message_delta`, `message_stop`, `ping`, `error`.
- **CRITICAL USAGE SEMANTICS** (document in comments, test explicitly):
  - `message_start.message.usage.input_tokens` is **EXCLUSIVE** of cached; real prompt size = `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`.
  - `message_delta.usage` fields are **CUMULATIVE TOTALS** not deltas — overwrite, don't add. `output_tokens` in `message_delta` is authoritative.
  - For the `Usage` struct (which normalizes to OTel semantics where `input_tokens` should INCLUDE cached), set `Usage.input_tokens = message_start input + cache_creation + cache_read`. Store the raw breakdown in `provider_meta`.
- Preserve thinking-block `signature` byte-exact.
- Tool use: `input_json_delta` fragments accumulate per content_block index into a single JSON string, then parsed at `content_block_stop`.

### 4. Request parser extractions (for cost/display)

- `model` (required)
- `stream` (bool, default false)
- `max_tokens` / `max_completion_tokens` / `max_output_tokens`
- `tools` presence flag
- `tool_choice`
- `metadata.user_id` (OpenAI) / `metadata.user_id` (Anthropic) — a per-request user hint that populates `LlmEvent.user_id` if config permits.

### 5. Testing battery

Put fixtures in `tests/fixtures/`:

```
openai/
  chat_stream_basic.sse
  chat_stream_tools.sse
  chat_stream_usage.sse
  responses_reasoning.sse
  responses_encrypted_reasoning.sse
anthropic/
  messages_stream_basic.sse
  messages_stream_tools.sse
  messages_stream_thinking.sse
  messages_stream_cached.sse
  messages_stream_interrupted.sse      (truncated mid-block)
```

For each fixture:
- Feed byte-by-byte in 1/17/all-at-once chunk sizes
- Assert `ReassembledResponse` fields match golden values
- Assert usage numbers match provider documentation

### 6. Integration test

End-to-end with wiremock: client → keplor-proxy (with OpenAIAdapter registered) → wiremock pretending to be api.openai.com. Assert:
- client bytes = wiremock bytes
- CaptureSink yields an `LlmEvent` with correct usage + cost (via `keplor-pricing`)
- `request_body` and `response_body` roundtrip through `keplor-store` with sha256 verification

## Acceptance criteria

- [ ] `cargo test -p keplor-providers` green
- [ ] `cargo test --test integration_openai_anthropic` green
- [ ] Test count reported in `docs/progress.md`
- [ ] Pass rate reported
- [ ] Compression ratios on the fixture set reported
- [ ] Binary size of the current `keplor` binary reported
- [ ] `cargo clippy -p keplor-providers -- -D warnings` green
