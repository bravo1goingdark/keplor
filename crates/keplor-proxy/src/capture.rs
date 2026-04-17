//! Capture sink trait and implementations.
//!
//! The [`CaptureSink`] trait is the boundary between the proxy forwarding
//! path and the observation/storage pipeline.  Implementations receive
//! streaming callbacks as request and response bytes flow through the proxy.

use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use http::{HeaderMap, Method, Uri};
use keplor_core::{EventId, Provider, RouteId};

/// Metadata about a request at the moment the proxy starts handling it.
#[derive(Debug, Clone)]
pub struct RequestCtx {
    /// Unique event identifier for this request/response pair.
    pub id: EventId,
    /// HTTP method.
    pub method: Method,
    /// Original request URI (path + query, as seen by the proxy).
    pub uri: Uri,
    /// Request headers (including auth — the sink decides what to persist).
    pub headers: HeaderMap,
    /// Route that matched this request.
    pub route_id: RouteId,
    /// Auto-detected provider, if any.
    pub provider: Option<Provider>,
    /// Wall-clock instant when the proxy started handling the request.
    pub started_at: Instant,
}

/// How the response stream ended.
#[derive(Debug, Clone)]
pub enum StreamOutcome {
    /// The full response was delivered.
    Complete {
        /// Time-to-first-byte in milliseconds (None for non-streaming).
        ttft_ms: Option<u32>,
        /// Total latency in milliseconds (request start → last byte).
        ttlt_ms: u32,
    },
    /// The client disconnected before the response was fully sent.
    ClientDisconnect {
        /// Bytes successfully sent before disconnect.
        bytes_sent: u64,
    },
    /// The upstream returned an error before streaming started.
    UpstreamError {
        /// HTTP status code from the upstream.
        status: u16,
        /// Best-effort error message.
        message: String,
    },
    /// Capture was dropped due to backpressure (forwarding continued).
    CaptureDropped,
}

/// Trait for receiving streamed capture callbacks from the proxy.
///
/// Implementations must be `Send + Sync` because they are shared across
/// request-handling tasks via `Arc<dyn CaptureSink>`.
#[async_trait]
pub trait CaptureSink: Send + Sync {
    /// Called when the proxy starts handling a new request.
    async fn on_request_start(&self, ctx: RequestCtx);

    /// Called for each chunk of the request body.
    async fn on_request_chunk(&self, id: EventId, chunk: Bytes);

    /// Called when the full request body has been forwarded.
    async fn on_request_end(&self, id: EventId);

    /// Called when the upstream responds with status + headers.
    async fn on_response_status(&self, id: EventId, status: u16, headers: &HeaderMap);

    /// Called for each chunk of the response body.
    async fn on_response_chunk(&self, id: EventId, chunk: Bytes);

    /// Called when the response stream finishes (or fails).
    async fn on_response_end(&self, id: EventId, outcome: StreamOutcome);

    /// Flush any buffered state.  Called during graceful shutdown.
    async fn flush(&self);
}

// ---------------------------------------------------------------------------
// NullSink — no-op, for tests and benchmarks
// ---------------------------------------------------------------------------

/// A [`CaptureSink`] that discards everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullSink;

#[async_trait]
impl CaptureSink for NullSink {
    async fn on_request_start(&self, _ctx: RequestCtx) {}
    async fn on_request_chunk(&self, _id: EventId, _chunk: Bytes) {}
    async fn on_request_end(&self, _id: EventId) {}
    async fn on_response_status(&self, _id: EventId, _status: u16, _headers: &HeaderMap) {}
    async fn on_response_chunk(&self, _id: EventId, _chunk: Bytes) {}
    async fn on_response_end(&self, _id: EventId, _outcome: StreamOutcome) {}
    async fn flush(&self) {}
}

// ---------------------------------------------------------------------------
// ChannelSink — ships events to an unbounded mpsc for testing
// ---------------------------------------------------------------------------

/// Enumeration of capture events, one variant per [`CaptureSink`] callback.
#[derive(Debug, Clone)]
pub enum CaptureEvent {
    /// Corresponds to [`CaptureSink::on_request_start`].
    RequestStart(RequestCtx),
    /// Corresponds to [`CaptureSink::on_request_chunk`].
    RequestChunk {
        /// Event id.
        id: EventId,
        /// Chunk bytes.
        chunk: Bytes,
    },
    /// Corresponds to [`CaptureSink::on_request_end`].
    RequestEnd(EventId),
    /// Corresponds to [`CaptureSink::on_response_status`].
    ResponseStatus {
        /// Event id.
        id: EventId,
        /// HTTP status.
        status: u16,
        /// Response headers.
        headers: HeaderMap,
    },
    /// Corresponds to [`CaptureSink::on_response_chunk`].
    ResponseChunk {
        /// Event id.
        id: EventId,
        /// Chunk bytes.
        chunk: Bytes,
    },
    /// Corresponds to [`CaptureSink::on_response_end`].
    ResponseEnd {
        /// Event id.
        id: EventId,
        /// How the stream ended.
        outcome: StreamOutcome,
    },
    /// Corresponds to [`CaptureSink::flush`].
    Flush,
}

