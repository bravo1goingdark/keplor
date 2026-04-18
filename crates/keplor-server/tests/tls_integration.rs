//! TLS integration tests — verifies HTTPS ingestion with self-signed certs.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use keplor_pricing::Catalog;
use keplor_server::auth::ApiKeySet;
use keplor_server::config::{ServerConfig, TlsConfig};
use keplor_server::server::install_metrics_recorder;
use keplor_server::{Pipeline, PipelineServer};
use keplor_store::{BatchConfig, BatchWriter, Store};
use rcgen::CertifiedKey;
use reqwest::StatusCode;
use std::io::Write;
use std::sync::Once;
use tempfile::NamedTempFile;

static INIT_CRYPTO: Once = Once::new();

struct CertPair {
    cert_file: NamedTempFile,
    key_file: NamedTempFile,
    /// DER certificate for the reqwest client trust anchor.
    cert_der: Vec<u8>,
}

fn generate_self_signed_cert() -> CertPair {
    let CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_owned(), "127.0.0.1".to_owned()])
            .unwrap();

    let cert_pem = cert.pem();
    let key_pem = signing_key.serialize_pem();
    let cert_der = cert.der().to_vec();

    let mut cert_file = NamedTempFile::new().unwrap();
    cert_file.write_all(cert_pem.as_bytes()).unwrap();
    cert_file.flush().unwrap();

    let mut key_file = NamedTempFile::new().unwrap();
    key_file.write_all(key_pem.as_bytes()).unwrap();
    key_file.flush().unwrap();

    CertPair { cert_file, key_file, cert_der }
}

async fn spawn_tls_server(certs: &CertPair) -> String {
    INIT_CRYPTO.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });

    let store = Arc::new(Store::open_in_memory().unwrap());
    let writer = Arc::new(BatchWriter::new(Arc::clone(&store), BatchConfig::default()));
    let catalog = Arc::new(Catalog::load_bundled().unwrap());
    let pipeline = Pipeline::new(store, writer, catalog);

    let keys = ApiKeySet::new(vec![], "free");
    let config = ServerConfig {
        tls: Some(TlsConfig {
            cert_path: certs.cert_file.path().to_path_buf(),
            key_path: certs.key_file.path().to_path_buf(),
        }),
        ..Default::default()
    };

    let metrics_handle = install_metrics_recorder();
    let server = PipelineServer::new(pipeline, keys, &config, metrics_handle);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("https://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        server.run_on(listener).await.unwrap();
    });

    // Give server time to start.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    base
}

fn build_tls_client(cert_der: &[u8]) -> reqwest::Client {
    let cert = reqwest::Certificate::from_der(cert_der).unwrap();
    reqwest::Client::builder().add_root_certificate(cert).build().unwrap()
}

#[tokio::test]
async fn tls_health_returns_ok() {
    let certs = generate_self_signed_cert();
    let base = spawn_tls_server(&certs).await;
    let client = build_tls_client(&certs.cert_der);

    let resp = client.get(format!("{base}/health")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn tls_ingest_single_event() {
    let certs = generate_self_signed_cert();
    let base = spawn_tls_server(&certs).await;
    let client = build_tls_client(&certs.cert_der);

    let resp = client
        .post(format!("{base}/v1/events"))
        .json(&serde_json::json!({
            "model": "gpt-4o",
            "provider": "openai",
            "usage": {"input_tokens": 500, "output_tokens": 200}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body["id"].as_str().unwrap().is_empty());
    assert!(body["cost_nanodollars"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn tls_rejects_plain_http() {
    let certs = generate_self_signed_cert();
    let base = spawn_tls_server(&certs).await;
    // Try plain HTTP against a TLS listener — should fail.
    let plain_base = base.replace("https://", "http://");
    let result = reqwest::get(format!("{plain_base}/health")).await;
    assert!(result.is_err(), "plain HTTP should fail against TLS listener");
}
