//! Criterion benchmarks for keplor-store hot paths.

#![allow(clippy::unwrap_used)]

use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use keplor_core::*;
use keplor_store::Store;
use sha2::{Digest, Sha256};
use smol_str::SmolStr;

// ── Helpers ────────────────────────────────────────────────────────────

fn make_event(i: usize) -> LlmEvent {
    LlmEvent {
        id: EventId::new(),
        ts_ns: 1_700_000_000_000_000_000 + i as i64,
        user_id: Some(UserId::from("user_1")),
        api_key_id: Some(ApiKeyId::from("key_1")),
        org_id: None,
        project_id: None,
        route_id: RouteId::from("chat"),
        provider: Provider::OpenAI,
        model: SmolStr::new("gpt-4o"),
        model_family: Some(SmolStr::new("gpt-4")),
        endpoint: SmolStr::new("/v1/chat/completions"),
        method: http::Method::POST,
        http_status: Some(200),
        usage: Usage { input_tokens: 100, output_tokens: 50, ..Usage::default() },
        cost_nanodollars: 750_000,
        latency: Latencies { ttft_ms: Some(25), total_ms: 300, time_to_close_ms: None },
        flags: EventFlags::STREAMING,
        error: None,
        request_sha256: [0u8; 32],
        response_sha256: [0u8; 32],
        client_ip: None,
        user_agent: None,
        request_id: None,
        trace_id: None,
        source: None,
        ingested_at: 0,
    }
}

fn system_prompt() -> String {
    "You are a helpful assistant that answers questions concisely and accurately. \
     Follow these guidelines carefully when responding to the user. Always provide \
     structured, well-organized responses with clear headings and bullet points \
     where appropriate. Include relevant examples and code snippets when discussing \
     technical topics."
        .to_string()
}

fn make_request(i: usize) -> Bytes {
    let sys = system_prompt();
    let msg = format!("Question {i}: help with topic {t}", t = i % 7);
    Bytes::from(format!(
        r#"{{"model":"gpt-4o","messages":[{{"role":"system","content":{sys_json}}},{{"role":"user","content":"{msg}"}}]}}"#,
        sys_json = serde_json::to_string(sys.as_str()).unwrap(),
    ))
}

fn make_response(i: usize) -> Bytes {
    Bytes::from(format!(
        r#"{{"id":"chatcmpl-{i:06}","object":"chat.completion","choices":[{{"index":0,"message":{{"role":"assistant","content":"Here is my response to your question number {i}."}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":100,"completion_tokens":50,"total_tokens":150}}}}"#
    ))
}

fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

// ── Benchmarks ─────────────────────────────────────────────────────────

fn bench_sha256(c: &mut Criterion) {
    let mut group = c.benchmark_group("sha256");
    for size in [256, 1024, 4096, 16384] {
        let data = vec![0xABu8; size];
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &data, |b, data| {
            b.iter(|| black_box(sha256_bytes(data)));
        });
    }
    group.finish();
}

fn bench_zstd_compress(c: &mut Criterion) {
    let mut group = c.benchmark_group("zstd_compress");
    let req = make_request(0);
    let resp = make_response(0);

    group.throughput(Throughput::Bytes(req.len() as u64));
    group.bench_function("request_body", |b| {
        b.iter(|| black_box(zstd::bulk::compress(&req, 3).unwrap()));
    });

    group.throughput(Throughput::Bytes(resp.len() as u64));
    group.bench_function("response_body", |b| {
        b.iter(|| black_box(zstd::bulk::compress(&resp, 3).unwrap()));
    });
    group.finish();
}

fn bench_split_request(c: &mut Criterion) {
    use keplor_store::components::split_request;
    let body = make_request(42);

    c.bench_function("split_request_openai", |b| {
        b.iter(|| black_box(split_request(&Provider::OpenAI, &body)));
    });
}

fn bench_append_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("append_batch");
    for batch_size in [32, 64, 128, 256] {
        let events: Vec<(LlmEvent, Bytes, Bytes)> =
            (0..batch_size).map(|i| (make_event(i), make_request(i), make_response(i))).collect();

        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(batch_size), &events, |b, events| {
            b.iter_with_setup(
                || Store::open_in_memory().unwrap(),
                |store| {
                    black_box(store.append_batch(events).unwrap());
                },
            );
        });
    }
    group.finish();
}

fn bench_append_event(c: &mut Criterion) {
    let event = make_event(0);
    let req = make_request(0);
    let resp = make_response(0);

    c.bench_function("append_event_single", |b| {
        b.iter_with_setup(
            || Store::open_in_memory().unwrap(),
            |store| {
                black_box(store.append_event(&event, &req, &resp).unwrap());
            },
        );
    });
}

fn bench_query(c: &mut Criterion) {
    let store = Store::open_in_memory().unwrap();
    // Seed 1000 events.
    let events: Vec<(LlmEvent, Bytes, Bytes)> =
        (0..1000).map(|i| (make_event(i), make_request(i), make_response(i))).collect();
    store.append_batch(&events).unwrap();

    let filter_none = keplor_store::EventFilter::default();
    let filter_user =
        keplor_store::EventFilter { user_id: Some(SmolStr::new("user_1")), ..Default::default() };

    let mut group = c.benchmark_group("query");
    group.bench_function("no_filter_limit50", |b| {
        b.iter(|| black_box(store.query(&filter_none, 50, None).unwrap()));
    });
    group.bench_function("user_filter_limit50", |b| {
        b.iter(|| black_box(store.query(&filter_user, 50, None).unwrap()));
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_sha256,
    bench_zstd_compress,
    bench_split_request,
    bench_append_batch,
    bench_append_event,
    bench_query,
);
criterion_main!(benches);
