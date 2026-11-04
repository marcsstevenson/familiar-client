// SPDX-License-Identifier: MIT OR Apache-2.0
//! The HTTP client: tokens, the scheduler, retries and Graph's error shapes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::scheduler::{Priority, Scheduler};

/// Graph's production endpoint.
pub const GRAPH_V1: &str = "https://graph.microsoft.com/v1.0";

/// Every request asks for immutable IDs, so a message keeps its ID when it
/// moves between folders. The store keys messages by them.
const PREFER_IMMUTABLE: &str = "IdType=\"ImmutableId\"";

/// Gives the client access tokens for one account.
///
/// Both calls may block (they can talk to the identity platform), so the
/// client runs them on Tokio's blocking pool.
pub trait TokenSource: Send + Sync + 'static {
    /// A token that is valid now, refreshed first if it is about to expire.
    fn access_token(&self) -> std::result::Result<String, String>;
    /// A new token, even if the current one looks valid. Called once after a 401.
    fn refresh(&self) -> std::result::Result<String, String>;
}

/// A fixed token, for tests and the mock.
#[derive(Clone, Debug)]
pub struct StaticToken(pub String);

impl TokenSource for StaticToken {
    fn access_token(&self) -> std::result::Result<String, String> {
        Ok(self.0.clone())
    }
    fn refresh(&self) -> std::result::Result<String, String> {
        Ok(self.0.clone())
    }
}

/// Tokens from the WP 1.4 sign-in store for one account.
pub struct AuthTokens {
    auth: Arc<familiar_auth::Auth>,
    account: String,
}

impl AuthTokens {
    /// Tokens for `account` (its key in config.toml).
    pub fn new(auth: Arc<familiar_auth::Auth>, account: impl Into<String>) -> Self {
        Self {
            auth,
            account: account.into(),
        }
    }
}

impl TokenSource for AuthTokens {
    fn access_token(&self) -> std::result::Result<String, String> {
        self.auth
            .access_token(&self.account)
            .map_err(|e| e.to_string())
    }
    fn refresh(&self) -> std::result::Result<String, String> {
        let record = self
            .auth
            .refresh(&self.account)
            .map_err(|e| e.to_string())?;
        record
            .access_token
            .ok_or_else(|| "refresh gave no access token".to_string())
    }
}

/// Client settings.
#[derive(Clone, Debug)]
pub struct ClientOptions {
    /// Graph base URL including the version, e.g. [`GRAPH_V1`].
    pub base_url: String,
    /// Attempts per request when Graph throttles or is unavailable.
    pub max_attempts: u32,
    /// Wait used when a 429 or 503 carries no `Retry-After`; doubled per attempt.
    pub default_retry_after: Duration,
    /// Longest wait honoured from one `Retry-After`.
    pub max_retry_after: Duration,
    /// Timeout for one HTTP exchange.
    pub timeout: Duration,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            base_url: GRAPH_V1.to_string(),
            max_attempts: 6,
            default_retry_after: Duration::from_secs(2),
            max_retry_after: Duration::from_secs(300),
            timeout: Duration::from_secs(60),
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

struct Inner {
    http: reqwest::Client,
    opts: ClientOptions,
    /// `scheme://host[:port]` of the base URL; links elsewhere are refused,
    /// so a token is never sent to another host.
    origin: String,
    tokens: Arc<dyn TokenSource>,
    token: Mutex<Option<String>>,
    scheduler: Scheduler,
}

/// A Graph client for one mailbox. Cheap to clone; clones share the token
/// and the scheduler.
#[derive(Clone)]
pub struct GraphClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for GraphClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphClient")
            .field("base_url", &self.inner.opts.base_url)
            .finish()
    }
}

impl GraphClient {
    /// A client with its own scheduler (one per mailbox).
    pub fn new(opts: ClientOptions, tokens: Arc<dyn TokenSource>) -> Result<Self> {
        Self::with_scheduler(opts, tokens, Scheduler::default())
    }

