# CLAUDE.md

This file tells Claude Code how to work on this repository. Read it before every task.

## Project

**Keplor** — a production-grade LLM logs ingestion and cost-accounting server, written in Rust. Single static musl binary under 10 MB, <1 ms ingestion latency p99, 10k req/s per core, <30 MB idle RAM.

Named for Johannes Kepler, who derived planetary laws from observations others recorded. Keplor does the same — it turns the LLM logs your systems send it into precise cost and usage insights.

## Non-negotiable principles

1. **Faithful ingestion** — accept events as-is. Never recompute, reformat, or drop fields the caller sent.
2. **Every provider schema** — accept events from any provider wire format with correct token-type handling for each.
3. **Heavy compression** — zstd with trained dictionaries per (provider, component_type). Target 30–80× on conversational JSON.
4. **Zero-dep default** — SQLite works out of the box. ClickHouse, S3, OTLP are optional sinks.
5. **Lean stack** — hyper 1.x + axum 0.8 + rustls 0.23 (aws-lc-rs) + tokio. NOT pingora, NOT openssl, NOT async-std.

## Providers (priority order)

OpenAI (Chat Completions + Responses), Anthropic Messages, Google Gemini (generateContent + streamGenerateContent with ?alt=sse), AWS Bedrock (Converse/ConverseStream + InvokeModel with AWS event-stream binary framing), Azure OpenAI, Mistral, Groq, xAI Grok, DeepSeek, Cohere v2, Ollama, and OpenAI-compatible fallthrough.

## Primary feature

Cost/usage accounting per (user, api_key, model, route, org, project). Pricing catalog = LiteLLM's `model_prices_and_context_window.json`, auto-refreshed daily, version-pinned fallback bundled in the binary. Cost stored as int64 nanodollars.

## Secondary feature

Full prompts/completions observability — request/response bodies compressed and stored, latency TTFT + TTLT, normalized error types.

## Future (M4+)

PII masking, retention policies, feedback API, training-dataset export. Don't build these now but design schemas to accommodate them.

## Telemetry output

Dual-emit both OpenTelemetry GenAI semantic conventions (`gen_ai.*`) and OpenInference attributes (`llm.*`, `openinference.span.kind`) on the same span for maximum downstream-tool compatibility (Langfuse, Phoenix, LangSmith, Datadog, Honeycomb, Grafana).

## Code quality bar

- `cargo fmt` and `cargo clippy -- -D warnings` must pass on every commit.
- No `.unwrap()` or `.expect()` on runtime paths. Use `thiserror` error types in libraries, `anyhow` only at the top level (`main.rs`, xtasks).
- Every async boundary is bounded: explicit channel capacities, buffer byte caps, semaphore limits — all documented in config.
- Every public item has a doc comment. Every module has a `//!` header.
- Unit tests for pure logic. Integration tests with `wiremock` for provider adapters. Criterion benchmarks for hot paths.
- Fixtures for every SSE/event-stream format in `tests/fixtures/<provider>/`.

## Workflow rules

1. Read `docs/architecture.md` and the relevant phase section in `docs/phases/` before writing code.
2. Plan the file/module layout first. Show the plan before implementing.
3. Work phase-by-phase. Don't skip ahead.
4. At the end of each phase:
   - Run `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`.
   - Report a short status: what's done, what's tested, what's deferred.
   - Update `docs/progress.md` with the phase retrospective.
5. Commit message format: `phase-N: <component>: <what changed>`.
6. Never commit secrets. Never bypass `.gitignore`. Never disable clippy lints without a `// reason:` comment.

## File ownership

Claude owns: `crates/**`, `tests/**`, `benches/**`, `xtask/**`, `docs/phases/**`, `docs/progress.md`.

Claude does NOT modify without asking: `CLAUDE.md`, `docs/architecture.md`, `docs/providers.md`, `Cargo.toml` workspace-level deps (can add to a crate, but workspace-level dep additions need confirmation), `.github/workflows/**`, `LICENSE`, `README.md`.

## Acknowledgement

At the start of every session, acknowledge you've read this file, then proceed to the current phase.
