// SPDX-License-Identifier: MIT OR Apache-2.0
//! The HTTP server: auth, faults, concurrency, recorded replay and the Graph routes.

use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::ops::Bound;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State as AxState};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::faults::{Fault, FaultKind};
use crate::identity::{self, IdReply, Identity};
use crate::model::{
    Calendar, Contact, Event, FOLDERS, Folder, Mailbox, MailboxState, Message, MutationError,
    ROOT_ID,
};
use crate::recorded::Recording;
use crate::render::{self, Filter, Prefer, Search, parse_dt};

/// Graph's default page size for the collections the mock serves.
const DEFAULT_PAGE: usize = 10;
/// Graph's largest `$top` for messages.
const MAX_PAGE: usize = 1000;
/// `$search` returns at most this many messages in total.
const SEARCH_CAP: usize = 1000;

/// A request the mock answered, for test assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoggedRequest {
    /// HTTP method.
    pub method: String,
    /// Path, for example `/v1.0/me/mailFolders/inbox/messages/delta`.
    pub path: String,
    /// Raw query string.
    pub query: String,
    /// Mailbox the token belonged to, if it was valid.
    pub mailbox: Option<String>,
    /// Response status.
    pub status: u16,
    /// Request body as text (empty for reads).
    pub body: String,
}

struct TokenInfo {
    mailbox: String,
    expired: bool,
}

struct Budget {
    limit: u32,
    window: Duration,
    used: HashMap<String, (Instant, u32)>,
}

pub(crate) struct State {
    mailboxes: HashMap<String, MailboxState>,
    tokens: HashMap<String, TokenInfo>,
    faults: Vec<Fault>,
    recording: Recording,
    log: Vec<LoggedRequest>,
    latency: Duration,
    max_concurrent: Option<usize>,
    sparse_delta: bool,
    budget: Option<Budget>,
    origin: String,
    base_url: String,
    request_counter: u64,
    identity: Identity,
}

struct Shared {
    state: Mutex<State>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Configures a [`MockGraph`] before it starts.
pub struct Builder {
    mailboxes: Vec<(String, Mailbox)>,
    tokens: Vec<(String, String)>,
    recording: Recording,
    latency: Duration,
    max_concurrent: Option<usize>,
    budget: Option<(u32, Duration)>,
    port: u16,
    tenants: HashMap<String, String>,
    token_lifetime: Duration,
    device_interval: u64,
}

impl Builder {
    /// Adds a mailbox under `key`, reachable with the token `"<key>-token"`.
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

    /// Recorded exchanges replayed ahead of the model.
    pub fn recording(mut self, recording: Recording) -> Self {
        self.recording.exchanges.extend(recording.exchanges);
        self
    }

    /// Delay added to every request while it counts as in flight. Defaults to none.
    pub fn latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    /// Concurrent requests allowed per mailbox before 429 `ApplicationThrottled`
    /// (Graph's `MailboxConcurrency` limit). Defaults to 4, as in Outlook's limits;
    /// `None` turns the check off.
    pub fn max_concurrent(mut self, limit: Option<usize>) -> Self {
        self.max_concurrent = limit;
        self
    }

    /// Request budget per mailbox: after `limit` requests within `window`, requests get 429
    /// with `Retry-After` set to the rest of the window (Outlook's 10,000 per 10 minutes,
    /// scaled down for tests).
    pub fn request_budget(mut self, limit: u32, window: Duration) -> Self {
        self.budget = Some((limit, window));
        self
    }

    /// Puts mailbox `key`'s user in tenant `tid` for the identity mock. Use
    /// [`crate::CONSUMER_TENANT`] for a personal account such as Hotmail. Mailboxes default
    /// to the work tenant [`crate::DEFAULT_TENANT`].
    pub fn tenant(mut self, key: impl Into<String>, tid: impl Into<String>) -> Self {
        self.tenants.insert(key.into(), tid.into());
        self
    }

    /// Lifetime of access tokens issued by the identity mock. Defaults to an hour.
    pub fn token_lifetime(mut self, lifetime: Duration) -> Self {
        self.token_lifetime = lifetime;
        self
    }

    /// The device code polling interval the identity mock asks for, in seconds. Defaults to 1.
    pub fn device_interval(mut self, secs: u64) -> Self {
        self.device_interval = secs;
        self
    }