    /// A client that shares `scheduler`, for several clients on one mailbox.
    pub fn with_scheduler(
        mut opts: ClientOptions,
        tokens: Arc<dyn TokenSource>,
        scheduler: Scheduler,
    ) -> Result<Self> {
        while opts.base_url.ends_with('/') {
            opts.base_url.pop();
        }
        let origin = origin_of(&opts.base_url)
            .ok_or_else(|| Error::Decode(format!("bad Graph base URL {}", opts.base_url)))?;
        let http = reqwest::Client::builder()
            .timeout(opts.timeout)
            .user_agent(concat!("familiar/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                opts,
                origin,
                tokens,
                token: Mutex::new(None),
                scheduler,
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

    fn url(&self, path_or_link: &str) -> Result<String> {
        if path_or_link.starts_with("http://") || path_or_link.starts_with("https://") {
            if origin_of(path_or_link).as_deref() != Some(self.inner.origin.as_str()) {
                return Err(Error::Decode(format!(
                    "refusing to follow a link to another host: {path_or_link}"
                )));
            }
            return Ok(path_or_link.to_string());
        }
        Ok(format!("{}{}", self.inner.opts.base_url, path_or_link))
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
        .map_err(|e| Error::Auth(format!("token task failed: {e}")))?
        .map_err(Error::Auth)?;
        *self.inner.token.lock().unwrap_or_else(|e| e.into_inner()) = Some(t.clone());
        Ok(t)
    }

    /// GETs a path under the base URL (`/me/mailFolders`) or a link Graph
    /// returned (`@odata.nextLink`, `@odata.deltaLink`), and returns the JSON.
    ///
    /// Waits for a scheduler slot at `priority`. Throttling (429) and 503 pause
    /// the whole mailbox for `Retry-After` and retry; a 401 refreshes the token
    /// once and retries. `prefer` adds to the `Prefer` header, which always
    /// asks for immutable IDs.
    pub async fn get(
        &self,
        path_or_link: &str,
        priority: Priority,
        prefer: &[&str],
    ) -> Result<Value> {
        let url = self.url(path_or_link)?;
        let mut prefer_header = PREFER_IMMUTABLE.to_string();
        for p in prefer {
            prefer_header.push_str(", ");
            prefer_header.push_str(p);
        }
        let mut refreshed = false;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let token = self.token(false).await?;
            let permit = self.inner.scheduler.acquire(priority).await;
            let resp = self
                .inner
                .http
                .get(&url)
                .bearer_auth(&token)
                .header("Prefer", &prefer_header)
                .header("Accept", "application/json")
                .send()
                .await?;
            let status = resp.status();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let request_id = resp
                .headers()
                .get("request-id")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let body = resp.bytes().await?;
            drop(permit);

            if status.is_success() {
                return Ok(serde_json::from_slice(&body)?);
            }
            let (code, message) = graph_error(&body);
            match status {
                StatusCode::UNAUTHORIZED if !refreshed => {
                    refreshed = true;
                    attempt -= 1;
                    self.token(true).await?;
                }
                StatusCode::UNAUTHORIZED => {
                    return Err(Error::Auth(format!("{code}: {message}")));
                }
                StatusCode::TOO_MANY_REQUESTS
                | StatusCode::SERVICE_UNAVAILABLE
                | StatusCode::GATEWAY_TIMEOUT => {
                    if attempt >= self.inner.opts.max_attempts {
                        return Err(Error::Throttled {
                            attempts: attempt,
                            code,
                        });
                    }
                    let wait = retry_after
                        .unwrap_or_else(|| {
                            self.inner.opts.default_retry_after * 2u32.saturating_pow(attempt - 1)
                        })
                        .min(self.inner.opts.max_retry_after);
                    self.inner.scheduler.pause_for(wait);
                }
                StatusCode::NOT_FOUND => return Err(Error::NotFound(format!("{url}: {code}"))),
                StatusCode::GONE => return Err(Error::SyncStateGone(format!("{code}: {message}"))),
                _ => {
                    return Err(Error::Graph {
                        status: status.as_u16(),
                        code,
                        message,
                        request_id,
                    });
                }
            }
        }
    }
}

/// `scheme://authority` of a URL.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    (!authority.is_empty()).then(|| {
        format!(
            "{}://{}",
            scheme.to_ascii_lowercase(),
            authority.to_ascii_lowercase()
        )
    })
}

/// Graph's `{"error": {"code", "message"}}`.
fn graph_error(body: &[u8]) -> (String, String) {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let e = &v["error"];
    (
        e["code"].as_str().unwrap_or("Unknown").to_string(),
        e["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| String::from_utf8_lossy(body).chars().take(200).collect()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert_eq!(
            origin_of("https://Graph.Microsoft.com/v1.0/me?x=1").as_deref(),
            Some("https://graph.microsoft.com")
        );
        assert_eq!(
            origin_of("http://127.0.0.1:8787/v1.0").as_deref(),
            Some("http://127.0.0.1:8787")
        );
        assert_eq!(origin_of("nope"), None);
    }

    #[test]
    fn links_to_other_hosts_are_refused() {
        let c = GraphClient::new(
            ClientOptions::with_base("https://graph.microsoft.com/v1.0/"),
            Arc::new(StaticToken("t".into())),
        )
        .unwrap();
        assert_eq!(c.base_url(), "https://graph.microsoft.com/v1.0");
        assert!(c.url("https://evil.example/v1.0/me").is_err());
        assert!(c.url("https://graph.microsoft.com.evil.example/x").is_err());
        assert_eq!(c.url("/me").unwrap(), "https://graph.microsoft.com/v1.0/me");
    }
}
