//! Body-parse fast path.
//!
//! Drop-in `axum::Json` replacement: reads the request body as
//! `Bytes` once, then deserializes via `simd-json` when the
//! `simd-json` feature is on, else `serde_json`. The simd path is
//! noticeably faster for 1–5 KB JSON on AVX2/AVX-512 CPUs and is
//! the bulk of the per-event parse cost on the ingest hot path.

use axum::body::Bytes;
use axum::extract::FromRequest;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};

/// JSON body extractor that swaps in simd-json when the feature is on.
pub struct FastJson<T>(pub T);

impl<S, T> FromRequest<S> for FastJson<T>
where
    S: Send + Sync,
    T: for<'de> serde::de::Deserialize<'de>,
{
    type Rejection = FastJsonError;

    async fn from_request(
        req: Request<axum::body::Body>,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        if !is_json_content_type(req.headers()) {
            return Err(FastJsonError::WrongContentType);
        }
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|e| FastJsonError::Body(e.to_string()))?;
        parse_json(bytes).map(FastJson)
    }
}

#[cfg(feature = "simd-json")]
fn parse_json<T>(bytes: Bytes) -> Result<T, FastJsonError>
where
    T: for<'de> serde::de::Deserialize<'de>,
{
    // simd-json::serde::from_slice requires &mut [u8] because it
    // mutates the buffer in place during parsing. We own a Bytes —
    // get a mutable copy of the contents and parse from that.
    let mut owned = bytes.to_vec();
    simd_json::serde::from_slice(&mut owned).map_err(|e| FastJsonError::Parse(e.to_string()))
}

#[cfg(not(feature = "simd-json"))]
fn parse_json<T>(bytes: Bytes) -> Result<T, FastJsonError>
where
    T: for<'de> serde::de::Deserialize<'de>,
{
    serde_json::from_slice(&bytes).map_err(|e| FastJsonError::Parse(e.to_string()))
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| {
        let lo = v.to_ascii_lowercase();
        lo.starts_with("application/json") || lo.starts_with("application/") && lo.contains("+json")
    })
}

/// Errors returned by [`FastJson`].
#[derive(Debug)]
pub enum FastJsonError {
    /// `Content-Type` was not a `*/json` variant.
    WrongContentType,
    /// Body could not be read.
    Body(String),
    /// Body could not be parsed as JSON.
    Parse(String),
}

impl IntoResponse for FastJsonError {
    fn into_response(self) -> Response {
        let (code, msg) = match self {
            FastJsonError::WrongContentType => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, "Content-Type must be application/json")
            },
            FastJsonError::Body(e) => (StatusCode::BAD_REQUEST, "failed to read body").map_with(e),
            FastJsonError::Parse(e) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "failed to parse JSON body").map_with(e)
            },
        };
        let body = serde_json::json!({ "error": msg });
        (code, axum::Json(body)).into_response()
    }
}

trait MapWithErr {
    fn map_with(self, _: String) -> (StatusCode, &'static str);
}
impl MapWithErr for (StatusCode, &'static str) {
    fn map_with(self, _: String) -> (StatusCode, &'static str) {
        // Keep error messages opaque — operators inspect logs for
        // serde_json/simd-json detail.
        self
    }
}