    /// Listens on this port instead of a free one.
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Starts the server on `127.0.0.1`. Must be called inside a Tokio runtime.
    pub async fn start(mut self) -> std::io::Result<MockGraph> {
        if self.mailboxes.is_empty() {
            self = self.mailbox("default", Mailbox::small());
            self.tokens.push(("test-token".into(), "default".into()));
        }
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", self.port)).await?;
        let addr = listener.local_addr()?;
        let origin = format!("http://{addr}");
        let base_url = format!("{origin}/v1.0");
        let mut identity = Identity::new(self.tenants);
        identity.lifetime = self.token_lifetime;
        identity.device_interval = self.device_interval;
        let state = State {
            mailboxes: self
                .mailboxes
                .into_iter()
                .map(|(k, m)| (k, MailboxState::new(m)))
                .collect(),
            tokens: self
                .tokens
                .into_iter()
                .map(|(t, mailbox)| {
                    (
                        t,
                        TokenInfo {
                            mailbox,
                            expired: false,
                        },
                    )
                })
                .collect(),
            faults: Vec::new(),
            recording: self.recording,
            log: Vec::new(),
            latency: self.latency,
            max_concurrent: self.max_concurrent,
            sparse_delta: false,
            budget: self.budget.map(|(limit, window)| Budget {
                limit,
                window,
                used: HashMap::new(),
            }),
            origin,
            base_url: base_url.clone(),
            request_counter: 0,
            identity,
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
        Ok(MockGraph {
            addr,
            base_url,
            shared,
            shutdown: Some(tx),
        })
    }
}

/// A running mock Microsoft Graph server on `127.0.0.1`.
///
/// It serves `GET` requests under `/v1.0/me/...` (and `/v1.0/users/<id or upn>/...`) for
/// folders, messages, delta, `$search`, calendars and events, from in-memory mailboxes
/// that tests can change while it runs. It stops when dropped.
///
/// ```no_run
/// # async fn demo() -> std::io::Result<()> {
/// use familiar_mock_graph::{Mailbox, MockGraph};
/// let mock = MockGraph::builder().mailbox("hotmail", Mailbox::small()).start().await?;
/// let url = format!("{}/me/mailFolders", mock.base_url());
/// // GET it with `Authorization: Bearer hotmail-token`.
/// # let _ = url; Ok(()) }
/// ```
pub struct MockGraph {
    addr: SocketAddr,
    base_url: String,
    shared: Arc<Shared>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Drop for MockGraph {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl MockGraph {
    /// A builder with no mailboxes. Starting it without any adds `default`
    /// ([`Mailbox::small`]) with the tokens `default-token` and `test-token`.
    pub fn builder() -> Builder {
        Builder {
            mailboxes: Vec::new(),
            tokens: Vec::new(),
            recording: Recording::default(),
            latency: Duration::ZERO,
            max_concurrent: Some(4),
            budget: None,
            port: 0,
            tenants: HashMap::new(),
            token_lifetime: Duration::from_secs(3600),
            device_interval: 1,
        }
    }

    /// Starts the default server: the small fixture as mailbox `default`, token `test-token`.
    pub async fn start() -> std::io::Result<Self> {
        Self::builder().start().await
    }

    /// `http://127.0.0.1:<port>/v1.0`, the provider's Graph base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The listening address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://127.0.0.1:<port>`, the identity mock's authority host: use it where the client
    /// would use `https://login.microsoftonline.com`.
    pub fn login_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Makes mailbox `key`'s user the one signed in to the "browser": `/authorize` and
    /// device code approval sign in as this user unless `login_hint` names another.
    /// `None` signs out, so device code polling stays pending.
    pub fn sign_in_as(&self, key: Option<&str>) {
        self.shared
            .lock()
            .identity
            .sign_in_as(key.map(str::to_string));
    }

    /// Revokes every refresh token issued for mailbox `key`, as a password change or
    /// "sign out everywhere" would. Refreshing then fails with `invalid_grant`.
    pub fn revoke_refresh_tokens(&self, key: &str) {
        self.shared.lock().identity.revoke(key);
    }

    /// Makes every access token of mailbox `key` answer 401 as expired.
    pub fn expire_access_tokens(&self, key: &str) {
        for t in self.shared.lock().tokens.values_mut() {
            if t.mailbox == key {
                t.expired = true;
            }
        }
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

    /// Makes `token` answer 401 as expired, as after an hour without a refresh.
    pub fn expire_token(&self, token: &str) {
        if let Some(t) = self.shared.lock().tokens.get_mut(token) {
            t.expired = true;
        }
    }

    /// Adds a valid token for mailbox `key`, as a refresh would issue.
    pub fn add_token(&self, token: &str, key: &str) {
        self.shared.lock().tokens.insert(
            token.into(),
            TokenInfo {
                mailbox: key.into(),
                expired: false,
            },
        );
    }

    /// Sets the per-request latency.
    pub fn set_latency(&self, latency: Duration) {
        self.shared.lock().latency = latency;
    }

    /// Makes incremental message delta rounds list changed messages with only
    /// `id`, `isRead` and `flag`, as Outlook.com sometimes does, instead of
    /// every selected field.
    pub fn set_sparse_message_delta(&self, sparse: bool) {
        self.shared.lock().sparse_delta = sparse;
    }

    /// Every request served so far, oldest first.
    pub fn requests(&self) -> Vec<LoggedRequest> {
        self.shared.lock().log.clone()
    }

    /// Forgets the request log.
    pub fn clear_requests(&self) {
        self.shared.lock().log.clear();
    }
}

/// Changes one mailbox on a running [`MockGraph`]. Each change is seen by later delta calls.
pub struct MailboxHandle<'a> {
    shared: &'a Shared,
    key: String,
}

impl MailboxHandle<'_> {
    fn with<R>(&self, f: impl FnOnce(&mut MailboxState) -> R) -> R {
        let mut st = self.shared.lock();
        f(st.mailboxes.get_mut(&self.key).expect("mailbox exists"))
    }

    /// Adds a message and returns its immutable ID.
    pub fn add_message(&self, msg: Message) -> Result<String, MutationError> {
        self.with(|m| m.insert_message(msg))
    }

    /// Changes a message (by immutable or REST ID). Its ID and folder stay the same.
    pub fn update_message(
        &self,
        id: &str,
        f: impl FnOnce(&mut Message),
    ) -> Result<(), MutationError> {
        self.with(|m| m.update_message(id, f))
    }

    /// Moves a message and returns its new REST ID. The immutable ID does not change.
    pub fn move_message(&self, id: &str, folder: &str) -> Result<String, MutationError> {
        self.with(|m| m.move_message(id, folder))
    }

    /// Permanently deletes a message.
    pub fn delete_message(&self, id: &str) -> Result<(), MutationError> {
        self.with(|m| m.delete_message(id))
    }

    /// A message's current state (by immutable or REST ID).
    pub fn message(&self, id: &str) -> Option<Message> {
        self.with(|m| {
            let imm = m.lookup_message(id)?.to_string();
            m.messages.get(&imm).map(|r| r.msg.clone())
        })
    }

    /// A message's current REST (default) ID.
    pub fn rest_id(&self, id: &str) -> Option<String> {
        self.with(|m| {
            let imm = m.lookup_message(id)?.to_string();
            m.messages.get(&imm).map(|r| r.rest_id.clone())
        })
    }

    /// Immutable IDs of the messages in a folder (ID or well-known name), newest first.
    pub fn message_ids(&self, folder: &str) -> Vec<String> {
        self.with(|m| {
            let Some(f) = m.resolve_folder(folder) else {
                return Vec::new();
            };
            m.by_folder
                .get(&f)
                .map(|s| s.iter().rev().map(|(_, id)| id.clone()).collect())
                .unwrap_or_default()
        })
    }

    /// Number of messages in the mailbox.
    pub fn message_count(&self) -> usize {
        self.with(|m| m.messages.len())
    }

    /// The ID of a folder given its ID or well-known name.
    pub fn folder_id(&self, id_or_name: &str) -> Option<String> {
        self.with(|m| m.resolve_folder(id_or_name))
    }

    /// Adds a folder and returns its ID.
    pub fn add_folder(&self, folder: Folder) -> String {
        self.with(|m| m.insert_folder(folder))
    }

    /// Renames a folder.
    pub fn rename_folder(&self, id: &str, name: &str) -> Result<(), MutationError> {
        self.with(|m| m.update_folder(id, |f| f.display_name = name.to_string()))
    }

    /// Deletes a folder with its subfolders and messages.
    pub fn delete_folder(&self, id: &str) -> Result<(), MutationError> {
        self.with(|m| m.delete_folder(id))
    }

    /// Adds a calendar and returns its ID.
    pub fn add_calendar(&self, calendar: Calendar) -> String {
        self.with(|m| m.add_calendar(calendar))
    }

    /// Adds an event and returns its ID.
    pub fn add_event(&self, event: Event) -> Result<String, MutationError> {
        self.with(|m| m.insert_event(event))
    }

    /// Adds a contact and returns its ID.
    pub fn add_contact(&self, contact: Contact) -> String {
        self.with(|m| m.add_contact(contact))
    }

    /// Removes a contact.
    pub fn remove_contact(&self, id: &str) {
        self.with(|m| m.contacts.retain(|c| c.id != id));
    }

    /// An event by ID.
    pub fn event(&self, id: &str) -> Option<Event> {
        self.with(|m| m.events.get(id).map(|r| r.ev.clone()))
    }

    /// Every event (masters, occurrences and single events), by start.
    pub fn events(&self) -> Vec<Event> {
        self.with(|m| {
            let mut v: Vec<Event> = m.events.values().map(|r| r.ev.clone()).collect();
            v.sort_by(|a, b| (a.start, &a.id).cmp(&(b.start, &b.id)));
            v
        })
    }

    /// A calendar's ID by name.
    pub fn calendar_id(&self, name: &str) -> Option<String> {
        self.with(|m| m.resolve_calendar(name))
    }

    /// Changes an event.
    pub fn update_event(&self, id: &str, f: impl FnOnce(&mut Event)) -> Result<(), MutationError> {
        self.with(|m| m.update_event(id, f))
    }

    /// Deletes an event.
    pub fn delete_event(&self, id: &str) -> Result<(), MutationError> {
        self.with(|m| m.delete_event(id))
    }

    /// Invalidates every delta and skip token issued so far: they answer
    /// 410 `SyncStateNotFound` and the client must start a full resync.
    pub fn expire_delta_tokens(&self) {
        self.with(|m| m.delta_epoch += 1);
    }

    /// The most requests this mailbox has had in flight at once.
    pub fn peak_in_flight(&self) -> usize {
        self.with(|m| m.peak_in_flight)
    }
}

// ---------------------------------------------------------------------------
// Request handling

pub(crate) struct GraphError {
    status: u16,
    code: String,
    message: String,
    headers: Vec<(String, String)>,
}

impl GraphError {
    fn new(status: u16, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            headers: Vec::new(),
        }
    }

