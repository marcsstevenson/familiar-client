// SPDX-License-Identifier: MIT OR Apache-2.0
//! Injected failures: quota errors, outages, auth errors and delays.

use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FaultKind {
    /// Any Google API error.
    Error {
        status: u16,
        reason: String,
        message: String,
        retry_after: Option<u64>,
    },
    /// Holds the request for a while, then serves it normally.
    Delay(Duration),
}

/// A failure injected into matching requests, including each request inside a batch.
/// Faults are checked in the order they were added; delays add up and the first error
/// answers the request.
///
/// ```
/// use familiar_mock_gmail::Fault;
/// // The next two message fetches get 429 rateLimitExceeded.
/// let fault = Fault::rate_limited().on_path("/messages/").times(2);
/// # let _ = fault;
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub(crate) kind: FaultKind,
    pub(crate) path_contains: Option<String>,
    pub(crate) method: Option<String>,
    pub(crate) remaining: Option<u32>,
}

impl Fault {
    fn error(status: u16, reason: &str, message: &str) -> Self {
        Self {
            kind: FaultKind::Error {
                status,
                reason: reason.into(),
                message: message.into(),
                retry_after: None,
            },
            path_contains: None,
            method: None,
            remaining: None,
        }
    }

    /// 429 `rateLimitExceeded` (`RESOURCE_EXHAUSTED`), Gmail's per-user quota error.
    /// Gmail sends no `Retry-After`; add one with [`Fault::retry_after`].
    pub fn rate_limited() -> Self {
        Self::error(
            429,
            "rateLimitExceeded",
            "Quota exceeded for quota metric 'Queries' and limit 'Queries per minute per user' \
             of service 'gmail.googleapis.com'.",
        )
    }

    /// 403 `userRateLimitExceeded` (`PERMISSION_DENIED`), the older form of the same error.
    pub fn user_rate_limited() -> Self {
        Self::error(403, "userRateLimitExceeded", "User-rate limit exceeded.")
    }

    /// 429 "Too many concurrent requests for user".
    pub fn too_many_concurrent() -> Self {
        Self::error(
            429,
            "rateLimitExceeded",
            "Too many concurrent requests for user.",
        )
    }

    /// 503 `backendError`.
    pub fn unavailable() -> Self {
        Self::error(503, "backendError", "The service is currently unavailable.")
    }

    /// 401 `authError`, as when the access token lapses mid-sync.
    pub fn auth_expired() -> Self {
        Self::error(401, "authError", crate::server::AUTH_MESSAGE)
    }

    /// 403 `insufficientPermissions`, as when a scope was not granted.
    pub fn insufficient_scope() -> Self {
        Self::error(
            403,
            "insufficientPermissions",
            "Request had insufficient authentication scopes.",
        )
    }

    /// Any status with a Google error body.
    pub fn status(status: u16, reason: &str, message: &str) -> Self {
        Self::error(status, reason, message)
    }

    /// Delays matching requests, then serves them normally.
    pub fn delay(delay: Duration) -> Self {
        Self {
            kind: FaultKind::Delay(delay),
            path_contains: None,
            method: None,
            remaining: None,
        }
    }

    /// Adds `Retry-After: secs` to the error.
    pub fn retry_after(mut self, secs: u64) -> Self {
        if let FaultKind::Error { retry_after, .. } = &mut self.kind {
            *retry_after = Some(secs);
        }
        self
    }

    /// Only requests whose path contains `fragment` (case-insensitive).
    pub fn on_path(mut self, fragment: impl Into<String>) -> Self {
        self.path_contains = Some(fragment.into().to_ascii_lowercase());
        self
    }

    /// Only requests with this HTTP method.
    pub fn on_method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into().to_ascii_uppercase());
        self
    }

    /// Fires for `n` matching requests, then removes itself. Without this it fires forever.
    pub fn times(mut self, n: u32) -> Self {
        self.remaining = Some(n);
        self
    }

    pub(crate) fn matches(&self, method: &str, path: &str) -> bool {
        self.method.as_deref().is_none_or(|m| m == method)
            && self
                .path_contains
                .as_deref()
                .is_none_or(|p| path.to_ascii_lowercase().contains(p))
    }
}
