// SPDX-License-Identifier: MIT OR Apache-2.0
//! The HTTP client: tokens, the scheduler, the quota budget, retries and
//! Gmail's error shapes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde_json::Value;
use tokio::time::Instant;

use familiar_provider::{Error, Priority, Result, Scheduler, TokenSource};

/// Gmail's production endpoint for the signed-in user.
pub const GMAIL_V1: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

/// Quota units a minute the client spends at most. Google's per-user limit
/// is 15,000 by its quota page; the plan budgets well under it.
pub const DEFAULT_UNITS_PER_MINUTE: u32 = 6_000;

/// Client settings.
#[derive(Clone, Debug)]
pub struct ClientOptions {
    /// Base URL for the user, e.g. [`GMAIL_V1`].
    pub base_url: String,
    /// Attempts per request when Gmail throttles or is unavailable.
    pub max_attempts: u32,
    /// First wait when a throttle or 5xx carries no `Retry-After`; doubled per attempt.
    pub default_retry_after: Duration,
    /// Longest wait honoured from one `Retry-After`.
    pub max_retry_after: Duration,
    /// Timeout for one HTTP exchange.
    pub timeout: Duration,
    /// Quota units a minute.
    pub units_per_minute: u32,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            base_url: GMAIL_V1.to_string(),
            max_attempts: 6,
            default_retry_after: Duration::from_secs(2),
            max_retry_after: Duration::from_secs(300),
            timeout: Duration::from_secs(60),
            units_per_minute: DEFAULT_UNITS_PER_MINUTE,
        }
    }
}

impl ClientOptions {
    /// Default options against another base URL (the mock, for instance).
    pub fn with_base(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::default()
        }
    }
}

/// What a request costs in Gmail quota units, from Google's quota table.
pub fn units(method: &Method, path: &str) -> u32 {
    let path = path.split('?').next().unwrap_or(path);
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    match (method.as_str(), segs.as_slice()) {
        (_, ["history"]) => 2,
        ("GET", ["labels", ..]) | ("GET", ["profile"]) => 1,
        ("POST", ["messages", "send"]) | ("POST", ["drafts", "send"]) => 100,
        ("POST", ["messages", "batchModify"]) | ("POST", ["messages", "batchDelete"]) => 50,
        (_, ["drafts", ..]) | (_, ["threads", ..]) => 10,
        (_, ["settings", ..]) => 1,
        (_, ["labels", ..]) => 5,
        _ => 5,
    }
}

/// A token bucket in quota units: full at the start, refilled evenly over a
/// minute.
#[derive(Debug)]
struct Quota {
    per_minute: f64,
    available: f64,
    at: Instant,
}

impl Quota {
    fn new(per_minute: u32) -> Self {
        let per_minute = f64::from(per_minute.max(1));
        Self {
            per_minute,
            available: per_minute,
            at: Instant::now(),
        }
    }

    /// Takes `cost` units and says how long to wait before sending.
    fn take(&mut self, cost: u32) -> Duration {
        let now = Instant::now();
        let refill = now.duration_since(self.at).as_secs_f64() * self.per_minute / 60.0;
        self.available = (self.available + refill).min(self.per_minute);
        self.at = now;
        self.available -= f64::from(cost);
        if self.available >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-self.available * 60.0 / self.per_minute)
        }
    }
}

struct Inner {
    http: reqwest::Client,
    opts: ClientOptions,
    tokens: Arc<dyn TokenSource>,
    token: Mutex<Option<String>>,
    scheduler: Scheduler,
    quota: Mutex<Quota>,
}

/// A Gmail client for one mailbox. Cheap to clone; clones share the token,
/// the scheduler and the quota budget.
#[derive(Clone)]
pub struct GmailClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for GmailClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GmailClient")
            .field("base_url", &self.inner.opts.base_url)
            .finish()
    }
}

