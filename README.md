# Keplor

> Observe every LLM request. Know exactly what it cost.

**Status:** pre-alpha — under active development.

Keplor is a transparent, observational HTTPS proxy for LLM traffic. It sits between your application and any LLM provider (OpenAI, Anthropic, Gemini, Bedrock, Azure, Mistral, Groq, xAI, DeepSeek, Cohere, Ollama, and any OpenAI-compatible endpoint), captures every request and response byte-for-byte, reassembles streaming responses losslessly, extracts precise token usage and cost, and stores it all in a heavily compressed local database — with optional fan-out to ClickHouse, S3, Postgres, or any OTLP-compatible backend.

Named for Johannes Kepler, who derived the laws of planetary motion by watching what was already there. Keplor does the same for your LLM traffic.

## What makes it different

- **Single static binary** under 10 MB. No Postgres. No ClickHouse. No Kafka. No Redis. SQLite works out of the box.
- **Pure observational proxy** — byte-for-byte passthrough, zero added latency, no routing opinions, no fallback logic, no request rewriting.
- **Lossless streaming capture** across every provider's wire format: OpenAI SSE, Anthropic named events, Gemini progressive JSON, AWS Bedrock event-stream binary framing, and the rest.
- **Heavy compression** via zstd with trained dictionaries per provider and component type — 30–80× ratios on real conversational traffic.
- **Precise cost accounting** using the industry-standard LiteLLM pricing catalog, with correct handling of prompt caching, reasoning tokens, batch discounts, modality rates, tier pricing, and geo multipliers.
- **Dual-schema telemetry** — every span carries both OpenTelemetry GenAI and OpenInference attributes, so Langfuse, Phoenix, LangSmith, Datadog, Honeycomb, and Grafana Tempo all ingest cleanly without reconfiguration.

## Quickstart

```bash
# Coming soon
docker run -p 8443:8443 -v keplor-data:/var/lib/keplor ghcr.io/you/keplor:latest
```

Point your LLM SDK at `https://localhost:8443` (base URL override), make a few requests, then:

```bash
keplor stats --last 24h
```

## Architecture

See [docs/architecture.md](docs/architecture.md).

## Development roadmap

See [docs/phases/](docs/phases/) for the 12-phase build plan.

## License

Apache-2.0.
