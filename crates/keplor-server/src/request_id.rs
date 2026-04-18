//! Request ID middleware — propagates or generates `X-Request-Id` headers.
//!
//! If the incoming request contains an `X-Request-Id` header, its value is
//! reused. Otherwise a new ULID is generated. The ID is:
//! 1. Inserted into request extensions as [`RequestId`].
//! 2. Added to the response as the `X-Request-Id` header.
//! 3. Recorded in the current tracing span.

use axum::http::{HeaderName, HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;
use smol_str::SmolStr;

static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// A request correlation ID, available in request extensions.
#[derive(Debug, Clone)]
pub struct RequestId(pub SmolStr);

/// Middleware that reads or generates `X-Request-Id` and propagates it.
pub async fn propagate_request_id(mut req: Request<axum::body::Body>, next: Next) -> Response {
    let id = req
        .headers()
        .get(&X_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(SmolStr::new)
        .unwrap_or_else(|| SmolStr::new(ulid::Ulid::new().to_string()));

    req.extensions_mut().insert(RequestId(id.clone()));

    tracing::Span::current().record("request_id", id.as_str());

    let mut resp = next.run(req).await;

    if let Ok(val) = HeaderValue::from_str(&id) {
        resp.headers_mut().insert(X_REQUEST_ID.clone(), val);
    }

    resp
}
