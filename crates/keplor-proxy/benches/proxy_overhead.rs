//! Criterion benchmark for proxy round-trip overhead.
//!
//! Measures p50/p99 overhead of a 16 KB request/response round-trip through
//! the proxy with a wiremock upstream.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use keplor_proxy::{NullSink, ProxyConfig, ProxyServer};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn build_config(upstream_url: &str, port: u16) -> ProxyConfig {
    let toml_str = format!(
        r#"
[server]
listen_addr = "127.0.0.1:{port}"

[capture]
enabled = false

[[routes]]
name = "bench"
host = "127.0.0.1"
upstream_url = "{upstream_url}"
"#
    );
    toml::from_str(&toml_str).expect("valid config")
}

fn proxy_roundtrip(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let (mock_server, port, _token) = rt.block_on(async {
        let mock_server = MockServer::start().await;

        let response_body = vec![0x42u8; 16 * 1024]; // 16 KB
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(response_body)
                    .insert_header("content-type", "application/json"),
            )
            .mount(&mock_server)
            .await;

        let port = portpicker::pick_unused_port().expect("no free port");
        let config = build_config(&mock_server.uri(), port);
        let server = ProxyServer::new(config, Arc::new(NullSink)).expect("server");
        let token = server.shutdown_token();

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        // Wait for server to be ready.
        let addr = format!("127.0.0.1:{port}");
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(&addr).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        (mock_server, port, token)
    });

    let request_body = vec![0x41u8; 16 * 1024]; // 16 KB
    let addr = format!("http://127.0.0.1:{port}/v1/chat/completions");

    c.bench_function("16kb_roundtrip", |b| {
        let client = reqwest::Client::new();
        b.to_async(&rt).iter(|| {
            let client = client.clone();
            let addr = addr.clone();
            let body = request_body.clone();
            async move {
                let resp = client
                    .post(&addr)
                    .header("host", "127.0.0.1")
                    .body(body)
                    .send()
                    .await
                    .expect("request");
                let _ = resp.bytes().await;
            }
        });
    });

    // Keep mock_server alive for the duration.
    drop(mock_server);
}

criterion_group!(benches, proxy_roundtrip);
criterion_main!(benches);