    fn header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }

    fn not_found() -> Self {
        Self::new(
            404,
            "ErrorItemNotFound",
            "The specified object was not found in the store.",
        )
    }

    fn bad_query(message: impl Into<String>) -> Self {
        Self::new(400, "ErrorInvalidUrlQuery", message)
    }

    fn throttled(retry_after: u64, code: &str, message: &str) -> Self {
        Self::new(429, code, message).header("Retry-After", retry_after.to_string())
    }

    fn auth(message: &str) -> Self {
        Self::new(401, "InvalidAuthenticationToken", message).header(
            "WWW-Authenticate",
            "Bearer realm=\"\", authorization_uri=\"https://login.microsoftonline.com/common/oauth2/authorize\", error=\"invalid_token\"",
        )
    }

    fn from_fault(kind: &FaultKind) -> Option<Self> {
        Some(match kind {
            FaultKind::Throttle { retry_after } => {
                Self::throttled(*retry_after, "TooManyRequests", "Please retry again later.")
            }
            FaultKind::Unavailable { retry_after } => Self::new(
                503,
                "ServiceUnavailable",
                "The service is temporarily unavailable.",
            )
            .header("Retry-After", retry_after.to_string()),
            FaultKind::AuthExpired => {
                Self::auth("Lifetime validation failed, the token is expired.")
            }
            FaultKind::Status {
                status,
                code,
                message,
            } => Self::new(*status, code, message.clone()),
            FaultKind::Delay(_) => return None,
        })
    }
}

struct Reply {
    status: u16,
    body: Value,
    headers: Vec<(String, String)>,
}

impl Reply {
    fn ok(body: Value) -> Self {
        Self {
            status: 200,
            body,
            headers: Vec::new(),
        }
    }
}

fn build_response(
    status: u16,
    body: &Value,
    headers: &[(String, String)],
    request_id: &str,
) -> Response {
    let mut resp = Response::new(Body::from(body.to_string()));
    *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let h = resp.headers_mut();
    h.insert(
        "content-type",
        HeaderValue::from_static("application/json;odata.metadata=minimal;charset=utf-8"),
    );
    if let Ok(v) = HeaderValue::from_str(request_id) {
        h.insert("request-id", v.clone());
        h.insert("client-request-id", v);
    }
    for (k, v) in headers {
        if let (Ok(k), Ok(v)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            h.insert(k, v);
        }
    }
    resp
}

fn error_response(e: &GraphError, request_id: &str) -> Response {
    let body = json!({
        "error": {
            "code": e.code,
            "message": e.message,
            "innerError": {
                "date": render::dt(&Utc::now()),
                "request-id": request_id,
                "client-request-id": request_id,
            }
        }
    });
    build_response(e.status, &body, &e.headers, request_id)
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

async fn dispatch(AxState(shared): AxState<Arc<Shared>>, req: Request) -> Response {
    let method = req.method().as_str().to_ascii_uppercase();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let mut params: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    if let Some((tenant, op)) = identity::parse_path(&path) {
        let body = axum::body::to_bytes(req.into_body(), 1 << 20)
            .await
            .unwrap_or_default();
        params.extend(form_urlencoded::parse(&body).into_owned());
        let (status, resp) = serve_identity(&shared, &tenant, &op, &method, &params);
        shared.lock().log.push(LoggedRequest {
            method,
            path,
            query,
            mailbox: None,
            status,
            body: String::new(),
        });
        return resp;
    }
    let headers = req.headers().clone();
    let body = axum::body::to_bytes(req.into_body(), 32 << 20)
        .await
        .unwrap_or_default();
    let (status, mailbox, resp) = serve(&shared, &method, &path, &params, &headers, &body).await;
    shared.lock().log.push(LoggedRequest {
        method,
        path,
        query,
        mailbox,
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    });
    resp
}

fn serve_identity(
    shared: &Arc<Shared>,
    tenant: &str,
    op: &str,
    method: &str,
    params: &[(String, String)],
) -> (u16, Response) {
    let mut st = shared.lock();
    let users = st
        .mailboxes
        .iter()
        .map(|(k, m)| (k.clone(), m.user.clone()))
        .collect();
    let mut issued = Vec::new();
    let reply = identity::handle(
        &mut st.identity,
        &users,
        tenant,
        op,
        method,
        params,
        &mut issued,
    );
    for (token, mailbox) in issued {
        st.tokens.insert(
            token,
            TokenInfo {
                mailbox,
                expired: false,
            },
        );
    }
    let (status, body, content_type, location) = match reply {
        IdReply::Json(status, body) => (
            status,
            Body::from(body.to_string()),
            Some("application/json; charset=utf-8"),
            None,
        ),
        IdReply::Text(status, text) => (
            status,
            Body::from(text),
            Some("text/plain; charset=utf-8"),
            None,
        ),
        IdReply::Redirect(url) => (302, Body::empty(), None, Some(url)),
    };
    let mut resp = Response::new(body);
    *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if let Some(ct) = content_type {
        resp.headers_mut()
            .insert("content-type", HeaderValue::from_static(ct));
    }
    if let Some(v) = location.and_then(|u| HeaderValue::from_str(&u).ok()) {
        resp.headers_mut().insert("location", v);
    }
    (status, resp)
}

enum Admit {
    Ok(String, Duration, String, InFlight),
    Refused(Duration, u16, Option<String>, Response),
}

/// Under the lock: injected faults, auth, request budget and concurrency.
fn admit(shared: &Arc<Shared>, method: &str, path: &str, headers: &HeaderMap) -> Admit {
    let mut st = shared.lock();
    st.request_counter += 1;
    let request_id = format!("00000000-0000-4000-8000-{:012}", st.request_counter);
    let mut delay = st.latency;
    let mut terminal = None;
    let mut i = 0;
    while i < st.faults.len() {
        if st.faults[i].matches(method, path) {
            let kind = st.faults[i].kind.clone();
            if let Some(n) = st.faults[i].remaining.as_mut() {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    st.faults.remove(i);
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
            match &kind {
                FaultKind::Delay(d) => delay += *d,
                other => {
                    terminal = GraphError::from_fault(other);
                    break;
                }
            }
        } else {
            i += 1;
        }
    }
    if let Some(e) = terminal {
        return Admit::Refused(delay, e.status, None, error_response(&e, &request_id));
    }

    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .map(str::trim)
        .unwrap_or("");
    let auth = if token.is_empty() {
        Err(GraphError::auth("Access token is empty."))
    } else {
        match st.tokens.get(token) {
            None => Err(GraphError::auth(
                "IDX14100: JWT is not well formed, there are no dots (.).",
            )),
            Some(t) if t.expired => Err(GraphError::auth(
                "Lifetime validation failed, the token is expired.",
            )),
            Some(t) => Ok(t.mailbox.clone()),
        }
    };
    let mailbox = match auth {
        Ok(m) => m,
        Err(e) => {
            return Admit::Refused(
                Duration::ZERO,
                e.status,
                None,
                error_response(&e, &request_id),
            );
        }
    };

    if let Some(b) = st.budget.as_mut() {
        let now = Instant::now();
        let entry = b.used.entry(mailbox.clone()).or_insert((now, 0));
        if now.duration_since(entry.0) >= b.window {
            *entry = (now, 0);
        }
        if entry.1 >= b.limit {
            let left = b.window.saturating_sub(now.duration_since(entry.0));
            let e = GraphError::throttled(
                left.as_secs().max(1),
                "ApplicationThrottled",
                "Application is over its MailboxRequests limit.",
            );
            return Admit::Refused(
                Duration::ZERO,
                e.status,
                Some(mailbox),
                error_response(&e, &request_id),
            );
        }
        entry.1 += 1;
    }

    let max = st.max_concurrent;
    let Some(mbx) = st.mailboxes.get_mut(&mailbox) else {
        let e = GraphError::new(
            404,
            "MailboxNotEnabledForRESTAPI",
            "The mailbox is unknown.",
        );
        return Admit::Refused(
            Duration::ZERO,
            e.status,
            Some(mailbox),
            error_response(&e, &request_id),
        );
    };
    if max.is_some_and(|m| mbx.in_flight >= m) {
        let e = GraphError::throttled(
            1,
            "ApplicationThrottled",
            "Application is over its MailboxConcurrency limit.",
        );
        return Admit::Refused(
            Duration::ZERO,
            e.status,
            Some(mailbox),
            error_response(&e, &request_id),
        );
    }
    mbx.in_flight += 1;
    mbx.peak_in_flight = mbx.peak_in_flight.max(mbx.in_flight);
    let guard = InFlight {
        shared: shared.clone(),
        mailbox: mailbox.clone(),
    };
    Admit::Ok(request_id, delay, mailbox, guard)
}

async fn serve(
    shared: &Arc<Shared>,
    method: &str,
    path: &str,
    params: &[(String, String)],
    headers: &HeaderMap,
    body: &[u8],
) -> (u16, Option<String>, Response) {
    let (request_id, delay, mailbox, guard) = match admit(shared, method, path, headers) {
        Admit::Ok(r, d, m, g) => (r, d, m, g),
        Admit::Refused(delay, status, mailbox, resp) => {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            return (status, mailbox, resp);
        }
    };

    // Phase 2: simulated latency while the request counts as in flight.
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }

    // Phase 3, under the lock again: recorded replay, then the model.
    let prefer_values: Vec<&str> = headers
        .get_all("prefer")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    let prefer = Prefer::parse(&prefer_values);
    let result = {
        let mut st = shared.lock();
        if let Some(rec) = st.recording.find(method, path, params, &mailbox) {
            let body = serde_json::to_string(&rec.body)
                .unwrap_or_default()
                .replace("{{baseUrl}}", &st.base_url);
            Ok(Reply {
                status: rec.status,
                body: serde_json::from_str(&body).unwrap_or(Value::Null),
                headers: rec.headers.clone().into_iter().collect(),
            })
        } else if method != "GET" {
            let base = st.base_url.clone();
            let body: Value = if body.is_empty() {
                Value::Null
            } else {
                match serde_json::from_slice(body) {
                    Ok(v) => v,
                    Err(e) => {
                        drop(st);
                        drop(guard);
                        let e = GraphError::new(
                            400,
                            "RequestBodyRead",
                            format!("The request body is not valid JSON: {e}"),
                        );
                        return (e.status, Some(mailbox), error_response(&e, &request_id));
                    }
                }
            };
            let mbox = st.mailboxes.get_mut(&mailbox).expect("admitted");
            writes::route(mbox, &base, method, path, &body, &prefer)
        } else {
            let ctx = Ctx {
                st: &st.mailboxes[&mailbox],
                origin: &st.origin,
                base: &st.base_url,
                path,
                q: Q(params),
                prefer,
                sparse_delta: st.sparse_delta,
            };
            route(&ctx, method)
        }
    };
    drop(guard);
    match result {
        Ok(mut r) => {
            if let Some(applied) = prefer.applied() {
                r.headers.push(("Preference-Applied".into(), applied));
            }
            (
                r.status,
                Some(mailbox),
                build_response(r.status, &r.body, &r.headers, &request_id),
            )
        }
        Err(e) => (e.status, Some(mailbox), error_response(&e, &request_id)),
    }
}

// ---------------------------------------------------------------------------
// Routes

#[path = "writes.rs"]
mod writes;

struct Q<'a>(&'a [(String, String)]);

impl Q<'_> {
    /// A query option by name, with or without the `$`.
    fn get(&self, name: &str) -> Option<&str> {
        let bare = name.trim_start_matches('$');
        self.0
            .iter()
            .find(|(k, _)| k.trim_start_matches('$').eq_ignore_ascii_case(bare))
            .map(|(_, v)| v.as_str())
    }

    fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    fn usize(&self, name: &str) -> Result<Option<usize>, GraphError> {
        self.get(name)
            .map(|v| {
                v.parse()
                    .map_err(|_| GraphError::bad_query(format!("Invalid value for {name}: {v}")))
            })
            .transpose()
    }

    fn top(&self, default: usize) -> Result<usize, GraphError> {
        let top = self.usize("$top")?.unwrap_or(default);
        if top == 0 || top > MAX_PAGE {
            return Err(GraphError::bad_query(format!(
                "$top must be between 1 and {MAX_PAGE}."
            )));
        }
        Ok(top)
    }

    fn select(&self) -> Option<Vec<String>> {
        self.get("$select").map(|s| {
            s.split(',')
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty())
                .collect()
        })
    }

    fn filter(&self) -> Result<Option<Filter>, GraphError> {
        self.get("$filter")
            .map(|f| Filter::parse(f).map_err(GraphError::bad_query))
            .transpose()
    }

    fn count(&self) -> bool {
        self.get("$count")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }

    /// The query with the given options removed and others added, encoded.
    fn rebuild(&self, drop: &[&str], add: &[(&str, String)]) -> String {
        let mut ser = form_urlencoded::Serializer::new(String::new());
        for (k, v) in self.0 {
            let bare = k.trim_start_matches('$');
            if drop
                .iter()
                .any(|d| d.trim_start_matches('$').eq_ignore_ascii_case(bare))
            {
                continue;
            }
            ser.append_pair(k, v);
        }
        for (k, v) in add {
            ser.append_pair(k, v);
        }
        ser.finish()
    }
}

