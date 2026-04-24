//! Integration tests: concurrent writes and throughput baselines.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Instant;

use keplor_core::*;
use keplor_store::Store;
use smol_str::SmolStr;

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

#[tokio::test]
async fn concurrent_writes_8_tasks() {
    let store = Arc::new(Store::open_in_memory().unwrap());

    let mut handles = Vec::new();
    for task_id in 0..8 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            for i in 0..50 {
                let idx = task_id * 50 + i;
                let event = make_event(idx);
                store.append_event(&event).unwrap();
            }
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // KeplorDB queries only see rotated segments — force a flush.
    store.wal_checkpoint().unwrap();

    // Verify all 400 events were stored.
    let filter = keplor_store::EventFilter::default();
    let events = store.query(&filter, 500, None).unwrap();
    assert_eq!(events.len(), 400, "all 400 events should be stored");
}

#[test]
fn throughput_append_event_baseline() {
    let store = Store::open_in_memory().unwrap();
    let n = 1000;

    let start = Instant::now();
    for i in 0..n {
        let event = make_event(i);
        store.append_event(&event).unwrap();
    }
    let elapsed = start.elapsed();
    let rate = n as f64 / elapsed.as_secs_f64();

    eprintln!("=== append_event baseline ===");
    eprintln!("Events:   {n}");
    eprintln!("Elapsed:  {elapsed:.2?}");
    eprintln!("Rate:     {rate:.0} events/sec");
}

#[test]
fn throughput_append_batch() {
    let store = Store::open_in_memory().unwrap();
    let n = 10_000;
    let batch_size = 64;

    let all_events: Vec<LlmEvent> = (0..n).map(make_event).collect();

    let start = Instant::now();
    for chunk in all_events.chunks(batch_size) {
        store.append_batch(chunk).unwrap();
    }
    let elapsed = start.elapsed();
    let rate = n as f64 / elapsed.as_secs_f64();

    eprintln!("=== append_batch (batch_size={batch_size}) ===");
    eprintln!("Events:   {n}");
    eprintln!("Elapsed:  {elapsed:.2?}");
    eprintln!("Rate:     {rate:.0} events/sec");
}

#[tokio::test]
async fn throughput_batch_writer() {
    use keplor_store::{BatchConfig, BatchWriter};

    let store = Arc::new(Store::open_in_memory().unwrap());
    let config = BatchConfig { batch_size: 128, channel_capacity: 16384, ..Default::default() };
    let writer = BatchWriter::new(Arc::clone(&store), config);
    let n = 10_000usize;

    let start = Instant::now();
    for i in 0..n {
        let event = make_event(i);
        writer.write_fire_and_forget(event).unwrap();
    }
    // Drop writer to close channel and flush remaining.
    drop(writer);
    // Give flush task time to finish.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let elapsed = start.elapsed();

    let rate = n as f64 / elapsed.as_secs_f64();

    eprintln!("=== BatchWriter fire-and-forget ===");
    eprintln!("Events:   {n}");
    eprintln!("Elapsed:  {elapsed:.2?}");
    eprintln!("Rate:     {rate:.0} events/sec");
}
