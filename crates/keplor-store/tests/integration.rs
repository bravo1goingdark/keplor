//! Integration tests: compression-ratio smoke test and concurrent writes.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
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
    }
}

fn system_prompts() -> Vec<String> {
    let bases = [
        "You are a helpful assistant that answers questions concisely and accurately.",
        "You are a professional code reviewer. Analyze code for bugs and security issues.",
        "You are a creative writing assistant helping develop engaging stories and essays.",
        "You are a data analysis expert interpreting datasets and statistical concepts.",
        "You are a customer support agent for a SaaS product with empathy and clarity.",
    ];
    let filler = "Follow these guidelines carefully when responding to the user. \
        Always provide structured, well-organized responses with clear headings and \
        bullet points where appropriate. Include relevant examples and code snippets \
        when discussing technical topics. Maintain a professional tone while being \
        approachable and helpful. If the user's question is ambiguous, ask for \
        clarification before proceeding. When providing recommendations, explain the \
        trade-offs and alternatives. Reference industry best practices and standards \
        where applicable. Keep responses focused and avoid unnecessary tangents. \
        Prioritize accuracy over speed and always verify your reasoning before \
        presenting conclusions. If you are unsure about something, clearly state your \
        level of confidence and suggest where the user might find authoritative sources. ";
    bases
        .iter()
        .map(|b| {
            let mut s = b.to_string();
            for _ in 0..10 {
                s.push_str(filler);
            }
            s
        })
        .collect()
}

fn tool_schemas() -> Vec<String> {
    let tool = r#"[{"type":"function","function":{"name":"get_weather","description":"Get the current weather in a given location with detailed forecast information including temperature, humidity, wind speed, and precipitation probability for the next 7 days","parameters":{"type":"object","properties":{"location":{"type":"string","description":"The city and state or country, e.g. San Francisco, CA or London, UK"},"unit":{"type":"string","enum":["celsius","fahrenheit"],"description":"Temperature unit preference"},"days":{"type":"integer","description":"Number of forecast days (1-14)","default":7}},"required":["location"]}}},{"type":"function","function":{"name":"search_documents","description":"Search the internal documentation knowledge base for relevant articles, guides, and API references matching the user query with support for semantic search and filtering","parameters":{"type":"object","properties":{"query":{"type":"string","description":"Natural language search query"},"limit":{"type":"integer","default":10,"description":"Maximum results to return"},"category":{"type":"string","enum":["api","guides","faq","troubleshooting"],"description":"Filter by document category"}},"required":["query"]}}}]"#;
    vec![tool.to_string(), tool.replace("get_weather", "fetch_forecast")]
}

