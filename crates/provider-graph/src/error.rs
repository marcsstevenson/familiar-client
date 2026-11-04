// SPDX-License-Identifier: MIT OR Apache-2.0
//! Provider errors.

/// A provider result.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong talking to Graph or applying what it said.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The request never got an HTTP answer (DNS, TLS, connection reset, timeout).
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    /// No usable access token, even after a refresh. The account must sign in again.
    #[error("not signed in: {0}")]
    Auth(String),

    /// Graph kept throttling after every retry.
    #[error("Graph is still throttling after {attempts} attempts ({code})")]
    Throttled {
        /// Attempts made.
        attempts: u32,
        /// Graph's error code, e.g. `ApplicationThrottled`.
        code: String,
    },

    /// The item doesn't exist (any more).
    #[error("not found: {0}")]
    NotFound(String),

    /// A delta or skip link is no longer valid (410 `SyncStateNotFound`).
    /// The resource must be synced from scratch.
    #[error("sync state expired: {0}")]
    SyncStateGone(String),

    /// Any other Graph error.
    #[error("Graph {status} {code}: {message}")]
    Graph {
        /// HTTP status.
        status: u16,
        /// Graph's error code.
        code: String,
        /// Graph's message.
        message: String,
        /// The `request-id` header, for support.
        request_id: Option<String>,
    },

    /// Graph answered with something we couldn't read.
    #[error("unexpected response from Graph: {0}")]
    Decode(String),

    /// The local store refused a write.
    #[error(transparent)]
    Store(#[from] familiar_store::Error),
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Decode(e.to_string())
    }
}
