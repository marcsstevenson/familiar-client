// SPDX-License-Identifier: MIT OR Apache-2.0
//! The HTTP server: sign-in, bearer auth, faults, quota, concurrency, batch and routing.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State as AxState};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::api::{self, ApiError, Reply};
use crate::auth::{Auth, AuthReply, Users};
use crate::faults::{Fault, FaultKind};
use crate::mime;
use crate::model::{Label, Mailbox, MailboxState, Message, MessageInfo, MutationError};

pub(crate) const AUTH_MESSAGE: &str = "Request had invalid authentication credentials. \
    Expected OAuth 2 access token, login cookie or other valid authentication credential.";

/// Gmail answers batches of more than 100 requests with 400.
const MAX_BATCH: usize = 100;

/// A request the mock answered, for test assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoggedRequest {
    /// HTTP method.
    pub method: String,
    /// Path, for example `/gmail/v1/users/me/history`.
    pub path: String,
    /// Raw query string.
    pub query: String,
    /// Mailbox the token belonged to, if it was valid.
    pub mailbox: Option<String>,
    /// Response status.
    pub status: u16,
    /// Request body as text (empty for reads).
    pub body: String,
    /// Sent inside a `/batch/gmail/v1` request.
    pub in_batch: bool,
}

struct Budget {
    units: u64,
    window: Duration,
    used: HashMap<String, (Instant, u64)>,
}

struct State {
    mailboxes: HashMap<String, MailboxState>,
    auth: Auth,
    faults: Vec<Fault>,
    log: Vec<LoggedRequest>,
    latency: Duration,
    max_concurrent: Option<usize>,
    budget: Option<Budget>,
}

struct Shared {
    state: Mutex<State>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Configures a [`MockGmail`] before it starts.
pub struct Builder {
    mailboxes: Vec<(String, Mailbox)>,
    tokens: Vec<(String, String)>,
    latency: Duration,
    max_concurrent: Option<usize>,
    quota: Option<(u64, Duration)>,
    port: u16,
    token_lifetime: Duration,
    client: Option<(String, String)>,
}

impl Builder {
    /// Adds a mailbox under `key`, reachable with the bearer token `"<key>-token"`.
    pub fn mailbox(mut self, key: impl Into<String>, mailbox: Mailbox) -> Self {
        let key = key.into();
        self.tokens.push((format!("{key}-token"), key.clone()));
        self.mailboxes.push((key, mailbox));
        self
    }

    /// Adds another valid bearer token for the mailbox `key`.
    pub fn token(mut self, token: impl Into<String>, key: impl Into<String>) -> Self {
        self.tokens.push((token.into(), key.into()));
        self
    }

    /// Delay added to every request while it counts as in flight. Defaults to none.
    pub fn latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    /// Concurrent requests allowed per mailbox before 429 "Too many concurrent requests
    /// for user". Defaults to `None` (no limit); requests inside a batch don't count.
    pub fn max_concurrent(mut self, limit: Option<usize>) -> Self {
        self.max_concurrent = limit;
        self
    }

    /// Quota units per mailbox per `window` (Gmail's is 15,000 per minute per user, scaled
    /// down for tests). A request that would go over gets 429 `rateLimitExceeded` with no
    /// `Retry-After`, as from Gmail. Costs follow Google's table: `messages.get` 5,
    /// `history.list` 2, `messages.send` 100, and so on.
    pub fn quota(mut self, units: u64, window: Duration) -> Self {
        self.quota = Some((units, window));
        self
    }

    /// Lifetime of access tokens from the token endpoint. Defaults to an hour.
    pub fn token_lifetime(mut self, lifetime: Duration) -> Self {
        self.token_lifetime = lifetime;
        self
    }

    /// Only this OAuth client may sign in, and the token endpoint checks its secret.
    /// By default any client ID is accepted without a secret.
    pub fn client(mut self, id: impl Into<String>, secret: impl Into<String>) -> Self {
        self.client = Some((id.into(), secret.into()));
        self
    }

