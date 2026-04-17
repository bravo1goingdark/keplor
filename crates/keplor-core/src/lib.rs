//! # keplor-core
//!
//! The typed backbone every other Keplor crate depends on.  Zero runtime
//! dependencies in the async / I/O sense: no `tokio`, no `hyper`, no
//! `rusqlite`, no network.  This crate is pure data + logic so it stays
//! trivially testable and trivially re-usable.
//!
//! ## Module map
//!
//! | Module          | Purpose                                                      |
//! |-----------------|--------------------------------------------------------------|
//! | [`id`]          | Newtype identifiers ([`EventId`] and friends).               |
//! | [`provider`]    | The [`Provider`] enum + host / path routing.                 |
//! | [`usage`]       | Per-request token counters and merge logic.                  |
//! | [`cost`]        | [`Cost`] — int64 nanodollars with exact display.             |
//! | [`error`]       | [`CoreError`] and the normalised [`ProviderError`].          |
//! | [`event`]       | The canonical [`LlmEvent`] record.                           |
//! | [`payload_ref`] | [`PayloadRef`] — how we point at stored request/response bytes. |
//! | [`flags`]       | [`EventFlags`] — per-event bitflags.                         |
//! | [`sanitize`]    | Whitelist-based header scrubbing for storage.                |

#![deny(missing_docs)]
// Inline unit tests use `.unwrap()` / `.expect()` freely; production code
// still obeys the workspace-level `warn(clippy::unwrap_used)` rule.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod cost;
pub mod error;
pub mod event;
pub mod flags;
pub mod id;
pub mod payload_ref;
pub mod provider;
pub mod sanitize;
pub mod usage;

// Flat re-exports: let downstream crates write `use keplor_core::Usage`
// instead of remembering the module path for common types.
pub use cost::Cost;
pub use error::{CoreError, ProviderError};
pub use event::{Latencies, LlmEvent, TraceId};
pub use flags::EventFlags;
pub use id::{ApiKeyId, EventId, OrgId, ProjectId, ProviderId, RouteId, UserId};
pub use payload_ref::{BlobId, Compression, DictId, PayloadRef, PayloadStorage};
pub use provider::Provider;
pub use sanitize::sanitize_headers;
pub use usage::Usage;
