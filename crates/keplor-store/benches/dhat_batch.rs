//! dhat allocation profiling for the batch write path.
//!
//! Run with: `cargo bench --bench dhat_batch`
//! Output: `dhat-heap.json` in the working directory — open at
//! <https://nnethercote.github.io/dh_view/dh_view.html>.

#![allow(clippy::unwrap_used)]

use bytes::Bytes;
use keplor_core::*;
use keplor_store::Store;
use sha2::{Digest, Sha256};
use smol_str::SmolStr;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

fn make_event(i: usize, req: &[u8], resp: &[u8]) -> LlmEvent {
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
        request_sha256: sha256_bytes(req),
        response_sha256: sha256_bytes(resp),
        client_ip: None,
        user_agent: None,
        request_id: None,
        trace_id: None,
        source: None,
        ingested_at: 0,
    }
}

fn make_request(i: usize) -> Bytes {
    let sys = "You are a helpful assistant that answers questions concisely.";
    let msg = format!("Question {i}: help with topic {t}", t = i % 7);
    Bytes::from(format!(
        r#"{{"model":"gpt-4o","messages":[{{"role":"system","content":"{sys}"}},{{"role":"user","content":"{msg}"}}]}}"#,
    ))
}

fn make_response(i: usize) -> Bytes {
    Bytes::from(format!(
        r#"{{"id":"chatcmpl-{i:06}","object":"chat.completion","choices":[{{"index":0,"message":{{"role":"assistant","content":"Response {i}."}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":100,"completion_tokens":50,"total_tokens":150}}}}"#
    ))
}

fn main() {
    let _profiler = dhat::Profiler::new_heap();

    let store = Store::open_in_memory().unwrap();
    let batch_size = 256;
    let batches = 10;

    for batch_idx in 0..batches {
        let events: Vec<(LlmEvent, Bytes, Bytes)> = (0..batch_size)
            .map(|i| {
                let idx = batch_idx * batch_size + i;
                let req = make_request(idx);
                let resp = make_response(idx);
                let event = make_event(idx, &req, &resp);
                (event, req, resp)
            })
            .collect();
        store.append_batch(&events).unwrap();
    }

    eprintln!("Profiled {n} events in {batches} batches", n = batches * batch_size);
}