    /// Listens on this port instead of a free one.
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Starts the server on `127.0.0.1`. Must be called inside a Tokio runtime.
    pub async fn start(mut self) -> std::io::Result<MockGmail> {
        if self.mailboxes.is_empty() {
            self = self.mailbox("default", Mailbox::small());
            self.tokens.push(("test-token".into(), "default".into()));
        }
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", self.port)).await?;
        let addr = listener.local_addr()?;
        let mut auth = Auth::new(self.token_lifetime, self.client);
        for (t, k) in self.tokens {
            auth.add_static(t, k);
        }
        let state = State {
            mailboxes: self
                .mailboxes
                .into_iter()
                .map(|(k, m)| (k, MailboxState::new(m)))
                .collect(),
            auth,
            faults: Vec::new(),
            log: Vec::new(),
            latency: self.latency,
            max_concurrent: self.max_concurrent,
            budget: self.quota.map(|(units, window)| Budget {
                units,
                window,
                used: HashMap::new(),
            }),
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
        });
        let app = Router::new().fallback(dispatch).with_state(shared.clone());
        let (tx, rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });
        Ok(MockGmail {
            addr,
            shared,
            shutdown: Some(tx),
        })
    }
}

/// A running mock Gmail API and Google sign-in on `127.0.0.1`. It stops when dropped.
///
/// ```no_run
/// # async fn demo() -> std::io::Result<()> {
/// use familiar_mock_gmail::{Mailbox, MockGmail};
/// let mock = MockGmail::builder().mailbox("gmail", Mailbox::small()).start().await?;
/// let url = format!("{}/users/me/labels", mock.api_url());
/// // GET it with `Authorization: Bearer gmail-token`.
/// # let _ = url; Ok(()) }
/// ```
pub struct MockGmail {
    addr: SocketAddr,
    shared: Arc<Shared>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Drop for MockGmail {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl MockGmail {
    /// A builder with no mailboxes. Starting it without any adds `default`
    /// ([`Mailbox::small`]) with the tokens `default-token` and `test-token`.
    pub fn builder() -> Builder {
        Builder {
            mailboxes: Vec::new(),
            tokens: Vec::new(),
            latency: Duration::ZERO,
            max_concurrent: None,
            quota: None,
            port: 0,
            token_lifetime: Duration::from_secs(3600),
            client: None,
        }
    }

    /// Starts the default server: the small fixture as mailbox `default`, token `test-token`.
    pub async fn start() -> std::io::Result<Self> {
        Self::builder().start().await
    }

    /// `http://127.0.0.1:<port>`: stands in for `https://gmail.googleapis.com`,
    /// `https://oauth2.googleapis.com` and `https://accounts.google.com` alike.
    pub fn origin(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// `http://127.0.0.1:<port>/gmail/v1`, the API base. Paths go on as `/users/me/...`.
    pub fn api_url(&self) -> String {
        format!("{}/gmail/v1", self.origin())
    }

    /// `http://127.0.0.1:<port>/upload/gmail/v1`, for media and multipart uploads.
    pub fn upload_url(&self) -> String {
        format!("{}/upload/gmail/v1", self.origin())
    }

    /// `http://127.0.0.1:<port>/batch/gmail/v1`, the batch endpoint.
    pub fn batch_url(&self) -> String {
        format!("{}/batch/gmail/v1", self.origin())
    }

    /// The authorisation endpoint, in place of `https://accounts.google.com/o/oauth2/v2/auth`.
    pub fn auth_url(&self) -> String {
        format!("{}/o/oauth2/v2/auth", self.origin())
    }

    /// The token endpoint, in place of `https://oauth2.googleapis.com/token`.
    pub fn token_url(&self) -> String {
        format!("{}/token", self.origin())
    }

    /// The revocation endpoint, in place of `https://oauth2.googleapis.com/revoke`.
    pub fn revoke_url(&self) -> String {
        format!("{}/revoke", self.origin())
    }

    /// The listening address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Makes mailbox `key`'s user the one signed in to the "browser": the authorisation
    /// endpoint signs in as them unless `login_hint` names another. `None` signs out, so
    /// it redirects with `error=access_denied`, as when the user cancels.
    pub fn sign_in_as(&self, key: Option<&str>) {
        self.shared.lock().auth.sign_in_as(key.map(str::to_string));
    }

    /// Revokes mailbox `key`'s refresh tokens, as a password change or removing the app's
    /// access would. Refreshing then fails with `invalid_grant`.
    pub fn revoke_refresh_tokens(&self, key: &str) {
        self.shared.lock().auth.revoke(key);
    }

    /// Makes every access token of mailbox `key` answer 401.
    pub fn expire_access_tokens(&self, key: &str) {
        self.shared.lock().auth.expire_access(key);
    }

    /// Makes one bearer token answer 401.
    pub fn expire_token(&self, token: &str) {
        if let Some(t) = self.shared.lock().auth.tokens.get_mut(token) {
            t.expired = true;
        }
    }

    /// Adds a valid bearer token for mailbox `key`.
    pub fn add_token(&self, token: &str, key: &str) {
        self.shared.lock().auth.add_static(token.into(), key.into());
    }

    /// Changes the mailbox `key`. Panics if there is no such mailbox.
    pub fn mailbox(&self, key: &str) -> MailboxHandle<'_> {
        assert!(
            self.shared.lock().mailboxes.contains_key(key),
            "no mailbox '{key}'"
        );
        MailboxHandle {
            shared: &self.shared,
            key: key.to_string(),
        }
    }

    /// Injects a fault. See [`Fault`].
    pub fn push_fault(&self, fault: Fault) {
        self.shared.lock().faults.push(fault);
    }

    /// Removes all injected faults.
    pub fn clear_faults(&self) {
        self.shared.lock().faults.clear();
    }

    /// Sets the per-request latency.
    pub fn set_latency(&self, latency: Duration) {
        self.shared.lock().latency = latency;
    }

    /// Every request served so far, oldest first. Requests inside a batch are listed
    /// after the batch request itself.
    pub fn requests(&self) -> Vec<LoggedRequest> {
        self.shared.lock().log.clone()
    }

    /// Forgets the request log.
    pub fn clear_requests(&self) {
        self.shared.lock().log.clear();
    }
}

/// Changes one mailbox on a running [`MockGmail`]. Every change is written to history,
/// so `history.list` sees it.
pub struct MailboxHandle<'a> {
    shared: &'a Shared,
    key: String,
}

impl MailboxHandle<'_> {
    fn with<R>(&self, f: impl FnOnce(&mut MailboxState) -> R) -> R {
        let mut st = self.shared.lock();
        f(st.mailboxes.get_mut(&self.key).expect("mailbox exists"))
    }