struct Ctx<'a> {
    st: &'a MailboxState,
    origin: &'a str,
    base: &'a str,
    path: &'a str,
    q: Q<'a>,
    prefer: Prefer,
    sparse_delta: bool,
}

impl Ctx<'_> {
    fn link(&self, drop: &[&str], add: &[(&str, String)]) -> String {
        let q = self.q.rebuild(drop, add);
        format!("{}{}?{}", self.origin, self.path, q)
    }

    fn context(&self, what: &str) -> String {
        format!("{}/$metadata#{}", self.base, what)
    }

    fn select(&self, v: Value) -> Value {
        render::select(v, self.q.select().as_deref())
    }
}

type R = Result<Reply, GraphError>;

fn route(c: &Ctx<'_>, method: &str) -> R {
    let Some(rest) = strip_prefix_ci(c.path, "/v1.0/") else {
        return Err(GraphError::new(
            400,
            "BadRequest",
            "Invalid version: the mock serves /v1.0 only.",
        ));
    };
    debug_assert_eq!(method, "GET");
    let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    let segs = match segs.as_slice() {
        ["me", tail @ ..] => tail.to_vec(),
        [u, who, tail @ ..] if u.eq_ignore_ascii_case("users") => {
            let user = &c.st.user;
            let me = [&user.id, &user.user_principal_name, &user.mail];
            if !me.iter().any(|m| m.eq_ignore_ascii_case(who)) {
                return Err(GraphError::new(
                    403,
                    "ErrorAccessDenied",
                    "Access is denied. Check credentials and try again.",
                ));
            }
            tail.to_vec()
        }
        _ => {
            return Err(GraphError::new(
                400,
                "BadRequest",
                "Resource not found for the segment.",
            ));
        }
    };
    let lower: Vec<String> = segs.iter().map(|s| s.to_ascii_lowercase()).collect();
    let l: Vec<&str> = lower.iter().map(String::as_str).collect();
    let st = c.st;
    match l.as_slice() {
        [] => Ok(Reply::ok(render::user(&st.user))),
        ["mailboxsettings"] => Ok(Reply::ok(render::mailbox_settings(&st.user))),
        ["contacts"] => list_contacts(c, ""),
        ["contactfolders"] => {
            let mut ids: Vec<&str> = st
                .contacts
                .iter()
                .map(|x| x.folder_id.as_str())
                .filter(|f| !f.is_empty())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            let items: Vec<Value> = ids
                .into_iter()
                .map(|id| json!({"id": id, "displayName": id, "parentFolderId": "contacts"}))
                .collect();
            paged(c, items, "users('me')/contactFolders", DEFAULT_PAGE)
        }
        ["contactfolders", _, "contacts"] => list_contacts(c, segs[1]),
        ["mailfolders"] => list_folders(c, ROOT_ID),
        ["mailfolders", "delta"] => folder_delta(c),
        ["mailfolders", _] => {
            let f = folder_id(c, segs[1])?;
            let rec = st.folder(&f).ok_or_else(GraphError::not_found)?;
            Ok(Reply::ok(c.select(render::folder(st, rec))))
        }
        ["mailfolders", _, "childfolders"] => {
            let f = folder_id(c, segs[1])?;
            list_folders(c, &f)
        }
        ["mailfolders", _, "messages"] => {
            let f = folder_id(c, segs[1])?;
            list_messages(c, Some(&f))
        }
        ["mailfolders", _, "messages", "delta"] => {
            let f = folder_id(c, segs[1])?;
            message_delta(c, &f)
        }
        ["mailfolders", _, "messages", _] => {
            let f = folder_id(c, segs[1])?;
            get_message(c, segs[3], Some(&f))
        }
        ["messages"] => list_messages(c, None),
        ["messages", _] => get_message(c, segs[1], None),
        ["messages", _, "attachments"] => list_attachments(c, segs[1]),
        ["messages", _, "attachments", _] => get_attachment(c, segs[1], segs[3]),
        ["calendars"] => {
            let items: Vec<Value> = st
                .calendars
                .iter()
                .map(|cal| c.select(render::calendar(&st.user, cal)))
                .collect();
            paged(c, items, "users('me')/calendars", DEFAULT_PAGE)
        }
        ["calendar"] => get_calendar(c, ""),
        ["calendars", _] => get_calendar(c, segs[1]),
        ["events"] | ["calendar", "events"] => list_events(c, "", None),
        ["calendars", _, "events"] => list_events(c, segs[1], None),
        ["events", _] | ["calendar", "events", _] => get_event(c, segs[segs.len() - 1], ""),
        ["calendars", _, "events", _] => get_event(c, segs[3], segs[1]),
        ["events", _, "instances"] => {
            let w = window(c)?;
            event_instances(c, segs[1], w)
        }
        ["calendarview"] | ["calendar", "calendarview"] => {
            let w = window(c)?;
            list_events(c, "", Some(w))
        }
        ["calendars", _, "calendarview"] => {
            let w = window(c)?;
            list_events(c, segs[1], Some(w))
        }
        ["calendarview", "delta"] | ["calendar", "calendarview", "delta"] => event_delta(c, ""),
        ["calendars", _, "calendarview", "delta"] => event_delta(c, segs[1]),
        _ => Err(GraphError::new(
            400,
            "BadRequest",
            format!(
                "Resource not found for the segment '{}' (not served by the mock).",
                segs.join("/")
            ),
        )),
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    (s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix))
        .then(|| &s[prefix.len()..])
}

fn folder_id(c: &Ctx<'_>, raw: &str) -> Result<String, GraphError> {
    c.st.resolve_folder(raw).ok_or_else(GraphError::not_found)
}

/// Slices a fully rendered collection with `$top`/`$skip` and adds `@odata.nextLink`.
fn paged(c: &Ctx<'_>, items: Vec<Value>, context: &str, default_top: usize) -> R {
    let top = c.q.top(default_top)?;
    let skip = c.q.usize("$skip")?.unwrap_or(0);
    let total = items.len();
    let page: Vec<Value> = items.into_iter().skip(skip).take(top).collect();
    Ok(Reply::ok(collection(
        c,
        context,
        page,
        (skip + top < total).then(|| c.link(&["$skip"], &[("$skip", (skip + top).to_string())])),
        c.q.count().then_some(total),
    )))
}