impl GmailClient {
    /// A client with its own scheduler (one per mailbox).
    pub fn new(mut opts: ClientOptions, tokens: Arc<dyn TokenSource>) -> Result<Self> {
        while opts.base_url.ends_with('/') {
            opts.base_url.pop();
        }
        if !(opts.base_url.starts_with("https://") || opts.base_url.starts_with("http://")) {
            return Err(Error::Decode(format!(
                "bad Gmail base URL {}",
                opts.base_url
            )));
        }
        let http = reqwest::Client::builder()
            .timeout(opts.timeout)
            .user_agent(concat!("familiar/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let quota = Mutex::new(Quota::new(opts.units_per_minute));
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                opts,
                tokens,
                token: Mutex::new(None),
                scheduler: Scheduler::default(),
                quota,
            }),
        })
    }

    /// The mailbox's scheduler.
    pub fn scheduler(&self) -> &Scheduler {
        &self.inner.scheduler
    }

    /// The base URL, without a trailing slash.
    pub fn base_url(&self) -> &str {
        &self.inner.opts.base_url
    }

    async fn token(&self, refresh: bool) -> Result<String> {
        if !refresh {
            if let Some(t) = self
                .inner
                .token
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                return Ok(t);
            }
        }
        let src = self.inner.tokens.clone();
        let t = tokio::task::spawn_blocking(move || {
            if refresh {
                src.refresh()
            } else {
                src.access_token()
            }
        })
        .await
        .map_err(|e| Error::Auth(format!("token task failed: {e}")))??;
        *self.inner.token.lock().unwrap_or_else(|e| e.into_inner()) = Some(t.clone());
        Ok(t)
    }

    /// GETs a path under the base URL (`/labels`, `/messages?q=...`) and
    /// returns the JSON.
    pub async fn get(&self, path: &str, priority: Priority) -> Result<Value> {
        self.request(Method::GET, path, None, priority).await
    }

    /// POSTs `body` as JSON and returns the answer (`Value::Null` for 204).
    pub async fn post(&self, path: &str, body: &Value, priority: Priority) -> Result<Value> {
        self.request(Method::POST, path, Some(body), priority).await
    }

    /// One request with retries. Throttling (429, or 403 with a rate-limit
    /// reason) pauses the whole mailbox and retries, writes included, since
    /// Gmail did nothing. A 401 refreshes the token once. A 5xx is retried
    /// for reads only.
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        priority: Priority,
    ) -> Result<Value> {
        let url = format!("{}{}", self.inner.opts.base_url, path);
        let cost = units(&method, path);
        let mut refreshed = false;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let token = self.token(false).await?;
            let wait = self
                .inner
                .quota
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take(cost);
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
            let permit = self.inner.scheduler.acquire(priority).await;
            let mut req = self
                .inner
                .http
                .request(method.clone(), &url)
                .bearer_auth(&token)
                .header("Accept", "application/json");
            if let Some(b) = body {
                req = req.json(b);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let bytes = resp.bytes().await?;
            drop(permit);

            if status.is_success() {
                if status == StatusCode::NO_CONTENT || bytes.is_empty() {
                    return Ok(Value::Null);
                }
                return Ok(serde_json::from_slice(&bytes)?);
            }
            let (reason, message) = gmail_error(&bytes);
            let throttled = status == StatusCode::TOO_MANY_REQUESTS
                || (status == StatusCode::FORBIDDEN
                    && matches!(
                        reason.as_str(),
                        "rateLimitExceeded" | "userRateLimitExceeded"
                    ));
            let retry_5xx = method == Method::GET && status.is_server_error();
            match status {
                StatusCode::UNAUTHORIZED if !refreshed => {
                    refreshed = true;
                    attempt -= 1;
                    self.token(true).await?;
                }
                StatusCode::UNAUTHORIZED => {
                    return Err(Error::Auth(format!("{reason}: {message}")));
                }
                _ if throttled || retry_5xx => {
                    if attempt >= self.inner.opts.max_attempts {
                        return Err(if throttled {
                            Error::Throttled {
                                attempts: attempt,
                                code: reason,
                            }
                        } else {
                            Error::Graph {
                                status: status.as_u16(),
                                code: reason,
                                message,
                                request_id: None,
                            }
                        });
                    }
                    let wait = retry_after
                        .unwrap_or_else(|| {
                            self.inner.opts.default_retry_after * 2u32.saturating_pow(attempt - 1)
                        })
                        .min(self.inner.opts.max_retry_after);
                    if throttled {
                        self.inner.scheduler.pause_for(wait);
                    } else {
                        tokio::time::sleep(wait).await;
                    }
                }
                StatusCode::NOT_FOUND => return Err(Error::NotFound(format!("{path}: {message}"))),
                _ => {
                    return Err(Error::Graph {
                        status: status.as_u16(),
                        code: reason,
                        message,
                        request_id: None,
                    });
                }
            }
        }
    }
}

/// Gmail's `{"error": {"code", "message", "errors": [{"reason"}], "status"}}`:
/// the first reason (else the status), and the message.
fn gmail_error(body: &[u8]) -> (String, String) {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let e = &v["error"];
    let reason = e["errors"][0]["reason"]
        .as_str()
        .or_else(|| e["status"].as_str())
        .unwrap_or("unknown")
        .to_string();
    let message = e["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| String::from_utf8_lossy(body).chars().take(200).collect());
    (reason, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs() {
        assert_eq!(units(&Method::GET, "/history?startHistoryId=1"), 2);
        assert_eq!(units(&Method::GET, "/messages/abc?format=raw"), 5);
        assert_eq!(units(&Method::POST, "/messages/send"), 100);
        assert_eq!(units(&Method::GET, "/labels"), 1);
    }

    #[test]
    fn errors() {
        let b = br#"{"error":{"code":403,"message":"slow down","errors":[{"reason":"userRateLimitExceeded"}],"status":"PERMISSION_DENIED"}}"#;
        assert_eq!(
            gmail_error(b),
            ("userRateLimitExceeded".into(), "slow down".into())
        );
        let b = br#"{"error":{"code":404,"message":"gone","status":"NOT_FOUND"}}"#;
        assert_eq!(gmail_error(b).0, "NOT_FOUND");
    }

    #[tokio::test]
    async fn quota_waits_once_the_bucket_is_empty() {
        let mut q = Quota::new(60);
        assert!(q.take(60).is_zero());
        let w = q.take(1);
        assert!(w > Duration::from_millis(900) && w <= Duration::from_secs(1));
    }
}
