//! Proxy server: axum app, TLS listener, handler, graceful shutdown.
//!
//! The [`ProxyServer`] ties together the route table, upstream pool,
//! capture sink, and concurrency limiter into a running reverse proxy.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;
use axum::Router;
use http::{header, HeaderMap, StatusCode, Uri};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::capture::{CaptureSink, RequestCtx, StreamOutcome};
use crate::config::{CaptureConfig, ProxyConfig, ServerConfig};
use crate::error::ProxyError;
use crate::headers;
use crate::limits::ConcurrencyLimiter;
use crate::route::RouteTable;
use crate::tee::TeeBody;
use crate::upstream::UpstreamPool;
use keplor_core::EventId;

/// Shared state passed to every request handler via axum's [`State`] extractor.
#[derive(Clone)]
struct AppState {
    route_table: Arc<RouteTable>,
    upstream: UpstreamPool,
    sink: Arc<dyn CaptureSink>,
    limiter: ConcurrencyLimiter,
    capture_config: CaptureConfig,
    #[allow(dead_code)]
    shutdown: CancellationToken,
}

/// A running proxy server.
pub struct ProxyServer {
    config: ProxyConfig,
    state: AppState,
}

impl ProxyServer {
    /// Build a new proxy server from config and a capture sink.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError`] on invalid config or TLS setup failure.
    pub fn new(config: ProxyConfig, sink: Arc<dyn CaptureSink>) -> Result<Self, ProxyError> {
        let route_table = RouteTable::from_config(&config.routes)?;
        let upstream = UpstreamPool::new(&config.upstream)?;
        let limiter = ConcurrencyLimiter::new(config.server.max_concurrent_requests);
        let shutdown = CancellationToken::new();

        let state = AppState {
            route_table: Arc::new(route_table),
            upstream,
            sink,
            limiter,
            capture_config: config.capture.clone(),
            shutdown,
        };

        Ok(Self { config, state })
    }