fn collection(
    c: &Ctx<'_>,
    context: &str,
    items: Vec<Value>,
    next: Option<String>,
    count: Option<usize>,
) -> Value {
    let mut body = json!({ "@odata.context": c.context(context) });
    if let Some(n) = count {
        body["@odata.count"] = json!(n);
    }
    body["value"] = Value::Array(items);
    if let Some(n) = next {
        body["@odata.nextLink"] = json!(n);
    }
    body
}

fn list_folders(c: &Ctx<'_>, parent: &str) -> R {
    let st = c.st;
    let hidden =
        c.q.get("includeHiddenFolders")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
    let filter = c.q.filter()?;
    let items: Vec<Value> = st
        .folders
        .iter()
        .filter(|r| r.folder.parent_folder_id.as_deref() == Some(parent))
        .filter(|r| hidden || !r.folder.is_hidden)
        .map(|r| render::folder(st, r))
        .filter(|v| filter.as_ref().is_none_or(|f| f.matches(v)))
        .map(|v| c.select(v))
        .collect();
    paged(c, items, "users('me')/mailFolders", DEFAULT_PAGE)
}

/// Message IDs in a folder (or all folders), newest first unless `asc`.
fn ordered_messages<'a>(st: &'a MailboxState, folder: Option<&str>, asc: bool) -> Vec<&'a str> {
    let mut ids: Vec<(DateTime<Utc>, &str)> = match folder {
        Some(f) => st
            .by_folder
            .get(f)
            .map(|s| s.iter().map(|(t, id)| (*t, id.as_str())).collect())
            .unwrap_or_default(),
        None => st
            .by_folder
            .values()
            .flat_map(|s| s.iter().map(|(t, id)| (*t, id.as_str())))
            .collect(),
    };
    if folder.is_none() {
        ids.sort();
    }
    if !asc {
        ids.reverse();
    }
    ids.into_iter().map(|(_, id)| id).collect()
}

fn list_messages(c: &Ctx<'_>, folder: Option<&str>) -> R {
    let st = c.st;
    let context = "users('me')/messages";
    if let Some(raw) = c.q.get("$search") {
        if c.q.has("$filter") || c.q.has("$orderby") {
            return Err(GraphError::bad_query(
                "The query parameter '$search' can't be combined with '$filter' or '$orderby'.",
            ));
        }
        if c.q.has("$skip") {
            return Err(GraphError::bad_query(
                "'$skip' is not supported with '$search'; follow @odata.nextLink.",
            ));
        }
        let search = Search::parse(raw);
        let hits: Vec<&str> = ordered_messages(st, folder, false)
            .into_iter()
            .filter(|id| search.matches(&st.messages[*id]))
            .take(SEARCH_CAP)
            .collect();
        let top = c.q.top(DEFAULT_PAGE)?;
        let offset: usize =
            c.q.get("$skiptoken")
                .map(|t| {
                    t.parse()
                        .map_err(|_| GraphError::bad_query("Invalid $skiptoken."))
                })
                .transpose()?
                .unwrap_or(0);
        let page = hits
            .iter()
            .skip(offset)
            .take(top)
            .map(|id| c.select(render::message(&st.messages[*id], &c.prefer)))
            .collect();
        let next = (offset + top < hits.len()).then(|| {
            c.link(
                &["$skiptoken"],
                &[("$skiptoken", (offset + top).to_string())],
            )
        });
        return Ok(Reply::ok(collection(c, context, page, next, None)));
    }

    let asc = match c.q.get("$orderby").map(|o| o.trim().to_ascii_lowercase()) {
        None => false,
        Some(o) if o == "receiveddatetime desc" => false,
        Some(o) if o == "receiveddatetime" || o == "receiveddatetime asc" => true,
        Some(o) => {
            return Err(GraphError::bad_query(format!(
                "The mock supports $orderby=receivedDateTime [asc|desc] only, not '{o}'."
            )));
        }
    };
    let ids = ordered_messages(st, folder, asc);
    let top = c.q.top(DEFAULT_PAGE)?;
    let skip = c.q.usize("$skip")?.unwrap_or(0);
    let (page, total): (Vec<Value>, usize) = match c.q.filter()? {
        Some(f) => {
            let all: Vec<Value> = ids
                .iter()
                .map(|id| render::message(&st.messages[*id], &c.prefer))
                .filter(|v| f.matches(v))
                .collect();
            let total = all.len();
            (
                all.into_iter()
                    .skip(skip)
                    .take(top)
                    .map(|v| c.select(v))
                    .collect(),
                total,
            )
        }
        None => (
            ids.iter()
                .skip(skip)
                .take(top)
                .map(|id| c.select(render::message(&st.messages[*id], &c.prefer)))
                .collect(),
            ids.len(),
        ),
    };
    let next =
        (skip + top < total).then(|| c.link(&["$skip"], &[("$skip", (skip + top).to_string())]));
    Ok(Reply::ok(collection(
        c,
        context,
        page,
        next,
        c.q.count().then_some(total),
    )))
}

fn get_message(c: &Ctx<'_>, id: &str, folder: Option<&str>) -> R {
    let st = c.st;
    let imm = st.lookup_message(id).ok_or_else(GraphError::not_found)?;
    let rec = &st.messages[imm];
    if folder.is_some_and(|f| f != rec.msg.folder_id) {
        return Err(GraphError::not_found());
    }
    let mut v = c.select(render::message(rec, &c.prefer));
    // `$expand=microsoft.graph.eventMessage/event(...)`, the form Outlook.com
    // accepts (it refuses the `/microsoft.graph.eventMessage` cast segment).
    if let Some(meeting) = &rec.msg.meeting {
        let expand = c.q.get("$expand").unwrap_or_default().to_ascii_lowercase();
        if expand.contains("microsoft.graph.eventmessage/event") {
            v["event"] = json!({ "id": meeting.event_id });
        }
    }
    Ok(Reply::ok(v))
}

fn attachment_json(a: &crate::model::Attachment) -> Value {
    json!({
        "@odata.type": "#microsoft.graph.fileAttachment",
        "id": a.id,
        "name": a.name,
        "contentType": a.content_type,
        "size": a.content_bytes.len() / 4 * 3,
        "isInline": a.is_inline,
        "contentId": a.content_id,
        "contentBytes": a.content_bytes,
    })
}

fn list_attachments(c: &Ctx<'_>, id: &str) -> R {
    let st = c.st;
    let imm = st.lookup_message(id).ok_or_else(GraphError::not_found)?;
    let items: Vec<Value> = st.messages[imm]
        .msg
        .attachments
        .iter()
        .map(|a| c.select(attachment_json(a)))
        .collect();
    paged(c, items, "users('me')/messages/attachments", DEFAULT_PAGE)
}