fn make_request(i: usize, prompts: &[String], tools: &[String]) -> Bytes {
    let sys = &prompts[i % prompts.len()];
    let tool_str =
        if i % 3 == 0 { format!(r#","tools":{}"#, tools[i % tools.len()]) } else { String::new() };
    let msg = format!("Question {i}: help with topic {t}", t = i % 7);
    Bytes::from(format!(
        r#"{{"model":"gpt-4o","messages":[{{"role":"system","content":{sys_json}}},{{"role":"user","content":"{msg}"}}]{tool_str}}}"#,
        sys_json = serde_json::to_string(sys.as_str()).unwrap(),
    ))
}

fn make_response(i: usize) -> Bytes {
    Bytes::from(format!(
        r#"{{"id":"chatcmpl-{i:06}","object":"chat.completion","choices":[{{"index":0,"message":{{"role":"assistant","content":"Here is my response to your question number {i}. The answer involves several key considerations that I'll outline below."}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":100,"completion_tokens":50,"total_tokens":150}}}}"#
    ))
}

#[test]
fn compression_ratio_1000_events() {
    let store = Store::open_in_memory().unwrap();
    let prompts = system_prompts();
    let tools = tool_schemas();

    let mut total_raw_bytes: usize = 0;

    for i in 0..1000 {
        let event = make_event(i);
        let req = make_request(i, &prompts, &tools);
        let resp = make_response(i);
        total_raw_bytes += req.len() + resp.len();
        store.append_event(&event, &req, &resp).unwrap();
    }

    let compressed = store.total_compressed_bytes().unwrap() as usize;
    let uncompressed = store.total_uncompressed_bytes().unwrap() as usize;
    let blob_count = store.blob_count().unwrap();

    let ratio = total_raw_bytes as f64 / compressed as f64;

    eprintln!("=== Compression Smoke Test ===");
    eprintln!("Events:            1000");
    eprintln!("Unique blobs:      {blob_count}");
    eprintln!("Total raw bytes:   {total_raw_bytes}");
    eprintln!("Unique uncompressed: {uncompressed}");
    eprintln!("Unique compressed: {compressed}");
    eprintln!(
        "Dedup ratio:       {:.1}x (raw / unique uncompressed)",
        total_raw_bytes as f64 / uncompressed as f64
    );
    eprintln!("Compression ratio: {ratio:.1}x (raw / unique compressed)");
    eprintln!("Compressed < 5% of raw: {}", (compressed as f64 / total_raw_bytes as f64) < 0.05);

    assert!(
        compressed as f64 / total_raw_bytes as f64 <= 0.05,
        "compressed ({compressed}) should be < 5% of raw ({total_raw_bytes}), ratio = {ratio:.1}x"
    );
}

#[tokio::test]
async fn concurrent_writes_8_tasks() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let prompts = Arc::new(system_prompts());
    let tools = Arc::new(tool_schemas());

    let mut handles = Vec::new();
    for task_id in 0..8 {
        let store = Arc::clone(&store);
        let prompts = Arc::clone(&prompts);
        let tools = Arc::clone(&tools);
        handles.push(tokio::spawn(async move {
            for i in 0..50 {
                let idx = task_id * 50 + i;
                let event = make_event(idx);
                let req = make_request(idx, &prompts, &tools);
                let resp = make_response(idx);
                store.append_event(&event, &req, &resp).unwrap();
            }
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // Verify all 400 events were stored.
    let filter = keplor_store::EventFilter::default();
    let events = store.query(&filter, 500, None).unwrap();
    assert_eq!(events.len(), 400, "all 400 events should be stored");
}

#[test]
fn throughput_append_event_baseline() {
    let store = Store::open_in_memory().unwrap();
    let prompts = system_prompts();
    let tools = tool_schemas();
    let n = 1000;

    let start = Instant::now();
    for i in 0..n {
        let event = make_event(i);
        let req = make_request(i, &prompts, &tools);
        let resp = make_response(i);
        store.append_event(&event, &req, &resp).unwrap();
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
    let prompts = system_prompts();
    let tools = tool_schemas();
    let n = 10_000;
    let batch_size = 64;

    let mut all_events: Vec<(LlmEvent, Bytes, Bytes)> = Vec::with_capacity(n);
    for i in 0..n {
        all_events.push((make_event(i), make_request(i, &prompts, &tools), make_response(i)));
    }

    let start = Instant::now();
    for chunk in all_events.chunks(batch_size) {
        let batch: Vec<(LlmEvent, Bytes, Bytes)> = chunk.to_vec();
        store.append_batch(&batch).unwrap();
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
    let prompts = system_prompts();
    let tools = tool_schemas();
    let n = 10_000usize;

    let start = Instant::now();
    for i in 0..n {
        let event = make_event(i);
        let req = make_request(i, &prompts, &tools);
        let resp = make_response(i);
        writer.write_fire_and_forget(event, req, resp).unwrap();
    }
    // Drop writer to close channel and flush remaining.
    drop(writer);
    // Give flush task time to finish.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let elapsed = start.elapsed();

    let filter = keplor_store::EventFilter::default();
    let _count = store.query(&filter, 1, None).unwrap().len();
    let blobs = store.blob_count().unwrap();
    let rate = n as f64 / elapsed.as_secs_f64();

    eprintln!("=== BatchWriter fire-and-forget ===");
    eprintln!("Events:   {n}");
    eprintln!("Elapsed:  {elapsed:.2?}");
    eprintln!("Rate:     {rate:.0} events/sec");
    eprintln!("Blobs:    {blobs}");

    assert!(blobs > 0, "blobs should have been written");
}