    /// Returns a clone of the shutdown token so callers can trigger shutdown.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.state.shutdown.clone()
    }

    /// Run the server until shutdown is signalled.
    ///
    /// Listens on the configured address.  If TLS cert/key paths are
    /// provided, terminates TLS; otherwise listens on plain HTTP.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError`] on bind failure, TLS loading error, or I/O error.
    pub async fn run(self) -> Result<(), ProxyError> {
        let listener = TcpListener::bind(self.config.server.listen_addr).await?;
        tracing::info!(addr = %self.config.server.listen_addr, "proxy listening");
        self.run_on(listener).await
    }

    /// Run the server on a pre-bound listener.  Useful for tests where the
    /// OS assigns a random port.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError`] on I/O error.
    pub async fn run_on(self, listener: TcpListener) -> Result<(), ProxyError> {
        let app = Router::new().fallback(proxy_handler).with_state(self.state.clone());

        let tls_acceptor = build_tls_acceptor(&self.config.server)?;
        let shutdown = self.state.shutdown.clone();
        let sink = self.state.sink.clone();
        let timeout = Duration::from_secs(self.config.server.shutdown_timeout_secs);

        if let Some(acceptor) = tls_acceptor {
            run_tls(listener, app, acceptor, shutdown.clone(), timeout).await;
        } else {
            run_plain(listener, app, shutdown.clone(), timeout).await;
        }

        sink.flush().await;
        tracing::info!("shutdown complete");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Plain HTTP server via axum::serve
// ---------------------------------------------------------------------------

async fn run_plain(
    listener: TcpListener,
    app: Router,
    shutdown: CancellationToken,
    _timeout: Duration,
) {
    let serve = axum::serve(listener, app).with_graceful_shutdown(shutdown.cancelled_owned());

    if let Err(e) = serve.await {
        tracing::error!(error = %e, "serve error");
    }
}

// ---------------------------------------------------------------------------
// TLS server via manual accept loop
// ---------------------------------------------------------------------------

async fn run_tls(
    listener: TcpListener,
    app: Router,
    acceptor: tokio_rustls::TlsAcceptor,
    shutdown: CancellationToken,
    _timeout: Duration,
) {
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::server::conn::auto::Builder as ServerBuilder;
    use tower::Service;

    loop {
        let (stream, peer_addr) = tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok(conn) => conn,
                    Err(e) => {
                        tracing::warn!(error = %e, "accept failed");
                        continue;
                    }
                }
            }
            _ = shutdown.cancelled() => break,
        };

        let acceptor = acceptor.clone();
        let service = app.clone().into_service::<hyper::body::Incoming>();

        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(stream).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(peer = %peer_addr, error = %e, "TLS handshake failed");
                    return;
                },
            };

            let hyper_svc = hyper::service::service_fn(move |req| {
                let mut svc = service.clone();
                async move { svc.call(req).await }
            });

            let builder = ServerBuilder::new(TokioExecutor::new());
            if let Err(e) = builder.serve_connection(TokioIo::new(tls_stream), hyper_svc).await {
                tracing::debug!(peer = %peer_addr, error = %e, "connection error");
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Proxy handler
// ---------------------------------------------------------------------------

async fn proxy_handler(State(state): State<AppState>, req: Request) -> Response {
    match do_proxy(state, req).await {
        Ok(resp) => resp,
        Err(e) => error_response(e),
    }
}

fn extract_host(headers: &HeaderMap) -> String {
    headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("<missing>").to_owned()
}

async fn do_proxy(state: AppState, req: Request) -> Result<Response, ProxyError> {
    let host = extract_host(req.headers());
    let path = req.uri().path().to_owned();

    // 1. Route resolution.
    let route = state
        .route_table
        .resolve(&host, &path)
        .ok_or_else(|| ProxyError::NoRoute { host: host.clone(), path: path.clone() })?;

    // 2. Concurrency limit.
    let _permit = state.limiter.try_acquire()?;
    let route_id_str = route.route_id.as_str().to_owned();

    metrics::gauge!("keplor_active_streams", "route" => route_id_str.clone()).increment(1.0);

    let started_at = Instant::now();
    let event_id = EventId::default();

    // 3. Decompose incoming request.
    let (parts, body) = req.into_parts();
    let method_str = parts.method.to_string();

    // 4. Build upstream URI: route's upstream base + original path + query.
    let upstream_uri = build_upstream_uri(&route.upstream_url, parts.uri.path_and_query())?;

    // 5. Request-side tee.
    let (req_tx, req_rx) = if state.capture_config.enabled {
        let (tx, rx) = mpsc::channel(state.capture_config.channel_capacity);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let tee_body = match req_tx {
        Some(tx) => TeeBody::new(body, tx, "request"),
        None => TeeBody::passthrough(body),
    };

    // 6. Build upstream request.
    let mut upstream_headers = parts.headers.clone();
    headers::strip_hop_by_hop_request(&mut upstream_headers);
    if let Some(authority) = route.upstream_url.authority() {
        headers::rewrite_host(&mut upstream_headers, authority.as_str());
    }

    let mut upstream_req_builder = http::Request::builder()
        .method(parts.method.clone())
        .uri(&upstream_uri)
        .version(http::Version::HTTP_11);

    if let Some(hdrs) = upstream_req_builder.headers_mut() {
        *hdrs = upstream_headers;
    }

    let upstream_req = upstream_req_builder
        .body(tee_body)
        .map_err(|e| ProxyError::Config(format!("failed to build upstream request: {e}")))?;

    // 7. Fire capture: request start + drain task.
    let sink = state.sink.clone();
    let ctx = RequestCtx {
        id: event_id,
        method: parts.method,
        uri: parts.uri,
        headers: parts.headers,
        route_id: route.route_id.clone(),
        provider: route.provider.clone(),
        started_at,
    };

    if state.capture_config.enabled {
        sink.on_request_start(ctx).await;
    }

    if let Some(mut rx) = req_rx {
        let sink_clone = sink.clone();
        tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                sink_clone.on_request_chunk(event_id, chunk).await;
            }
            sink_clone.on_request_end(event_id).await;
        });
    }

    // 8. Send upstream request.
    let upstream_start = Instant::now();
    let upstream_resp = state.upstream.client().request(upstream_req).await?;
    let ttft_ms = Some(upstream_start.elapsed().as_millis() as u32);

    let status_u16 = upstream_resp.status().as_u16();

    // 9. Fire capture: response status.
    if state.capture_config.enabled {
        sink.on_response_status(event_id, status_u16, upstream_resp.headers()).await;
    }

    // 10. Response-side tee.
    let mut resp_headers = upstream_resp.headers().clone();
    headers::strip_hop_by_hop_response(&mut resp_headers);

    let resp_status = upstream_resp.status();
    let resp_body = upstream_resp.into_body();

    let (resp_tx, resp_rx) = if state.capture_config.enabled {
        let (tx, rx) = mpsc::channel(state.capture_config.channel_capacity);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let tee_resp_body = match resp_tx {
        Some(tx) => TeeBody::new(resp_body, tx, "response"),
        None => TeeBody::passthrough(resp_body),
    };

    // 11. Spawn response capture drain task.
    {
        let route_id_owned = route_id_str.clone();
        let method_str_clone = method_str.clone();
        if let Some(mut rx) = resp_rx {
            let sink_clone = sink.clone();
            tokio::spawn(async move {
                while let Some(chunk) = rx.recv().await {
                    sink_clone.on_response_chunk(event_id, chunk).await;
                }

                let ttlt_ms = started_at.elapsed().as_millis() as u32;
                sink_clone
                    .on_response_end(event_id, StreamOutcome::Complete { ttft_ms, ttlt_ms })
                    .await;

                metrics::counter!(
                    "keplor_requests_total",
                    "route" => route_id_owned.clone(),
                    "method" => method_str_clone,
                    "status" => status_u16.to_string(),
                )
                .increment(1);

                metrics::gauge!("keplor_active_streams", "route" => route_id_owned).decrement(1.0);
            });
        } else {
            metrics::counter!(
                "keplor_requests_total",
                "route" => route_id_owned,
                "method" => method_str_clone,
                "status" => status_u16.to_string(),
            )
            .increment(1);
            metrics::gauge!("keplor_active_streams", "route" => route_id_str).decrement(1.0);
        }
    }

    // 12. Build response for the client.
    let mut response_builder = Response::builder().status(resp_status);
    if let Some(hdrs) = response_builder.headers_mut() {
        *hdrs = resp_headers;
    }
    let response = response_builder
        .body(Body::new(tee_resp_body))
        .map_err(|e| ProxyError::Config(format!("failed to build response: {e}")))?;

    Ok(response)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_upstream_uri(
    base: &Uri,
    original_path_and_query: Option<&http::uri::PathAndQuery>,
) -> Result<Uri, ProxyError> {
    let scheme = base.scheme_str().unwrap_or("https");
    let authority = base.authority().map(|a| a.as_str()).unwrap_or("localhost");
    let path_and_query = original_path_and_query.map(|pq| pq.as_str()).unwrap_or("/");

    let uri_str = format!("{scheme}://{authority}{path_and_query}");
    uri_str.parse::<Uri>().map_err(ProxyError::from)
}

fn error_response(err: ProxyError) -> Response {
    let (status, msg) = match &err {
        ProxyError::NoRoute { .. } => (StatusCode::BAD_GATEWAY, err.to_string()),
        ProxyError::ConcurrencyLimit(_) => (StatusCode::SERVICE_UNAVAILABLE, err.to_string()),
        ProxyError::ConnectTimeout => (StatusCode::GATEWAY_TIMEOUT, err.to_string()),
        ProxyError::HyperClient(e) => {
            tracing::warn!(error = %e, "upstream error");
            (StatusCode::BAD_GATEWAY, "upstream error".to_owned())
        },
        _ => {
            tracing::error!(error = %err, "proxy error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal proxy error".to_owned())
        },
    };

    let mut resp = Response::new(Body::from(msg));
    *resp.status_mut() = status;
    resp.headers_mut()
        .insert("content-type", http::HeaderValue::from_static("text/plain; charset=utf-8"));
    resp
}

fn build_tls_acceptor(
    config: &ServerConfig,
) -> Result<Option<tokio_rustls::TlsAcceptor>, ProxyError> {
    let (cert_path, key_path) = match (&config.tls_cert_path, &config.tls_key_path) {
        (Some(c), Some(k)) => (c, k),
        (None, None) => return Ok(None),
        _ => {
            return Err(ProxyError::Config(
                "tls_cert_path and tls_key_path must both be set or both absent".into(),
            ));
        },
    };

    let cert_pem = std::fs::read(cert_path).map_err(|e| {
        ProxyError::Config(format!("failed to read TLS cert {}: {e}", cert_path.display()))
    })?;
    let key_pem = std::fs::read(key_path).map_err(|e| {
        ProxyError::Config(format!("failed to read TLS key {}: {e}", key_path.display()))
    })?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_pem.as_slice())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ProxyError::Config(format!("invalid TLS cert: {e}")))?;

    let key = rustls_pemfile::private_key(&mut key_pem.as_slice())
        .map_err(|e| ProxyError::Config(format!("invalid TLS key: {e}")))?
        .ok_or_else(|| ProxyError::Config("no private key found in PEM file".into()))?;

    let tls_config =
        rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(certs, key)?;

    Ok(Some(tokio_rustls::TlsAcceptor::from(Arc::new(tls_config))))
}
