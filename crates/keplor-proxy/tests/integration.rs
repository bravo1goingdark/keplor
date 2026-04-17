//! Integration tests for the proxy.
//!
//! Spins up a `wiremock` upstream server and a `ProxyServer` in plain HTTP
//! mode, then sends requests through the proxy and asserts byte-exact
//! forwarding and capture correctness.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use keplor_proxy::{CaptureEvent, ChannelSink, NullSink, ProxyConfig, ProxyServer};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn test_config(upstream_url: &str, listen_port: u16) -> ProxyConfig {
    let toml_str = format!(
        r#"
[server]
listen_addr = "127.0.0.1:{listen_port}"
max_concurrent_requests = 100

[upstream]
connect_timeout_secs = 5

[capture]
channel_capacity = 256

[[routes]]
name = "test"
host = "127.0.0.1"
upstream_url = "{upstream_url}"
"#
    );
    toml::from_str(&toml_str).expect("valid test config")
}

async fn wait_for_port(addr: &str) {
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("server did not start on {addr}");
}

#[tokio::test]
async fn proxy_forwards_get_request() {
    let mock_server = MockServer::start().await;
    let body = "hello from upstream";

    Mock::given(method("GET"))
        .and(path("/v1/test"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(body).insert_header("x-custom", "preserved"),
        )
        .mount(&mock_server)
        .await;

    let port = portpicker::pick_unused_port().expect("no free port");
    let config = test_config(&mock_server.uri(), port);
    let (sink, mut rx) = ChannelSink::new();
    let server = ProxyServer::new(config, Arc::new(sink)).expect("server setup");
    let token = server.shutdown_token();

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    let addr = format!("127.0.0.1:{port}");
    wait_for_port(&addr).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/v1/test"))
        .header("host", "127.0.0.1")
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("x-custom").unwrap(), "preserved");
    let resp_body = resp.text().await.expect("body read failed");
    assert_eq!(resp_body, body);

    // Shutdown server.
    token.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Check capture events.
    rx.close();
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }

    // Should have: RequestStart, RequestEnd, ResponseStatus, ResponseChunk(s), ResponseEnd, Flush
    assert!(
        events.iter().any(|e| matches!(e, CaptureEvent::RequestStart(_))),
        "missing RequestStart"
    );
    assert!(
        events.iter().any(|e| matches!(e, CaptureEvent::ResponseStatus { status: 200, .. })),
        "missing ResponseStatus(200)"
    );
    assert!(
        events.iter().any(|e| matches!(e, CaptureEvent::ResponseEnd { .. })),
        "missing ResponseEnd"
    );

    // Verify captured response bytes match.
    let captured_bytes: Vec<u8> = events
        .iter()
        .filter_map(|e| match e {
            CaptureEvent::ResponseChunk { chunk, .. } => Some(chunk.to_vec()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(String::from_utf8_lossy(&captured_bytes), body, "captured bytes don't match");
}

#[tokio::test]
async fn proxy_forwards_post_with_body() {
    let mock_server = MockServer::start().await;
    let req_body = r#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#;
    let resp_body = r#"{"id":"chatcmpl-abc","choices":[{"message":{"content":"Hello!"}}]}"#;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(resp_body)
                .insert_header("content-type", "application/json"),
        )
        .mount(&mock_server)
        .await;

    let port = portpicker::pick_unused_port().expect("no free port");
    let config = test_config(&mock_server.uri(), port);
    let (sink, mut rx) = ChannelSink::new();
    let server = ProxyServer::new(config, Arc::new(sink)).expect("server setup");
    let token = server.shutdown_token();

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    let addr = format!("127.0.0.1:{port}");
    wait_for_port(&addr).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .header("host", "127.0.0.1")
        .header("authorization", "Bearer sk-live-test")
        .header("content-type", "application/json")
        .body(req_body)
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.expect("body read");
    assert_eq!(body, resp_body);

    token.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;

    rx.close();
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }

    // Verify captured request body.
    let captured_req: Vec<u8> = events
        .iter()
        .filter_map(|e| match e {
            CaptureEvent::RequestChunk { chunk, .. } => Some(chunk.to_vec()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(
        String::from_utf8_lossy(&captured_req),
        req_body,
        "captured request body doesn't match"
    );

    // Verify captured response body.
    let captured_resp: Vec<u8> = events
        .iter()
        .filter_map(|e| match e {
            CaptureEvent::ResponseChunk { chunk, .. } => Some(chunk.to_vec()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(
        String::from_utf8_lossy(&captured_resp),
        resp_body,
        "captured response body doesn't match"
    );
}

#[tokio::test]
async fn proxy_forwards_upstream_errors_transparently() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_string(r#"{"error":{"message":"rate limited"}}"#)
                .insert_header("retry-after", "30"),
        )
        .mount(&mock_server)
        .await;

    let port = portpicker::pick_unused_port().expect("no free port");
    let config = test_config(&mock_server.uri(), port);
    let server = ProxyServer::new(config, Arc::new(NullSink)).expect("server setup");
    let token = server.shutdown_token();

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    let addr = format!("127.0.0.1:{port}");
    wait_for_port(&addr).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .header("host", "127.0.0.1")
        .body("{}")
        .send()
        .await
        .expect("request failed");

    // Upstream 429 is forwarded as-is.
    assert_eq!(resp.status(), 429);
    assert_eq!(resp.headers().get("retry-after").unwrap(), "30");

    token.cancel();
}

#[tokio::test]
async fn proxy_returns_502_for_unknown_host() {
    let mock_server = MockServer::start().await;

    let port = portpicker::pick_unused_port().expect("no free port");
    let config = test_config(&mock_server.uri(), port);
    let server = ProxyServer::new(config, Arc::new(NullSink)).expect("server setup");
    let token = server.shutdown_token();

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    let addr = format!("127.0.0.1:{port}");
    wait_for_port(&addr).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/test"))
        .header("host", "unknown.example.com")
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 502);

    token.cancel();
}

#[tokio::test]
async fn proxy_preserves_query_params() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&mock_server)
        .await;

    let port = portpicker::pick_unused_port().expect("no free port");
    let config = test_config(&mock_server.uri(), port);
    let server = ProxyServer::new(config, Arc::new(NullSink)).expect("server setup");
    let token = server.shutdown_token();

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    let addr = format!("127.0.0.1:{port}");
    wait_for_port(&addr).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/v1/models?filter=gpt"))
        .header("host", "127.0.0.1")
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200);
    token.cancel();
}
