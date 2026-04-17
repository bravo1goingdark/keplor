//! Dual-emit span builder: every Keplor span carries both the
//! OpenTelemetry GenAI (`gen_ai.*`) and OpenInference (`llm.*`) attribute
//! families so Langfuse, Phoenix, LangSmith, Datadog, Honeycomb, and
//! Grafana Tempo all ingest without reconfiguration.
