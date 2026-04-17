//! Minimal echo proxy that forwards all traffic to a configured upstream
//! and logs captured bytes to stdout.
//!
//! Usage:
//!   cargo run -p keplor-proxy --example echo-proxy
//!
//! Set environment variables to configure:
//!   KEPLOR_SERVER_LISTEN_ADDR=127.0.0.1:8080
//!   ECHO_UPSTREAM=https://httpbin.org
//!   ECHO_HOST=httpbin.org

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::env;
use std::sync::Arc;

use keplor_proxy::{ChannelSink, ProxyConfig, ProxyServer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let upstream = env::var("ECHO_UPSTREAM").unwrap_or_else(|_| "https://httpbin.org".into());
    let host = env::var("ECHO_HOST").unwrap_or_else(|_| "httpbin.org".into());
    let listen = env::var("KEPLOR_SERVER_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());

    let toml_str = format!(
        r#"
[server]
listen_addr = "{listen}"

[[routes]]
name = "echo"
host = "{host}"
upstream_url = "{upstream}"
"#
    );

    let config: ProxyConfig = toml::from_str(&toml_str)?;

    let (sink, mut rx) = ChannelSink::new();

    // Spawn a task to log capture events.
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            eprintln!("[capture] {event:?}");
        }
    });

    let server = ProxyServer::new(config, Arc::new(sink))?;

    eprintln!("echo-proxy listening on {listen}");
    eprintln!("  forwarding Host: {host} → {upstream}");
    eprintln!("  example: curl -H 'Host: {host}' http://{listen}/get");

    server.run().await?;

    Ok(())
}
