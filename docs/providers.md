# Provider reference

Quick-reference card for every provider Keplor supports. For full wire-format details see [architecture.md](architecture.md). For implementation, see `crates/keplor-providers/src/<provider>.rs`.

Legend: ✅ supported · 🟡 partial · ⬜ planned

| Provider | Streaming | Tools | Reasoning | Caching | Vision | Audio | Cost |
|---|---|---|---|---|---|---|---|
| OpenAI Chat Completions | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| OpenAI Responses API | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| Anthropic Messages | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | — | ⬜ |
| Google Gemini (AI Studio) | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| Google Gemini (Vertex) | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| AWS Bedrock Converse | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | — | ⬜ |
| AWS Bedrock InvokeModel | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | — | ⬜ |
| Azure OpenAI | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |
| Mistral | ⬜ | ⬜ | — | — | ⬜ | — | ⬜ |
| Groq | ⬜ | ⬜ | — | — | — | — | ⬜ |
| xAI Grok | ⬜ | ⬜ | ⬜ | — | ⬜ | — | ⬜ |
| DeepSeek | ⬜ | ⬜ | ⬜ | ⬜ | — | — | ⬜ |
| Cohere v2 | ⬜ | ⬜ | — | — | — | — | ⬜ |
| Ollama | ⬜ | ⬜ | — | — | ⬜ | — | — (local) |
| OpenAI-compatible fallthrough | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ | ⬜ |

## Key wire-format gotchas (one-liners)

- **OpenAI Chat Completions**: `data: [DONE]` sentinel is not JSON — byte-match first.
- **OpenAI Responses API**: no `[DONE]`; terminate on `response.completed`. Usage fields renamed (`input_tokens` not `prompt_tokens`).
- **Anthropic**: `message_delta.usage` is *cumulative totals*, not a delta. `input_tokens` excludes cached; add `cache_creation + cache_read` for true prompt size. Preserve thinking-block `signature` byte-exact.
- **Gemini default streaming**: progressive JSON array, not SSE. Use `?alt=sse` for SSE. Vertex vs AI Studio differ on whether `candidatesTokenCount` includes thoughts.
- **Bedrock**: AWS event-stream binary framing with 12-byte prelude + headers + payload + CRC32. `InvokeModelWithResponseStream` double-wraps as base64 in `{"chunk": {"bytes": "..."}}`.
- **Azure OpenAI**: `api-key` header + `api-version` query param. Adds `content_filter_results`.
- **Cohere v2**: ends with `event: message-end`. Bill from `usage.billed_units`, not `usage.tokens`.
- **Groq**: timing in `x_groq.usage`. Streaming usage sometimes missing.
- **Grok / DeepSeek**: `reasoning_content` in deltas. DeepSeek rejects echoed reasoning in next turn.
- **Ollama**: NDJSON, `done: true` terminates. Durations in nanoseconds.
