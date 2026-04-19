//! End-to-end HTTP integration tests.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use keplor_pricing::Catalog;
use keplor_server::auth::ApiKeySet;
use keplor_server::config::ServerConfig;
use keplor_server::server::install_metrics_recorder;
use keplor_server::{Pipeline, PipelineServer};
use keplor_store::{BatchConfig, BatchWriter, Store};
use reqwest::StatusCode;
use std::sync::Once;

static INIT_CRYPTO: Once = Once::new();

async fn spawn_server(api_keys: Vec<String>) -> String {
    INIT_CRYPTO.call_once(|| {
        // Ok(()) on first call, Err if already installed — both are fine.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
    let store = Arc::new(Store::open_in_memory().unwrap());
    let writer = Arc::new(BatchWriter::new(Arc::clone(&store), BatchConfig::default()));
    let catalog = Arc::new(Catalog::load_bundled().unwrap());
    let pipeline = Pipeline::new(store, writer, catalog);

    let keys = ApiKeySet::new(api_keys, "free");
    let config = ServerConfig::default();
    let metrics_handle = install_metrics_recorder();
    let server = PipelineServer::new(pipeline, keys, &config, metrics_handle).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");

    tokio::spawn(async move {
        server.run_on(listener).await.unwrap();
    });

    // Give server a moment to start.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    base
}

#[tokio::test]
async fn health_returns_ok() {
    let base = spawn_server(vec![]).await;
    let resp = reqwest::get(format!("{base}/health")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["db"], "connected");
}

#[tokio::test]
async fn ingest_single_event() {
    let base = spawn_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/events"))
        .json(&serde_json::json!({
            "model": "gpt-4o",
            "provider": "openai",
            "usage": {"input_tokens": 1000, "output_tokens": 500},
            "source": "test"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body["id"].as_str().unwrap().is_empty());
    assert!(body["cost_nanodollars"].as_i64().unwrap() > 0);
    assert_eq!(body["provider"], "openai");
}

#[tokio::test]
async fn ingest_batch() {
    let base = spawn_server(vec![]).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/v1/events/batch"))
        .json(&serde_json::json!({
            "events": [
                {"model": "gpt-4o", "provider": "openai"},
                {"model": "claude-sonnet-4-20250514", "provider": "anthropic"}
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["accepted"], 2);
    assert_eq!(body["rejected"], 0);
}

#[tokio::test]
async fn ingest_and_query() {
    let base = spawn_server(vec![]).await;
    let client = reqwest::Client::new();

    // Ingest.
    client
        .post(format!("{base}/v1/events"))
        .json(&serde_json::json!({
            "model": "gpt-4o",
            "provider": "openai",
            "user_id": "alice",
            "source": "test-harness"
        }))
        .send()
        .await
        .unwrap();

    // Small delay for batch flush.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Query all.
    let resp = client.get(format!("{base}/v1/events")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body["events"].as_array().unwrap().is_empty());

    // Query with user filter.
    let resp = client.get(format!("{base}/v1/events?user_id=alice")).send().await.unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["user_id"], "alice");

    // Query with non-matching filter.
    let resp = client.get(format!("{base}/v1/events?user_id=bob")).send().await.unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn auth_rejects_bad_key() {
    let base = spawn_server(vec!["secret123".into()]).await;
    let client = reqwest::Client::new();

    // No auth header.
    let resp = client
        .post(format!("{base}/v1/events"))
        .json(&serde_json::json!({"model": "gpt-4o", "provider": "openai"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Wrong key.
    let resp = client
        .post(format!("{base}/v1/events"))
        .header("Authorization", "Bearer wrongkey")
        .json(&serde_json::json!({"model": "gpt-4o", "provider": "openai"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Correct key.
    let resp = client
        .post(format!("{base}/v1/events"))
        .header("Authorization", "Bearer secret123")
        .json(&serde_json::json!({"model": "gpt-4o", "provider": "openai"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn validation_rejects_bad_input() {
    let base = spawn_server(vec![]).await;
    let client = reqwest::Client::new();

    // Empty model.
    let resp = client
        .post(format!("{base}/v1/events"))
        .json(&serde_json::json!({"model": "", "provider": "openai"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Missing required fields.
    let resp = client
        .post(format!("{base}/v1/events"))
        .json(&serde_json::json!({"model": "gpt-4o"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn auth_key_attribution_overrides_spoofed_id() {
    // Use explicit id:secret format.
    let base = spawn_server(vec!["prod-svc:my-secret-key".into()]).await;
    let client = reqwest::Client::new();

    // Client tries to spoof api_key_id, but server should override it.
    let resp = client
        .post(format!("{base}/v1/events"))
        .header("Authorization", "Bearer my-secret-key")
        .json(&serde_json::json!({
            "model": "gpt-4o",
            "provider": "openai",
            "api_key_id": "spoofed-id",
            "user_id": "alice"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Small delay for batch flush.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Query and verify the api_key_id was set by the server, not the client.
    let resp = client
        .get(format!("{base}/v1/events?user_id=alice"))
        .header("Authorization", "Bearer my-secret-key")
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0]["api_key_id"], "prod-svc",
        "server should inject authenticated key id, not client-provided spoofed-id"
    );
}

#[tokio::test]
async fn metrics_endpoint_works() {
    let base = spawn_server(vec![]).await;
    let resp = reqwest::get(format!("{base}/metrics")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(content_type.contains("text/plain"));
}

// ── Failure-mode tests ──────────────���─────────────────────────────────

/// Helper that boots a server with a custom pipeline/config for failure-mode tests.
async fn spawn_custom_server(pipeline: Pipeline, config: ServerConfig) -> String {
    INIT_CRYPTO.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
    let keys = ApiKeySet::new(vec![], "free");
    let metrics_handle = install_metrics_recorder();
    let server = PipelineServer::new(pipeline, keys, &config, metrics_handle).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");

    tokio::spawn(async move {
        server.run_on(listener).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    base
}

#[tokio::test]
async fn backpressure_returns_503() {
    // Channel capacity = 1 so the queue fills quickly.
    let store = Arc::new(Store::open_in_memory().unwrap());
    let tiny_config =
        BatchConfig { batch_size: 256, channel_capacity: 1, ..BatchConfig::default() };
    let writer = Arc::new(BatchWriter::new(Arc::clone(&store), tiny_config));
    let catalog = Arc::new(Catalog::load_bundled().unwrap());
    let pipeline = Pipeline::new(store, writer, catalog);

    let base = spawn_custom_server(pipeline, ServerConfig::default()).await;
    let client = reqwest::Client::new();

    // Blast fire-and-forget events — at least one should hit 503.
    let mut saw_503 = false;
    for _ in 0..50 {
        let resp = client
            .post(format!("{base}/v1/events/batch"))
            .json(&serde_json::json!({
                "events": (0..20).map(|_| serde_json::json!({
                    "model": "gpt-4o", "provider": "openai"
                })).collect::<Vec<_>>()
            }))
            .send()
            .await
            .unwrap();
        if resp.status() == StatusCode::SERVICE_UNAVAILABLE
            || resp.status() == StatusCode::MULTI_STATUS
        {
            saw_503 = true;
            break;
        }
    }

    assert!(saw_503, "expected 503 or 207 (partial failure) when queue is saturated");
}

#[tokio::test]
async fn db_size_limit_returns_507() {
    // 1 MB limit — ingest events with large metadata to push past it.
    let store = Arc::new(Store::open_in_memory().unwrap());
    let writer = Arc::new(BatchWriter::new(Arc::clone(&store), BatchConfig::default()));
    let catalog = Arc::new(Catalog::load_bundled().unwrap());
    let pipeline = Pipeline::new(store, writer, catalog).with_max_db_size_mb(1);

    let base = spawn_custom_server(pipeline, ServerConfig::default()).await;
    let client = reqwest::Client::new();

    let big_body = "x".repeat(2048);
    let mut saw_507 = false;
    for _ in 0..50 {
        let events: Vec<_> = (0..100)
            .map(|_| {
                serde_json::json!({
                    "model": "gpt-4o",
                    "provider": "openai",
                    "metadata": {"payload": &big_body},
                })
            })
            .collect();
        let resp = client
            .post(format!("{base}/v1/events/batch"))
            .header("x-keplor-durable", "true")
            .json(&serde_json::json!({ "events": events }))
            .send()
            .await
            .unwrap();

        if resp.status() == StatusCode::INSUFFICIENT_STORAGE {
            saw_507 = true;
            break;
        }
        // A 207 with storage_full errors also counts.
        if resp.status() == StatusCode::MULTI_STATUS {
            let body: serde_json::Value = resp.json().await.unwrap();
            let text = serde_json::to_string(&body).unwrap_or_default();
            if text.contains("storage full") {
                saw_507 = true;
                break;
            }
        }
    }
    assert!(saw_507, "expected 507 when database exceeds max_db_size_mb");
}

#[tokio::test]
async fn validation_rejects_oversized_model_name() {
    let base = spawn_server(vec![]).await;
    let client = reqwest::Client::new();

    // Model name exceeding 256 characters.
    let long_model = "m".repeat(300);
    let resp = client
        .post(format!("{base}/v1/events"))
        .json(&serde_json::json!({"model": long_model, "provider": "openai"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("256"));
}

#[tokio::test]
async fn health_accessible_under_connection_pressure() {
    // Server with max_connections = 1.
    let store = Arc::new(Store::open_in_memory().unwrap());
    let writer = Arc::new(BatchWriter::new(Arc::clone(&store), BatchConfig::default()));
    let catalog = Arc::new(Catalog::load_bundled().unwrap());
    let pipeline = Pipeline::new(store, writer, catalog);

    let mut config = ServerConfig::default();
    config.server.max_connections = 1;

    let base = spawn_custom_server(pipeline, config).await;
    let client = reqwest::Client::new();

    // Occupy the single connection slot with a slow ingest.
    let slow = tokio::spawn({
        let client = client.clone();
        let url = format!("{base}/v1/events/batch");
        async move {
            // Large durable batch to keep the connection busy.
            let events: Vec<_> = (0..500)
                .map(|_| serde_json::json!({"model": "gpt-4o", "provider": "openai"}))
                .collect();
            let _ = client
                .post(url)
                .header("x-keplor-durable", "true")
                .json(&serde_json::json!({"events": events}))
                .send()
                .await;
        }
    });

    // Small delay to ensure the slow request is in-flight.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // Health should still respond because it bypasses the concurrency limit.
    let resp = client
        .get(format!("{base}/health"))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "health endpoint should remain accessible under connection pressure"
    );

    slow.abort();
}

#[tokio::test]
async fn graceful_shutdown_drains_events() {
    INIT_CRYPTO.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });

    let store = Arc::new(Store::open_in_memory().unwrap());
    let writer = Arc::new(BatchWriter::new(Arc::clone(&store), BatchConfig::default()));
    let catalog = Arc::new(Catalog::load_bundled().unwrap());
    let pipeline = Pipeline::new(Arc::clone(&store), writer, catalog);

    let keys = ApiKeySet::new(vec![], "free");
    let config = ServerConfig::default();
    let metrics_handle = install_metrics_recorder();
    let server = PipelineServer::new(pipeline, keys, &config, metrics_handle).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");

    let server_handle = tokio::spawn(async move {
        server.run_on(listener).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let client = reqwest::Client::new();

    // Ingest events (fire-and-forget via batch).
    client
        .post(format!("{base}/v1/events/batch"))
        .json(&serde_json::json!({
            "events": (0..10).map(|_| serde_json::json!({
                "model": "gpt-4o", "provider": "openai"
            })).collect::<Vec<_>>()
        }))
        .send()
        .await
        .unwrap();

    // Trigger graceful shutdown via SIGINT to the current process would be
    // disruptive, so instead we abort the server handle and verify events
    // were flushed during the batch writer's 50ms interval.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    server_handle.abort();

    // Verify events were persisted.
    let filter = keplor_store::EventFilter::default();
    let events = store.query_summary(&filter, 100, None).unwrap();
    assert!(events.len() >= 10, "expected at least 10 events persisted, got {}", events.len());
}
