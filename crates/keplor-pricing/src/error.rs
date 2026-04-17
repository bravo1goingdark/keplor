//! Pricing-subsystem errors.

use std::path::PathBuf;

/// Errors produced by the pricing catalogue and cost engine.
#[derive(Debug, thiserror::Error)]
pub enum PricingError {
    /// The bundled or on-disk JSON could not be deserialised.
    #[error("failed to parse pricing catalog: {reason}")]
    Parse {
        /// Human-readable parse failure description.
        reason: String,
    },

    /// A model key was not found in the catalog (including after fallback).
    #[error("model not found in pricing catalog: {key}")]
    ModelNotFound {
        /// The normalised key that was looked up.
        key: String,
    },

    /// An on-disk catalog file could not be read.
    #[error("failed to read catalog from {path}: {source}")]
    Io {
        /// Path that was attempted.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// Network fetch of a remote catalog failed.
    #[cfg(feature = "fetch")]
    #[error("failed to fetch remote catalog: {source}")]
    Fetch {
        /// Underlying HTTP/network error.
        source: reqwest::Error,
    },
}
