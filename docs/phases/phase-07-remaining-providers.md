# Phase 7 — Remaining providers

**Status:** not started
**Depends on:** phases 5, 6
**Unlocks:** phase 9 (multi-provider testing)

## Goal

Add the remaining nine providers. Same `ProviderAdapter` + `StreamReassembler` contract as phase 5. Each adapter is its own module with its own fixture battery.

## Prompt

Implement in this order (simplest first, most complex last):

### 1. DeepSeek (`deepseek.rs`)

- OpenAI-compatible.
- Extension: `delta.reasoning_content` in streaming, final `message.reasoning_content` non-streaming.
- **IMPORTANT**: on subsequent turns do NOT forward `reasoning_content` back in `messages[]` — or rather, we don't rewrite; we just DOCUMENT that clients must strip it. Our adapter flags the `reasoning_content` presence in `provider_meta`.

### 2. Mistral (`mistral.rs`)

OpenAI-compatible, no surprises.

### 3. Groq (`groq.rs`)

OpenAI-compatible. Timing in `x_groq.usage`. Stream usage sometimes missing — fallback to `tiktoken-rs` if model is GPT-like, or Llama tokenizer for Llama/Mixtral.

### 4. xAI Grok (`xai.rs`)

OpenAI-compatible. `message.reasoning_content` similar to DeepSeek. Usage includes `prompt_tokens_details.text_tokens` and `completion_tokens_details.reasoning_tokens`.

### 5. Azure OpenAI (`azure.rs`)

OpenAI-compatible path (`/openai/deployments/{deployment}/chat/completions?api-version=...`) + `api-key` header. Add `content_filter_results` preservation in `provider_meta`. Handle both legacy versioned and new v1 unversioned paths.

### 6. Cohere v2 (`cohere.rs`)

Native format. Streaming ends with `event: message-end` (no `[DONE]`). Usage split: `usage.billed_units` (what you pay for) vs `usage.tokens` (wire size). Use `billed_units` for cost.

### 7. Ollama (`ollama.rs`)

NDJSON streaming. `done: true` terminates. Usage: `prompt_eval_count`, `eval_count`. Timing in nanoseconds: `prompt_eval_duration`, `eval_duration`, `load_duration`, `total_duration`. Cost = 0 (local) unless a pricing override is set.

### 8. Gemini (`gemini.rs`) — tricky

- Two endpoints: `generateContent` (non-streaming), `streamGenerateContent` (streaming).
- Default streaming format: **progressive JSON array, not SSE**. `Content-Type: application/json`. Elements separated by `\n,\n` after the opening `[`. Must parse as a stateful streaming JSON-array decoder.
- With `?alt=sse`: standard SSE `data: {...}\n\n`, no `[DONE]` — terminate on `usageMetadata` appearing in a chunk or on stream EOF.
- Handle both Vertex and AI Studio surfaces:
  - `*.googleapis.com/v1beta/models/{model}:generateContent`
  - `/v1/projects/*/locations/*/publishers/google/models/{model}:streamGenerateContent`
- `usageMetadata`: `promptTokenCount`, `candidatesTokenCount`, `totalTokenCount`, `cachedContentTokenCount`, `thoughtsTokenCount`, `toolUsePromptTokenCount`.
- **NORMALIZATION QUIRK**: on Vertex, `candidatesTokenCount` **EXCLUDES** thoughts (`totalTokenCount = prompt + candidates + thoughts + toolUse`). On AI Studio, `candidates` **INCLUDES** thoughts. Detect by host and adjust. Write a DEFCON-2 doc comment.

### 9. Bedrock (`bedrock.rs`) — hardest

- **AWS SigV4 passthrough** — don't re-sign, the client already signed; we just forward.
- Two endpoints:
  - `POST /model/{modelId}/converse` and `/converse-stream` (Converse API)
  - `POST /model/{modelId}/invoke` and `/invoke-with-response-stream` (InvokeModel API)
- **Binary wire format**: AWS event-stream. Use `aws-smithy-eventstream` for decoding (12-byte prelude + headers + payload + CRC32). Event types (Converse): `messageStart`, `contentBlockStart`, `contentBlockDelta` (subtypes: `text` / `toolUse` / `reasoningContent` / `redactedReasoningContent`), `contentBlockStop`, `messageStop`, `metadata`.
- Usage in `metadata` frame: `inputTokens`, `outputTokens`, `cacheReadInputTokens`, `cacheWriteInputTokens`.
- For `InvokeModelWithResponseStream`, chunks are `{"chunk": {"bytes": "<base64>"}}` — decode and re-feed to the appropriate model-native reassembler. Anthropic-on-Bedrock reuses the Anthropic adapter's reassembler logic. Factor common code.

### Common work across all nine

- Fixture SSE/binary files in `tests/fixtures/<provider>/`.
- Golden-file integration tests.
- Pricing-catalog key mapping in `keplor-pricing` (extend with any new model keys).
- Route registration examples in example configs.

## Acceptance criteria

- [ ] All nine providers implemented and tested
- [ ] `cargo test -p keplor-providers` green
- [ ] Provider matrix table in `docs/progress.md` green/red for: streaming-works, non-streaming-works, tools-work, reasoning-tracked, caching-tracked, vision-tracked, audio-tracked, cost-accurate
- [ ] Update `docs/providers.md` support matrix (replace ⬜ with ✅ / 🟡)
- [ ] `cargo clippy -p keplor-providers -- -D warnings` green
