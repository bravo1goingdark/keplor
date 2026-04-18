//! Criterion benchmarks for the keplor-server ingestion pipeline.
//!
//! Benchmarks the full pipeline path: JSON → validate → normalize → cost
//! lookup → SHA-256 → build event → queue for batch write.

#![allow(clippy::unwrap_used)]

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use keplor_pricing::Catalog;
use keplor_server::Pipeline;
use keplor_store::{BatchConfig, BatchWriter, Store};

// ── Helpers ────────────────────────────────────────────────────────────

/// Pipeline + tokio runtime handle.  BatchWriter spawns a background
/// flush task, so a runtime must be alive for the entire benchmark.
struct BenchEnv {
    pipeline: Pipeline,
    _rt: tokio::runtime::Runtime,
}

fn setup_pipeline() -> BenchEnv {
    let rt =
        tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let _guard = rt.enter();
    let store = Arc::new(Store::open_in_memory().unwrap());
    // Large channel so the benchmark loop doesn't hit back-pressure.
    let config = BatchConfig { channel_capacity: 512_000, ..BatchConfig::default() };
    let writer = Arc::new(BatchWriter::new(Arc::clone(&store), config));
    let catalog = Arc::new(Catalog::load_bundled().unwrap());
    BenchEnv { pipeline: Pipeline::new(store, writer, catalog), _rt: rt }
}

/// Smallest valid event — no bodies, no optional fields.
fn minimal_json() -> &'static str {
    r#"{"model":"gpt-4o","provider":"openai"}"#
}

/// Realistic event with usage, bodies (~2 KB total).
fn with_bodies_json() -> &'static str {
    r#"{
        "model":"gpt-4o",
        "provider":"openai",
        "usage":{"input_tokens":150,"output_tokens":80},
        "latency":{"ttft_ms":22,"total_ms":245},
        "http_status":200,
        "source":"litellm",
        "user_id":"user_42",
        "endpoint":"/v1/chat/completions",
        "flags":{"streaming":true},
        "request_body":{
            "model":"gpt-4o",
            "messages":[
                {"role":"system","content":"You are a helpful assistant that answers questions concisely and accurately."},
                {"role":"user","content":"Explain the concept of gravitational lensing in astrophysics and how it has been used to detect dark matter in galaxy clusters."}
            ],
            "stream":true,"max_tokens":512
        },
        "response_body":{
            "id":"chatcmpl-bench001","object":"chat.completion","model":"gpt-4o-2024-08-06",
            "choices":[{"index":0,"message":{"role":"assistant","content":"Gravitational lensing is a phenomenon predicted by Einstein's general theory of relativity, where the path of light is bent as it passes near a massive object."},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":150,"completion_tokens":80,"total_tokens":230}
        }
    }"#
}

/// Full event with all optional fields, metadata, ~5 KB total.
fn full_event_json() -> &'static str {
    r#"{
        "model":"claude-sonnet-4-20250514",
        "provider":"anthropic",
        "usage":{
            "input_tokens":1200,"output_tokens":600,
            "cache_read_input_tokens":200,"cache_creation_input_tokens":50,
            "reasoning_tokens":100
        },
        "latency":{"ttft_ms":35,"total_ms":1200,"time_to_close_ms":5},
        "timestamp":1700000000000000000,
        "http_status":200,
        "source":"my-gateway",
        "user_id":"user_99",
        "api_key_id":"key_prod_01",
        "org_id":"acme-corp",
        "project_id":"proj_chat",
        "route_id":"customer-support",
        "endpoint":"/v1/messages",
        "flags":{"streaming":true,"reasoning":true,"cache_used":true},
        "trace_id":"4bf92f3577b34da6a3ce929d0e0e4736",
        "request_id":"req_01xyz",
        "user_agent":"my-app/1.2.3",
        "request_body":{
            "model":"claude-sonnet-4-20250514","max_tokens":1024,
            "system":"You are a customer support assistant for Acme Corp. Always be polite, reference the knowledge base articles, and escalate complex issues to human agents.",
            "messages":[
                {"role":"user","content":"I've been having trouble with my order #12345. It was supposed to arrive last week but I still haven't received it. Can you help me track it down? I've already checked the tracking number and it says 'in transit' for the past 5 days."},
                {"role":"assistant","content":"I'd be happy to help you track down your order. Let me look into this right away."},
                {"role":"user","content":"Thanks, I really need this resolved because it's a gift for my daughter's birthday which is this weekend."}
            ]
        },
        "response_body":{
            "id":"msg_01abc","type":"message","role":"assistant",
            "content":[{"type":"text","text":"I completely understand the urgency, and I want to make sure your daughter's gift arrives in time. Let me check the detailed shipping status for order #12345. Based on the extended 'in transit' status, I'm going to escalate this with our shipping partner right away."}],
            "model":"claude-sonnet-4-20250514",
            "usage":{"input_tokens":1200,"output_tokens":600}
        },
        "metadata":{"session_id":"sess_abc123","retry_count":0,"priority":"high"}
    }"#
}

// ── Benchmarks ─────────────────────────────────────────────────────────

fn bench_ingest_fire_and_forget(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline_ingest");

    let cases: &[(&str, &str)] = &[
        ("minimal", minimal_json()),
        ("with_bodies", with_bodies_json()),
        ("full_event", full_event_json()),
    ];

    for (name, json) in cases {
        // Measure the byte throughput of the JSON payload.
        group.throughput(Throughput::Bytes(json.len() as u64));

        group.bench_with_input(BenchmarkId::new("fire_and_forget", name), json, |b, json| {
            let env = setup_pipeline();
            b.iter(|| {
                let event = serde_json::from_str(json).unwrap();
                // ChannelFull is expected under sustained bench pressure — it
                // means pipeline processing completed but the channel is
                // saturated.  We still measure the processing time.
                let _ = black_box(env.pipeline.ingest_fire_and_forget(event, None, "free"));
            });
        });
    }

    group.finish();
}

fn bench_ingest_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline_throughput");

    for batch_size in [32, 128, 512] {
        group.throughput(Throughput::Elements(batch_size as u64));

        group.bench_with_input(
            BenchmarkId::new("fire_and_forget_batch", batch_size),
            &batch_size,
            |b, &batch_size| {
                let env = setup_pipeline();
                let json = with_bodies_json();
                b.iter(|| {
                    for _ in 0..batch_size {
                        let event = serde_json::from_str(json).unwrap();
                        let _ = black_box(env.pipeline.ingest_fire_and_forget(event, None, "free"));
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_deser_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline_deser");

    let cases: &[(&str, &str)] = &[
        ("minimal", minimal_json()),
        ("with_bodies", with_bodies_json()),
        ("full_event", full_event_json()),
    ];

    for (name, json) in cases {
        group.throughput(Throughput::Bytes(json.len() as u64));

        group.bench_with_input(BenchmarkId::new("from_str", name), json, |b, json| {
            b.iter(|| {
                let event: keplor_server::schema::IngestEvent =
                    serde_json::from_str(black_box(json)).unwrap();
                black_box(event);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_ingest_fire_and_forget, bench_ingest_throughput, bench_deser_only);
criterion_main!(benches);