fn get_attachment(c: &Ctx<'_>, id: &str, att: &str) -> R {
    let st = c.st;
    let imm = st.lookup_message(id).ok_or_else(GraphError::not_found)?;
    let a = st.messages[imm]
        .msg
        .attachments
        .iter()
        .find(|a| a.id == att)
        .ok_or_else(GraphError::not_found)?;
    Ok(Reply::ok(c.select(attachment_json(a))))
}

// --- delta -----------------------------------------------------------------

/// Delta paging state, carried in `$skiptoken` and `$deltatoken`.
struct DeltaState {
    epoch: u64,
    since: u64,
    snapshot: u64,
    window: Option<(i64, i64)>,
    cursor: Option<(u64, String)>,
}

impl DeltaState {
    fn skip_token(&self, cursor: &(u64, String)) -> String {
        let (ws, we) = self.window.unwrap_or((0, 0));
        format!(
            "{}.{}.{}.{}.{}.{}.{}",
            self.epoch, self.since, self.snapshot, ws, we, cursor.0, cursor.1
        )
    }

    fn delta_token(&self) -> String {
        let (ws, we) = self.window.unwrap_or((0, 0));
        format!("{}.{}.{}.{}", self.epoch, self.snapshot, ws, we)
    }

    fn parse(c: &Ctx<'_>, needs_window: bool) -> Result<Self, GraphError> {
        let gone = || {
            GraphError::new(
                410,
                "SyncStateNotFound",
                "The sync state generation is not found. Start a new full sync.",
            )
        };
        let bad = || GraphError::bad_query("Malformed delta token.");
        let epoch_now = c.st.delta_epoch;
        let to_window = |ws: i64, we: i64| (needs_window || ws != 0 || we != 0).then_some((ws, we));
        if let Some(t) = c.q.get("$deltatoken") {
            let p: Vec<&str> = t.splitn(4, '.').collect();
            let [e, s, ws, we] = p.as_slice() else {
                return Err(bad());
            };
            let epoch: u64 = e.parse().map_err(|_| bad())?;
            if epoch != epoch_now {
                return Err(gone());
            }
            return Ok(Self {
                epoch,
                since: s.parse().map_err(|_| bad())?,
                snapshot: c.st.seq,
                window: to_window(
                    ws.parse().map_err(|_| bad())?,
                    we.parse().map_err(|_| bad())?,
                ),
                cursor: None,
            });
        }
        if let Some(t) = c.q.get("$skiptoken") {
            let p: Vec<&str> = t.splitn(7, '.').collect();
            let [e, s, n, ws, we, cs, ck] = p.as_slice() else {
                return Err(bad());
            };
            let epoch: u64 = e.parse().map_err(|_| bad())?;
            if epoch != epoch_now {
                return Err(gone());
            }
            return Ok(Self {
                epoch,
                since: s.parse().map_err(|_| bad())?,
                snapshot: n.parse().map_err(|_| bad())?,
                window: to_window(
                    ws.parse().map_err(|_| bad())?,
                    we.parse().map_err(|_| bad())?,
                ),
                cursor: Some((cs.parse().map_err(|_| bad())?, (*ck).to_string())),
            });
        }
        let window = if needs_window {
            let (s, e) = window(c)?;
            Some((s.timestamp(), e.timestamp()))
        } else {
            None
        };
        Ok(Self {
            epoch: epoch_now,
            since: 0,
            snapshot: c.st.seq,
            window,
            cursor: None,
        })
    }
}

/// Entries of a change log after `since` and up to `snapshot`, past the cursor.
fn delta_slice<'a>(
    set: Option<&'a BTreeSet<(u64, String)>>,
    ds: &DeltaState,
    limit: usize,
    mut keep: impl FnMut(&str) -> bool,
) -> (Vec<&'a (u64, String)>, bool) {
    let Some(set) = set else {
        return (Vec::new(), false);
    };
    let start = match &ds.cursor {
        Some(c) => Bound::Excluded(c.clone()),
        None => Bound::Included((ds.since + 1, String::new())),
    };
    let mut out = Vec::new();
    for e in set.range((start, Bound::Unbounded)) {
        if e.0 > ds.snapshot {
            break;
        }
        // An initial sync lists what is there now; removals only matter to a client
        // that has synced before.
        if e.1.starts_with("r|") && ds.since == 0 {
            continue;
        }
        if !keep(&e.1) {
            continue;
        }
        if out.len() == limit {
            return (out, true);
        }
        out.push(e);
    }
    (out, false)
}

fn delta_reply(c: &Ctx<'_>, ds: &DeltaState, items: Vec<Value>, last: Option<(u64, String)>) -> R {
    let mut body = json!({ "@odata.context": c.context("Collection(delta)"), "value": items });
    match last {
        Some(cursor) => {
            body["@odata.nextLink"] = json!(c.link(
                &["$skiptoken", "$deltatoken", "startDateTime", "endDateTime"],
                &[("$skiptoken", ds.skip_token(&cursor))]
            ));
        }
        None => {
            body["@odata.deltaLink"] = json!(c.link(
                &["$skiptoken", "$deltatoken", "startDateTime", "endDateTime"],
                &[("$deltatoken", ds.delta_token())]
            ));
        }
    }
    Ok(Reply::ok(body))
}

fn page_size(c: &Ctx<'_>) -> Result<usize, GraphError> {
    let n = match c.prefer.max_page {
        Some(n) => n,
        None => c.q.top(DEFAULT_PAGE)?,
    };
    Ok(n.clamp(1, MAX_PAGE))
}

fn message_delta(c: &Ctx<'_>, folder: &str) -> R {
    let st = c.st;
    let ds = DeltaState::parse(c, false)?;
    let filter = c.q.filter()?;
    let limit = page_size(c)?;
    let (entries, more) = delta_slice(st.msg_log.get(folder), &ds, limit, |key| {
        match (&filter, key.strip_prefix("u|")) {
            (Some(f), Some(id)) => st
                .messages
                .get(id)
                .is_some_and(|r| f.matches(&render::message(r, &c.prefer))),
            _ => true,
        }
    });
    let items = entries
        .iter()
        .map(|(_, key)| match key.strip_prefix("u|") {
            Some(id) if c.sparse_delta && ds.since > 0 => {
                let full = render::message(&st.messages[id], &c.prefer);
                let mut v = serde_json::Map::new();
                for k in ["id", "isRead", "flag"] {
                    if let Some(x) = full.get(k) {
                        v.insert(k.into(), x.clone());
                    }
                }
                Value::Object(v)
            }
            Some(id) => c.select(render::message(&st.messages[id], &c.prefer)),
            None => {
                let mut p = key[2..].splitn(2, '|');
                let imm = p.next().unwrap_or_default();
                let rest = p.next().unwrap_or_default();
                let id = if c.prefer.immutable_ids { imm } else { rest };
                render::removed("#microsoft.graph.message", id)
            }
        })
        .collect();
    let last = more.then(|| entries.last().map(|e| (*e).clone())).flatten();
    delta_reply(c, &ds, items, last)
}

