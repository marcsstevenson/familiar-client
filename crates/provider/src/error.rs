// SPDX-License-Identifier: MIT OR Apache-2.0
//! Provider errors.

/// A provider result.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong talking to a mail provider or applying what it said.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The request never got an HTTP answer (DNS, TLS, connection reset, timeout).
    #[error("network error: {0}")]
    Network(#[source] Box<dyn std::error::Error + Send + Sync>),

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

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(Box::new(e))
    }
}

impl From<familiar_auth::AuthError> for Error {
    fn from(e: familiar_auth::AuthError) -> Self {
        match e.kind {
            familiar_auth::AuthErrorKind::Offline => Self::Network(Box::new(e)),
            _ => Self::Auth(e.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use familiar_auth::{AuthError, AuthErrorKind};

    #[test]
    fn token_refresh_distinguishes_offline_from_rejected_credentials() {
        let offline = AuthError::new(AuthErrorKind::Offline, "connection reset");
        assert!(matches!(Error::from(offline), Error::Network(_)));

        let rejected = AuthError::new(AuthErrorKind::AuthRequired, "invalid_grant");
        assert!(matches!(Error::from(rejected), Error::Auth(_)));
    }
}
