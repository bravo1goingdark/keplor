//! The proxy core: listener, upstream client, and the body-tee pattern that
//! clones every `Bytes` frame onto a bounded capture channel without
//! buffering the forwarded body.
//!
//! # Architecture
//!
//! The proxy receives an HTTPS request, looks up the matching route, then
//! streams the request body to the upstream while teeing every `Bytes` frame
//! into a bounded capture channel.  The response is streamed back in the
//! same fashion.  If the capture channel fills, the tee drops capture
//! (incrementing `keplor_capture_dropped_total`) rather than blocking the
//! forwarded byte stream.

#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod capture;
pub mod config;
mod error;
pub mod headers;
pub mod limits;
pub mod route;
pub mod server;
pub mod tee;
pub mod upstream;

pub use capture::{CaptureEvent, CaptureSink, ChannelSink, NullSink, RequestCtx, StreamOutcome};
pub use config::ProxyConfig;
pub use error::ProxyError;
pub use limits::ConcurrencyLimiter;
pub use route::{Route, RouteTable};
pub use server::ProxyServer;
pub use tee::TeeBody;
pub use upstream::UpstreamPool;