fn folder_delta(c: &Ctx<'_>) -> R {
    let st = c.st;
    let ds = DeltaState::parse(c, false)?;
    let limit = page_size(c)?;
    let (entries, more) = delta_slice(st.folder_log.get(FOLDERS), &ds, limit, |key| {
        key.strip_prefix("u|")
            .is_none_or(|id| st.folder(id).is_some_and(|r| !r.folder.is_hidden))
    });
    let items = entries
        .iter()
        .map(|(_, key)| match key.strip_prefix("u|") {
            Some(id) => c.select(render::folder(st, st.folder(id).expect("in log"))),
            None => {
                let id = key[2..].split('|').next().unwrap_or_default();
                render::removed("#microsoft.graph.mailFolder", id)
            }
        })
        .collect();
    let last = more.then(|| entries.last().map(|e| (*e).clone())).flatten();
    delta_reply(c, &ds, items, last)
}

// --- calendars ---------------------------------------------------------------

fn calendar_id(c: &Ctx<'_>, raw: &str) -> Result<String, GraphError> {
    c.st.resolve_calendar(raw).ok_or_else(GraphError::not_found)
}

fn get_calendar(c: &Ctx<'_>, raw: &str) -> R {
    let id = calendar_id(c, raw)?;
    let cal =
        c.st.calendars
            .iter()
            .find(|x| x.id == id)
            .ok_or_else(GraphError::not_found)?;
    Ok(Reply::ok(c.select(render::calendar(&c.st.user, cal))))
}

fn window(c: &Ctx<'_>) -> Result<(DateTime<Utc>, DateTime<Utc>), GraphError> {
    let missing = || {
        GraphError::new(
            400,
            "ErrorInvalidParameter",
            "This request requires a time window specified by the query string parameters StartDateTime and EndDateTime.",
        )
    };
    let s = c.q.get("startDateTime").ok_or_else(missing)?;
    let e = c.q.get("endDateTime").ok_or_else(missing)?;
    let s = parse_dt(s).ok_or_else(|| GraphError::bad_query("Invalid startDateTime."))?;
    let e = parse_dt(e).ok_or_else(|| GraphError::bad_query("Invalid endDateTime."))?;
    Ok((s, e))
}

fn overlaps(ev: &Event, (s, e): (DateTime<Utc>, DateTime<Utc>)) -> bool {
    ev.start < e && ev.end > s
}

fn list_events(c: &Ctx<'_>, raw: &str, window: Option<(DateTime<Utc>, DateTime<Utc>)>) -> R {
    let st = c.st;
    let cal = calendar_id(c, raw)?;
    let filter = c.q.filter()?;
    let mut evs: Vec<_> = st
        .events
        .values()
        .filter(|r| r.ev.calendar_id == cal)
        // calendarView lists occurrences, never series masters.
        .filter(|r| window.is_none_or(|w| overlaps(&r.ev, w) && r.ev.event_type != "seriesMaster"))
        .collect();
    evs.sort_by(|a, b| (a.ev.start, &a.ev.id).cmp(&(b.ev.start, &b.ev.id)));
    let items: Vec<Value> = evs
        .into_iter()
        .map(|r| render::event(&st.user, r, &c.prefer))
        .filter(|v| filter.as_ref().is_none_or(|f| f.matches(v)))
        .map(|v| c.select(v))
        .collect();
    paged(c, items, "users('me')/events", DEFAULT_PAGE)
}

fn list_contacts(c: &Ctx<'_>, folder: &str) -> R {
    let items: Vec<Value> =
        c.st.contacts
            .iter()
            .filter(|x| x.folder_id == folder)
            .map(|x| {
                c.select(json!({
                    "@odata.type": "#microsoft.graph.contact",
                    "id": x.id,
                    "displayName": x.display_name,
                    "companyName": x.company_name,
                    "emailAddresses": x.email_addresses.iter()
                        .map(|e| json!({"name": e.name, "address": e.address}))
                        .collect::<Vec<_>>(),
                    "mobilePhone": x.mobile_phone,
                    "businessPhones": [],
                    "homePhones": [],
                    "parentFolderId": if folder.is_empty() { "contacts" } else { folder },
                }))
            })
            .collect();
    paged(c, items, "users('me')/contacts", DEFAULT_PAGE)
}

/// A series master's occurrences and exceptions in a window.
fn event_instances(c: &Ctx<'_>, master: &str, w: (DateTime<Utc>, DateTime<Utc>)) -> R {
    let st = c.st;
    if !st.events.contains_key(master) {
        return Err(GraphError::not_found());
    }
    let mut evs: Vec<_> = st
        .events
        .values()
        .filter(|r| r.ev.series_master_id.as_deref() == Some(master) && overlaps(&r.ev, w))
        .collect();
    evs.sort_by(|a, b| (a.ev.start, &a.ev.id).cmp(&(b.ev.start, &b.ev.id)));
    let items: Vec<Value> = evs
        .into_iter()
        .map(|r| c.select(render::event(&st.user, r, &c.prefer)))
        .collect();
    paged(c, items, "users('me')/events", DEFAULT_PAGE)
}

fn get_event(c: &Ctx<'_>, id: &str, raw_cal: &str) -> R {
    let st = c.st;
    let rec = st.events.get(id).ok_or_else(GraphError::not_found)?;
    if !raw_cal.is_empty() && calendar_id(c, raw_cal)? != rec.ev.calendar_id {
        return Err(GraphError::not_found());
    }
    Ok(Reply::ok(c.select(render::event(&st.user, rec, &c.prefer))))
}

fn event_delta(c: &Ctx<'_>, raw: &str) -> R {
    let st = c.st;
    let cal = calendar_id(c, raw)?;
    let ds = DeltaState::parse(c, true)?;
    let (ws, we) = ds.window.unwrap_or((0, 0));
    let w = (
        DateTime::from_timestamp(ws, 0).unwrap_or_default(),
        DateTime::from_timestamp(we, 0).unwrap_or_default(),
    );
    let limit = page_size(c)?;
    let (entries, more) = delta_slice(st.event_log.get(&cal), &ds, limit, |key| {
        key.strip_prefix("u|").is_none_or(|id| {
            st.events
                .get(id)
                .is_some_and(|r| overlaps(&r.ev, w) && r.ev.event_type != "seriesMaster")
        })
    });
    let items = entries
        .iter()
        .map(|(_, key)| match key.strip_prefix("u|") {
            // Like Graph, delta gives occurrences only their ID, type,
            // times and series master; the rest is on the master.
            Some(id) if st.events[id].ev.event_type == "occurrence" => {
                render::sparse_occurrence(&st.events[id])
            }
            Some(id) => c.select(render::event(&st.user, &st.events[id], &c.prefer)),
            None => {
                let id = key[2..].split('|').next().unwrap_or_default();
                render::removed("#microsoft.graph.event", id)
            }
        })
        .collect();
    let last = more.then(|| entries.last().map(|e| (*e).clone())).flatten();
    delta_reply(c, &ds, items, last)
}