    /// Delivers a message and returns its ID.
    pub fn add_message(&self, msg: Message) -> Result<String, MutationError> {
        self.with(|m| m.insert(msg))
    }

    /// Adds and removes label IDs on a message, as `messages.modify` does.
    pub fn modify(&self, id: &str, add: &[&str], remove: &[&str]) -> Result<(), MutationError> {
        let own = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        self.with(|m| m.modify(id, &own(add), &own(remove)))
    }

    /// Deletes a message for good.
    pub fn delete_message(&self, id: &str) -> Result<(), MutationError> {
        self.with(|m| m.delete(id))
    }

    /// A message's current state.
    pub fn message(&self, id: &str) -> Option<MessageInfo> {
        self.with(|m| m.messages.get(id).map(|s| s.info()))
    }

    /// IDs of the messages with a label, newest first.
    pub fn message_ids(&self, label: &str) -> Vec<String> {
        self.with(|m| {
            m.ordered()
                .into_iter()
                .filter(|s| s.has(label))
                .map(|s| s.id.clone())
                .collect()
        })
    }

    /// Number of messages in the mailbox.
    pub fn message_count(&self) -> usize {
        self.with(|m| m.messages.len())
    }

    /// Adds a user label and returns its ID.
    pub fn add_label(&self, label: Label) -> Result<String, MutationError> {
        self.with(|m| m.add_label(label))
    }

    /// Renames a user label. Like Gmail, children (`Parent/Child`) keep their names.
    pub fn rename_label(&self, id: &str, name: &str) -> Result<(), MutationError> {
        self.with(|m| m.update_label(id, |l| l.name = name.to_string()))
    }

