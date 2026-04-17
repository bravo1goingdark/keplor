//! Proxy-specific error types.

use std::fmt;

/// Errors originating from the proxy layer.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    /// Lower-level hyper error (server-side connections).
    #[error("hyper: {0}")]
    Hyper(#[from] hyper::Error),

    /// hyper-util legacy client error (upstream connections).
    #[error("upstream client: {0}")]
    HyperClient(#[from] hyper_util::client::legacy::Error),

    /// TLS configuration or handshake error.
    #[error("tls: {0}")]
    Tls(#[from] rustls::Error),

    /// Filesystem / network I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// No route matched the incoming `Host` + path.
    #[error("no route for {host}{path}")]
    NoRoute {
        /// The `Host` header value (or `<missing>` if absent).
        host: String,
        /// The request path.
        path: String,
    },

    /// Upstream did not respond within the connect timeout.
    #[error("upstream connect timeout")]
    ConnectTimeout,

    /// The server-wide concurrency limit has been reached.
    #[error("concurrency limit reached ({0})")]
    ConcurrencyLimit(usize),

    /// Configuration loading or validation error.
    #[error("config: {0}")]
    Config(String),

    /// URI construction error.
    #[error("invalid uri: {0}")]
    InvalidUri(InvalidUri),
}

/// Wrapper for [`http::uri::InvalidUri`] that implements [`std::error::Error`].
#[derive(Debug)]
pub struct InvalidUri(pub http::uri::InvalidUri);

impl fmt::Display for InvalidUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for InvalidUri {}

impl From<http::uri::InvalidUri> for ProxyError {
    fn from(e: http::uri::InvalidUri) -> Self {
        Self::InvalidUri(InvalidUri(e))
    }
}
