// SPDX-License-Identifier: MIT OR Apache-2.0
//! Injected failures: throttling, outages, auth errors and delays.

use std::time::Duration;

/// What an injected fault does to a matching request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FaultKind {
    /// 429 Too Many Requests with `Retry-After`.
    Throttle { retry_after: u64 },
    /// 503 Service Unavailable with `Retry-After`.
    Unavailable { retry_after: u64 },
    /// 401 with an expired-token error, as when an access token lapses mid-sync.
    AuthExpired,
    /// Any Graph error.
    Status {
        status: u16,
        code: String,
        message: String,
    },
    /// Holds the request for a while, then serves it normally.
    Delay(Duration),
}

/// A failure injected into matching requests. Faults are checked in the order they were
/// added; delays add up and the first terminal fault answers the request.
///
/// ```
/// use familiar_mock_graph::Fault;
/// // The next two requests for messages get 429 with Retry-After: 1.
/// let fault = Fault::throttle(1).on_path("/messages").times(2);
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
    fn new(kind: FaultKind) -> Self {
        Self {
            kind,
            path_contains: None,
            method: None,
            remaining: None,
        }
    }

    /// 429 with `Retry-After: retry_after_secs` and Graph's `TooManyRequests` error.
    pub fn throttle(retry_after_secs: u64) -> Self {
        Self::new(FaultKind::Throttle {
            retry_after: retry_after_secs,
        })
    }

    /// 503 with `Retry-After: retry_after_secs`.
    pub fn service_unavailable(retry_after_secs: u64) -> Self {
        Self::new(FaultKind::Unavailable {
            retry_after: retry_after_secs,
        })
    }

    /// 401 `InvalidAuthenticationToken` (expired token) with a `WWW-Authenticate` header.
    pub fn auth_expired() -> Self {
        Self::new(FaultKind::AuthExpired)
    }

    /// 403 `ErrorAccessDenied`, as when consent or a scope is missing.
    pub fn access_denied() -> Self {
        Self::status(
            403,
            "ErrorAccessDenied",
            "Access is denied. Check credentials and try again.",
        )
    }

    /// Any status with a Graph error body.
    pub fn status(status: u16, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(FaultKind::Status {
            status,
            code: code.into(),
            message: message.into(),
        })
    }

    /// Delays matching requests and then serves them normally.
    pub fn delay(delay: Duration) -> Self {
        Self::new(FaultKind::Delay(delay))
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
