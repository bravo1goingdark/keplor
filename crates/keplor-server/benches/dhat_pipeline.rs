//! dhat allocation profiling for the full ingestion pipeline.
//!
//! Run with: `cargo bench --bench dhat_pipeline -p keplor-server`
//! Output: `dhat-heap.json` in the working directory — open at
//! <https://nnethercote.github.io/dh_view/dh_view.html>.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use keplor_pricing::Catalog;
use keplor_server::Pipeline;
use keplor_store::{BatchConfig, BatchWriter, Store};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn minimal_json() -> &'static str {
    r#"{"model":"gpt-4o","provider":"openai","usage":{"input_tokens":100,"output_tokens":50}}"#
}

fn with_bodies_json() -> &'static str {
    r#"{
        "model":"gpt-4o","provider":"openai",
        "usage":{"input_tokens":150,"output_tokens":80},
        "http_status":200,"source":"litellm","user_id":"user_42",
        "endpoint":"/v1/chat/completions",
        "request_body":{
            "model":"gpt-4o",
            "messages":[
                {"role":"system","content":"You are a helpful assistant."},
                {"role":"user","content":"Explain gravitational lensing in astrophysics."}
            ],
            "stream":true,"max_tokens":512
        },
        "response_body":{
            "id":"chatcmpl-001","object":"chat.completion","model":"gpt-4o",
            "choices":[{"index":0,"message":{"role":"assistant","content":"Gravitational lensing bends light near massive objects."},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":150,"completion_tokens":80,"total_tokens":230}
        }
    }"#
}

fn main() {
    let _profiler = dhat::Profiler::new_heap();

    // BatchWriter spawns a tokio task for its flush loop, so we need a runtime.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let _guard = rt.enter();

    let store = Arc::new(Store::open_in_memory().unwrap());
    let writer = Arc::new(BatchWriter::new(Arc::clone(&store), BatchConfig::default()));
    let catalog = Arc::new(Catalog::load_bundled().unwrap());
    let pipeline = Pipeline::new(store, writer, catalog);

    let event_count = 2000;
    let json_sources = [minimal_json(), with_bodies_json()];

    for i in 0..event_count {
        let json = json_sources[i % json_sources.len()];
        let event = serde_json::from_str(json).unwrap();
        pipeline.ingest_fire_and_forget(event, None, "free").unwrap();
    }

    eprintln!("Profiled {event_count} events through full pipeline");
}
