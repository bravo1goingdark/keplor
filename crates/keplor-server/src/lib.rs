//! Keplor ingestion server — receives LLM log events via HTTP, normalises
//! them, computes cost, and stores them with compression and deduplication.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod auth;
pub mod config;
pub mod error;
pub mod idempotency;
pub mod normalize;
pub mod pipeline;
pub mod rate_limit;
pub mod request_id;
pub mod rollup;
pub mod routes;
pub mod schema;
pub mod server;
pub mod validate;

pub use config::ServerConfig;
pub use pipeline::Pipeline;
pub use server::{install_metrics_recorder, PipelineServer};