/// A [`CaptureSink`] that sends every callback as a [`CaptureEvent`] over
/// an unbounded channel.  Useful for integration / assembly tests.
#[derive(Debug, Clone)]
pub struct ChannelSink {
    tx: tokio::sync::mpsc::UnboundedSender<CaptureEvent>,
}

impl ChannelSink {
    /// Create a new `ChannelSink` and its receiving half.
    pub fn new() -> (Self, tokio::sync::mpsc::UnboundedReceiver<CaptureEvent>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

#[async_trait]
impl CaptureSink for ChannelSink {
    async fn on_request_start(&self, ctx: RequestCtx) {
        let _ = self.tx.send(CaptureEvent::RequestStart(ctx));
    }

    async fn on_request_chunk(&self, id: EventId, chunk: Bytes) {
        let _ = self.tx.send(CaptureEvent::RequestChunk { id, chunk });
    }

    async fn on_request_end(&self, id: EventId) {
        let _ = self.tx.send(CaptureEvent::RequestEnd(id));
    }

    async fn on_response_status(&self, id: EventId, status: u16, headers: &HeaderMap) {
        let _ = self.tx.send(CaptureEvent::ResponseStatus { id, status, headers: headers.clone() });
    }

    async fn on_response_chunk(&self, id: EventId, chunk: Bytes) {
        let _ = self.tx.send(CaptureEvent::ResponseChunk { id, chunk });
    }

    async fn on_response_end(&self, id: EventId, outcome: StreamOutcome) {
        let _ = self.tx.send(CaptureEvent::ResponseEnd { id, outcome });
    }

    async fn flush(&self) {
        let _ = self.tx.send(CaptureEvent::Flush);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn null_sink_accepts_all_callbacks() {
        let sink = NullSink;
        let id = EventId::default();
        let ctx = RequestCtx {
            id,
            method: Method::POST,
            uri: Uri::from_static("/v1/chat/completions"),
            headers: HeaderMap::new(),
            route_id: RouteId::from("test"),
            provider: None,
            started_at: Instant::now(),
        };
        sink.on_request_start(ctx).await;
        sink.on_request_chunk(id, Bytes::from_static(b"hello")).await;
        sink.on_request_end(id).await;
        sink.on_response_status(id, 200, &HeaderMap::new()).await;
        sink.on_response_chunk(id, Bytes::from_static(b"world")).await;
        sink.on_response_end(id, StreamOutcome::Complete { ttft_ms: Some(10), ttlt_ms: 100 }).await;
        sink.flush().await;
    }

    #[tokio::test]
    async fn channel_sink_delivers_events_in_order() {
        let (sink, mut rx) = ChannelSink::new();
        let id = EventId::default();
        let ctx = RequestCtx {
            id,
            method: Method::GET,
            uri: Uri::from_static("/test"),
            headers: HeaderMap::new(),
            route_id: RouteId::from("test"),
            provider: None,
            started_at: Instant::now(),
        };

        sink.on_request_start(ctx).await;
        sink.on_request_chunk(id, Bytes::from_static(b"a")).await;
        sink.on_request_end(id).await;
        sink.on_response_status(id, 200, &HeaderMap::new()).await;
        sink.on_response_chunk(id, Bytes::from_static(b"b")).await;
        sink.on_response_end(id, StreamOutcome::Complete { ttft_ms: None, ttlt_ms: 50 }).await;
        sink.flush().await;

        // Drop the sender so the receiver knows when the stream ends.
        drop(sink);

        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
        assert_eq!(events.len(), 7);
        assert!(matches!(events[0], CaptureEvent::RequestStart(_)));
        assert!(matches!(events[1], CaptureEvent::RequestChunk { .. }));
        assert!(matches!(events[2], CaptureEvent::RequestEnd(_)));
        assert!(matches!(events[3], CaptureEvent::ResponseStatus { .. }));
        assert!(matches!(events[4], CaptureEvent::ResponseChunk { .. }));
        assert!(matches!(events[5], CaptureEvent::ResponseEnd { .. }));
        assert!(matches!(events[6], CaptureEvent::Flush));
    }

    #[tokio::test]
    async fn channel_sink_survives_closed_receiver() {
        let (sink, rx) = ChannelSink::new();
        drop(rx);
        let id = EventId::default();
        // Should not panic even though the receiver is gone.
        sink.on_request_chunk(id, Bytes::from_static(b"orphan")).await;
        sink.flush().await;
    }
}