    /// Deletes a user label, taking it off its messages.
    pub fn delete_label(&self, id: &str) -> Result<(), MutationError> {
        self.with(|m| m.delete_label(id))
    }

    /// Every label, system ones first.
    pub fn labels(&self) -> Vec<Label> {
        self.with(|m| m.labels.clone())
    }

    /// Draft IDs and their current message IDs.
    pub fn drafts(&self) -> Vec<(String, String)> {
        self.with(|m| {
            m.drafts
                .iter()
                .map(|(d, id)| (d.clone(), id.clone()))
                .collect()
        })
    }

    /// Messages sent through `messages.send` or `drafts.send`, oldest first.
    pub fn sent(&self) -> Vec<MessageInfo> {
        self.with(|m| {
            m.sent
                .iter()
                .filter_map(|id| m.messages.get(id).map(|s| s.info()))
                .collect()
        })
    }

    /// The mailbox's current history ID.
    pub fn history_id(&self) -> u64 {
        self.with(|m| m.history_id)
    }

    /// Forgets all history so far, as Gmail does after about a week: `history.list` from
    /// any earlier history ID then answers 404.
    pub fn expire_history(&self) {
        self.with(|m| m.expire_history())
    }

    /// Quota units charged to this mailbox so far.
    pub fn units_used(&self) -> u64 {
        self.with(|m| m.units_used)
    }

    /// The most requests seen in flight at once.
    pub fn peak_in_flight(&self) -> usize {
        self.with(|m| m.peak_in_flight)
    }
}

// ---------------------------------------------------------------------------
// Dispatch

fn json_response(status: u16, body: &Value, headers: &[(String, String)]) -> Response {
    let mut resp = Response::new(Body::from(body.to_string()));
    *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    resp.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("application/json; charset=UTF-8"),
    );
    add_headers(&mut resp, headers);
    resp
}

fn add_headers(resp: &mut Response, headers: &[(String, String)]) {
    for (k, v) in headers {
        if let (Ok(k), Ok(v)) = (
            axum::http::HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            resp.headers_mut().insert(k, v);
        }
    }
}

/// A served request before it becomes an HTTP response, so a batch can wrap it.
struct Out {
    status: u16,
    body: Option<Value>,
    /// A non-JSON body (the batch response).
    text: Option<String>,
    headers: Vec<(String, String)>,
}

impl Out {
    fn error(e: &ApiError) -> Self {
        Self {
            status: e.status,
            body: Some(e.body()),
            text: None,
            headers: e.headers.clone(),
        }
    }

    fn into_response(self) -> Response {
        match (self.body, self.text) {
            (Some(b), _) => json_response(self.status, &b, &self.headers),
            (None, text) => {
                let mut r = Response::new(text.map(Body::from).unwrap_or_else(Body::empty));
                *r.status_mut() = StatusCode::from_u16(self.status).unwrap_or(StatusCode::OK);
                add_headers(&mut r, &self.headers);
                r
            }
        }
    }
}

