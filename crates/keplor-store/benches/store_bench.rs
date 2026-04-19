//! Criterion benchmarks for keplor-store hot paths.

#![allow(clippy::unwrap_used)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use keplor_core::*;
use keplor_store::Store;
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
        metadata: None,
        tier: smol_str::SmolStr::new("free"),
    }
}

// ── Benchmarks ─────────────────────────────────────────────────────────

fn bench_append_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("append_batch");
    for batch_size in [32, 64, 128, 256] {
        let events: Vec<LlmEvent> = (0..batch_size).map(make_event).collect();

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

    c.bench_function("append_event_single", |b| {
        b.iter_with_setup(
            || Store::open_in_memory().unwrap(),
            |store| {
                black_box(store.append_event(&event).unwrap());
            },
        );
    });
}

fn bench_query(c: &mut Criterion) {
    let store = Store::open_in_memory().unwrap();
    // Seed 1000 events.
    let events: Vec<LlmEvent> = (0..1000).map(make_event).collect();
    store.append_batch(&events).unwrap();

    let filter_none = keplor_store::EventFilter::default();
    let filter_user =
        keplor_store::EventFilter { user_id: Some(SmolStr::new("user_1")), ..Default::default() };

    let mut group = c.benchmark_group("query");
    group.bench_function("full_no_filter_limit50", |b| {
        b.iter(|| black_box(store.query(&filter_none, 50, None).unwrap()));
    });
    group.bench_function("full_user_filter_limit50", |b| {
        b.iter(|| black_box(store.query(&filter_user, 50, None).unwrap()));
    });
    group.bench_function("summary_no_filter_limit50", |b| {
        b.iter(|| black_box(store.query_summary(&filter_none, 50, None).unwrap()));
    });
    group.bench_function("summary_user_filter_limit50", |b| {
        b.iter(|| black_box(store.query_summary(&filter_user, 50, None).unwrap()));
    });
    group.finish();
}

criterion_group!(benches, bench_append_batch, bench_append_event, bench_query,);
criterion_main!(benches);