fn params(query: &str) -> Vec<(String, String)> {
    form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

async fn dispatch(AxState(shared): AxState<Arc<Shared>>, req: Request) -> Response {
    let method = req.method().as_str().to_ascii_uppercase();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let headers = req.headers().clone();
    let body = axum::body::to_bytes(req.into_body(), 64 << 20)
        .await
        .unwrap_or_default();

    if matches!(path.as_str(), "/o/oauth2/v2/auth" | "/token" | "/revoke") {
        let mut p = params(&query);
        if method == "POST" {
            p.extend(params(&String::from_utf8_lossy(&body)));
        }
        let mut st = shared.lock();
        let users: Users = st
            .mailboxes
            .iter()
            .map(|(k, m)| (k.clone(), (m.email.clone(), m.name.clone())))
            .collect();
        let reply = crate::auth::handle(&mut st.auth, &users, &path, &p);
        let (status, resp) = match reply {
            AuthReply::Json(s, v) => (s, json_response(s, &v, &[])),
            AuthReply::Redirect(url) => {
                let mut r = Response::new(Body::empty());
                *r.status_mut() = StatusCode::FOUND;
                if let Ok(v) = HeaderValue::from_str(&url) {
                    r.headers_mut().insert("location", v);
                }
                (302, r)
            }
        };
        st.log.push(LoggedRequest {
            method,
            path,
            query,
            mailbox: None,
            status,
            body: String::new(),
            in_batch: false,
        });
        return resp;
    }

    let token = bearer(&headers);
    let content_type = header_str(&headers, "content-type");
    let (out, mailbox) = if path == "/batch/gmail/v1" && method == "POST" {
        batch(&shared, &token, &content_type, &body).await
    } else {
        serve(
            &shared,
            &method,
            &path,
            &query,
            &token,
            &content_type,
            &body,
            false,
        )
        .await
    };
    shared.lock().log.push(LoggedRequest {
        method,
        path,
        query,
        mailbox,
        status: out.status,
        body: String::from_utf8_lossy(&body).into_owned(),
        in_batch: false,
    });
    out.into_response()
}

fn header_str(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

fn bearer(headers: &HeaderMap) -> String {
    let v = header_str(headers, "authorization");
    v.strip_prefix("Bearer ")
        .or_else(|| v.strip_prefix("bearer "))
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Decrements the mailbox's in-flight count when the request finishes.
struct InFlight {
    shared: Arc<Shared>,
    mailbox: String,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(m) = self.shared.lock().mailboxes.get_mut(&self.mailbox) {
            m.in_flight = m.in_flight.saturating_sub(1);
        }
    }
}

/// An admitted request: mailbox key, delay to apply, in-flight guard.
type Admitted = (String, Duration, Option<InFlight>);
/// A refused request: delay to apply first, mailbox key if known, the error.
type Refused = (Duration, Option<String>, ApiError);

/// Faults, auth, quota and concurrency, under the lock. Returns the mailbox key, the delay
/// to apply, and the in-flight guard (none for requests inside a batch).
fn admit(
    shared: &Arc<Shared>,
    method: &str,
    path: &str,
    token: &str,
    units: u64,
    counts_in_flight: bool,
) -> Result<Admitted, Refused> {
    let mut st = shared.lock();
    let mut delay = if counts_in_flight {
        st.latency
    } else {
        Duration::ZERO
    };
    let mut terminal = None;
    let mut i = 0;
    while i < st.faults.len() {
        if !st.faults[i].matches(method, path) {
            i += 1;
            continue;
        }
        let kind = st.faults[i].kind.clone();
        match st.faults[i].remaining.as_mut() {
            Some(n) => {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    st.faults.remove(i);
                } else {
                    i += 1;
                }
            }
            None => i += 1,
        }
        match kind {
            FaultKind::Delay(d) => delay += d,
            FaultKind::Error {
                status,
                reason,
                message,
                retry_after,
            } => {
                let mut e = ApiError::new(status, &reason, message);
                if status == 401 {
                    e.headers.push(www_authenticate());
                }
                if let Some(s) = retry_after {
                    e.headers.push(("Retry-After".into(), s.to_string()));
                }
                terminal = Some(e);
                break;
            }
        }
    }
    if let Some(e) = terminal {
        return Err((delay, None, e));
    }

    let Some(mailbox) = (!token.is_empty()).then(|| st.auth.check(token)).flatten() else {
        let mut e = ApiError::new(401, "authError", AUTH_MESSAGE);
        e.headers.push(www_authenticate());
        return Err((Duration::ZERO, None, e));
    };

    if let Some(b) = st.budget.as_mut() {
        let now = Instant::now();
        let entry = b.used.entry(mailbox.clone()).or_insert((now, 0));
        if now.duration_since(entry.0) >= b.window {
            *entry = (now, 0);
        }
        if entry.1 + units > b.units {
            let e = ApiError::new(
                429,
                "rateLimitExceeded",
                "Quota exceeded for quota metric 'Queries' and limit 'Queries per minute per \
                 user' of service 'gmail.googleapis.com'.",
            );
            return Err((Duration::ZERO, Some(mailbox), e));
        }
        entry.1 += units;
    }

    let max = st.max_concurrent;
    let Some(mbx) = st.mailboxes.get_mut(&mailbox) else {
        return Err((
            Duration::ZERO,
            Some(mailbox),
            ApiError::new(400, "failedPrecondition", "Mail service not enabled"),
        ));
    };
    mbx.units_used += units;
    if !counts_in_flight {
        return Ok((mailbox, delay, None));
    }
    if max.is_some_and(|m| mbx.in_flight >= m) {
        let e = ApiError::new(
            429,
            "rateLimitExceeded",
            "Too many concurrent requests for user.",
        );
        return Err((Duration::ZERO, Some(mailbox), e));
    }
    mbx.in_flight += 1;
    mbx.peak_in_flight = mbx.peak_in_flight.max(mbx.in_flight);
    let guard = InFlight {
        shared: shared.clone(),
        mailbox: mailbox.clone(),
    };
    Ok((mailbox, delay, Some(guard)))
}

fn www_authenticate() -> (String, String) {
    (
        "WWW-Authenticate".into(),
        "Bearer realm=\"https://accounts.google.com/\", error=\"invalid_token\"".into(),
    )
}

/// Splits `/gmail/v1/users/{userId}/rest` (or the `/upload` form) into the user and rest.
fn split_api_path(path: &str) -> Option<(bool, &str, &str)> {
    let (upload, tail) = if let Some(t) = path.strip_prefix("/upload/gmail/v1/users/") {
        (true, t)
    } else {
        (false, path.strip_prefix("/gmail/v1/users/")?)
    };
    let (user, rest) = tail.split_once('/').unwrap_or((tail, ""));
    Some((upload, user, rest))
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    shared: &Arc<Shared>,
    method: &str,
    path: &str,
    query: &str,
    token: &str,
    content_type: &str,
    body: &[u8],
    in_batch: bool,
) -> (Out, Option<String>) {
    let Some((upload, user, rest)) = split_api_path(path) else {
        let e = ApiError::new(404, "notFound", format!("The mock does not serve {path}"));
        return (Out::error(&e), None);
    };
    let units = u64::from(api::cost(method, rest));
    let (mailbox, delay, guard) = match admit(shared, method, path, token, units, !in_batch) {
        Ok(ok) => ok,
        Err((delay, mailbox, e)) => {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            return (Out::error(&e), mailbox);
        }
    };
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    let p = params(query);
    let result = {
        let mut st = shared.lock();
        let mbx = st.mailboxes.get_mut(&mailbox).expect("admitted");
        if user != "me" && !user.eq_ignore_ascii_case(&mbx.email) {
            Err(ApiError::new(
                403,
                "forbidden",
                format!("Delegation denied for {}", mbx.email),
            ))
        } else {
            let req = api::Req {
                method,
                path: rest,
                params: &p,
                content_type,
                body,
                upload,
            };
            api::route(mbx, &req)
        }
    };
    drop(guard);
    let out = match result {
        Ok(Reply::Json(v)) => Out {
            status: 200,
            body: Some(v),
            text: None,
            headers: Vec::new(),
        },
        Ok(Reply::Empty) => Out {
            status: 204,
            body: None,
            text: None,
            headers: Vec::new(),
        },
        Err(e) => Out::error(&e),
    };
    (out, Some(mailbox))
}

/// `POST /batch/gmail/v1`: a `multipart/mixed` body of `application/http` parts, each
/// served as its own request (faults, auth and quota apply to each) and answered in a
/// `multipart/mixed` response in the same order.
async fn batch(
    shared: &Arc<Shared>,
    token: &str,
    content_type: &str,
    body: &[u8],
) -> (Out, Option<String>) {
    // The batch request itself goes through faults and auth, at no quota cost.
    let (mailbox, delay, guard) = match admit(shared, "POST", "/batch/gmail/v1", token, 0, true) {
        Ok(ok) => ok,
        Err((delay, mailbox, e)) => {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            return (Out::error(&e), mailbox);
        }
    };
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    let Some(boundary) = mime::param(content_type, "boundary") else {
        let e = ApiError::new(
            400,
            "badRequest",
            "Batch requests need a multipart/mixed boundary",
        );
        return (Out::error(&e), Some(mailbox));
    };
    let parts = mime::multipart(body, &boundary);
    if parts.len() > MAX_BATCH {
        let e = ApiError::new(
            400,
            "badRequest",
            format!("Too many requests in a batch: at most {MAX_BATCH}"),
        );
        return (Out::error(&e), Some(mailbox));
    }
    let mut out = String::new();
    let reply_boundary = "batch_mock_gmail";
    for part in parts {
        let (part_headers, http) = mime::split(part);
        let content_id = mime::header(&part_headers, "content-id")
            .map(|c| c.trim_matches(['<', '>']).to_string())
            .unwrap_or_default();
        let inner = parse_inner(http);
        let (status, text) = match inner {
            None => (
                400,
                ApiError::new(400, "badRequest", "Malformed batch part")
                    .body()
                    .to_string(),
            ),
            Some(r) => {
                let inner_token = if r.token.is_empty() { token } else { &r.token };
                let (o, mb) = Box::pin(serve(
                    shared,
                    &r.method,
                    &r.path,
                    &r.query,
                    inner_token,
                    &r.content_type,
                    &r.body,
                    true,
                ))
                .await;
                shared.lock().log.push(LoggedRequest {
                    method: r.method.clone(),
                    path: r.path.clone(),
                    query: r.query.clone(),
                    mailbox: mb,
                    status: o.status,
                    body: String::from_utf8_lossy(&r.body).into_owned(),
                    in_batch: true,
                });
                (o.status, o.body.map(|b| b.to_string()).unwrap_or_default())
            }
        };
        let reason = StatusCode::from_u16(status)
            .ok()
            .and_then(|s| s.canonical_reason())
            .unwrap_or("");
        out.push_str(&format!(
            "--{reply_boundary}\r\nContent-Type: application/http\r\nContent-ID: <response-{content_id}>\r\n\r\n\
             HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{text}\r\n"
        ));
    }
    out.push_str(&format!("--{reply_boundary}--\r\n"));
    drop(guard);
    let o = Out {
        status: 200,
        body: None,
        text: Some(out),
        headers: vec![(
            "Content-Type".into(),
            format!("multipart/mixed; boundary={reply_boundary}"),
        )],
    };
    (o, Some(mailbox))
}

struct Inner {
    method: String,
    path: String,
    query: String,
    token: String,
    content_type: String,
    body: Vec<u8>,
}

/// One `application/http` part: `GET /gmail/v1/users/me/messages/x?format=metadata`,
/// optional headers, then an optional body. Full URLs are accepted too.
fn parse_inner(http: &[u8]) -> Option<Inner> {
    let text_end = mime::find(http, b"\n").unwrap_or(http.len());
    let line = String::from_utf8_lossy(&http[..text_end]);
    let mut words = line.split_whitespace();
    let method = words.next()?.to_ascii_uppercase();
    let target = words.next()?;
    let rest = &http[(text_end + 1).min(http.len())..];
    let (headers, body) = if rest.starts_with(b"\r\n") || rest.starts_with(b"\n") {
        (
            Vec::new(),
            rest.strip_prefix(b"\r\n")
                .or_else(|| rest.strip_prefix(b"\n"))
                .unwrap_or(rest),
        )
    } else {
        mime::split(rest)
    };
    let target = match target.find("://") {
        Some(i) => target[i + 3..]
            .find('/')
            .map_or("/", |j| &target[i + 3 + j..]),
        None => target,
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let auth = mime::header(&headers, "authorization").unwrap_or("");
    Some(Inner {
        method,
        path: path.to_string(),
        query: query.to_string(),
        token: auth
            .strip_prefix("Bearer ")
            .unwrap_or("")
            .trim()
            .to_string(),
        content_type: mime::header(&headers, "content-type")
            .unwrap_or("")
            .to_string(),
        body: trim_trailing_newlines(body).to_vec(),
    })
}

fn trim_trailing_newlines(mut b: &[u8]) -> &[u8] {
    while let [rest @ .., b'\r' | b'\n'] = b {
        b = rest;
    }
    b
}
