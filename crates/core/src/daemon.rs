// SPDX-License-Identifier: MIT OR Apache-2.0
//! Talking to `famd`: where it lives, the wire protocol and a blocking client.
//!
//! `famd` is the only process that writes to the store. The CLI and GUI read
//! the store directly and send anything that changes it to the daemon over a
//! per-user local socket (Unix) or named pipe (Windows).
//!
//! # Protocol
//!
//! JSON-RPC 2.0, one JSON object per line in each direction. A client sends a
//! [`Request`] and reads one [`Response`] with the same `id`; it may then send
//! another on the same connection. Errors carry the command [`ErrorKind`] in
//! `error.data.kind`, so a failure in the daemon reaches the caller with the
//! same exit code it would have had locally.
//!
//! | Method | Params | Result |
//! | --- | --- | --- |
//! | [`METHOD_STATUS`] | none | [`DaemonStatus`] |
//! | [`METHOD_PIN`] | [`PinParams`] | [`PinResult`] |
//! | [`METHOD_FETCH_BODY`] | [`FetchBodyParams`] | [`FetchBodyResult`] |
//! | [`METHOD_SYNC_NOW`] | [`SyncNowParams`] | `{}` |
//! | [`METHOD_SEARCH_ONLINE`] | [`SearchOnlineParams`] | [`SearchOnlineResult`] |
//! | [`METHOD_CHECK`] | [`CheckParams`] | [`CheckReport`] |
//! | [`METHOD_GRAPH_GET`] | [`GraphGetParams`] | the Graph response's JSON |
//! | [`METHOD_STOP`] | none | `{}`, then the daemon exits |
//!
//! # Where
//!
//! Everything is under the data directory (`FAMILIAR_DATA_DIR`, or the
//! platform's local data directory plus `Familiar`; `%USERPROFILE%\.familiar\data` on
//! Windows, outside AppData): the store in `store/`,
//! the search index in `search/` and, on Unix, the socket `famd.sock` (mode 0600). On Windows the pipe is
//! `\\.\pipe\familiar-famd-<hash of the data directory>`, so each data
//! directory (and each test) gets its own daemon.

use crate::error::{CommandError, ErrorKind};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `daemon.status`: what the daemon is doing for each account.
pub const METHOD_STATUS: &str = "daemon.status";
/// `mail.pin`: pin or unpin a message's body, fetching it if needed.
pub const METHOD_PIN: &str = "mail.pin";
/// `mail.fetch_body`: fetch a body on demand (into the cache when outside the window).
pub const METHOD_FETCH_BODY: &str = "mail.fetch_body";
/// `sync.now`: poll every folder of an account (or all accounts) now.
pub const METHOD_SYNC_NOW: &str = "sync.now";
/// `mail.search_online`: search message text on the server (Graph `$search`).
pub const METHOD_SEARCH_ONLINE: &str = "mail.search_online";
/// `daemon.stop`: shut down cleanly.
pub const METHOD_STOP: &str = "daemon.stop";
/// `sync.check`: compare the store with the server and report drift (WP 1.12).
pub const METHOD_CHECK: &str = "sync.check";

/// `graph.get`: GET a Microsoft Graph path with an account's token (`fam api`).
pub const METHOD_GRAPH_GET: &str = "graph.get";

/// Plans, applies and queues a write: [`crate::ops::SubmitParams`] → [`crate::ops::WriteOutput`].
pub const METHOD_OPS_SUBMIT: &str = "ops.submit";
/// Waits for ops to land: [`crate::ops::WaitParams`] → `Vec<OpView>`.
pub const METHOD_OPS_WAIT: &str = "ops.wait";
/// Recent ops: [`OpsListParams`] → `Vec<OpView>`.
pub const METHOD_OPS_LIST: &str = "ops.list";
/// Retries a failed op: [`OpIdParams`] → `bool`.
pub const METHOD_OPS_RETRY: &str = "ops.retry";
/// Cancels a pending or failed op: [`OpIdParams`] → `bool`.
pub const METHOD_OPS_CANCEL: &str = "ops.cancel";

/// `ops.list` parameters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpsListParams {
    /// Include ops done in the last this many seconds.
    pub done_within_secs: i64,
    /// Most rows.
    pub limit: usize,
}

/// One op by ID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpIdParams {
    /// Op ID.
    pub id: i64,
}

/// The `sync_state` resource the engine sets once an account's first sync of
/// headers has finished.
pub const FIRST_SYNC_RESOURCE: &str = "engine:first-sync";

/// How long a client waits for an answer. A pin can download a body.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// How long `fam sync check` waits: it lists every message on the server.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(3 * 3600);

// ---------------------------------------------------------------------------
// Paths

/// Where the daemon's store and socket are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonPaths {
    /// The data directory.
    pub data_dir: PathBuf,
    /// The store directory.
    pub store: PathBuf,
    /// The search index directory, kept by famd.
    pub index: PathBuf,
    /// The socket path (Unix) or pipe name (Windows).
    pub socket: PathBuf,
}

impl DaemonPaths {
    /// The paths for a data directory.
    pub fn at(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            store: data_dir.join("store"),
            index: data_dir.join("search"),
            socket: socket_for(data_dir),
        }
    }

    /// The paths for the data directory in use (`FAMILIAR_DATA_DIR` or the default).
    pub fn resolve() -> Result<Self, CommandError> {
        let dir = familiar_auth::store::data_dir()
            .map_err(|e| CommandError::new(ErrorKind::Config, e.message))?;
        Ok(Self::at(&dir))
    }
}

#[cfg(unix)]
fn socket_for(data_dir: &Path) -> PathBuf {
    data_dir.join("famd.sock")
}

#[cfg(windows)]
fn socket_for(data_dir: &Path) -> PathBuf {
    PathBuf::from(format!(
        r"\\.\pipe\familiar-famd-{:016x}",
        fnv1a(data_dir.to_string_lossy().to_lowercase().as_bytes())
    ))
}

#[cfg(windows)]
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ---------------------------------------------------------------------------
// Wire types

/// A JSON-RPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Echoed in the response.
    pub id: u64,
    /// One of the `METHOD_*` names.
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: Value,
}

/// A JSON-RPC response: `result` or `error`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// The request's `id` (0 when the request couldn't be read).
    pub id: u64,
    /// The result, on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// The error, on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    /// A success.
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// A failure.
    pub fn err(id: u64, e: &CommandError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(RpcError::from(e)),
        }
    }
}

/// A JSON-RPC error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    /// JSON-RPC code: -32700 parse error, -32601 unknown method, -32602 bad
    /// params, -32000 a command error (see `data.kind`).
    pub code: i64,
    /// Explanation.
    pub message: String,
    /// The command error kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<RpcErrorData>,
}

/// `error.data`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcErrorData {
    /// The command error kind, which fixes the exit code.
    pub kind: ErrorKind,
}

impl From<&CommandError> for RpcError {
    fn from(e: &CommandError) -> Self {
        Self {
            code: match e.kind {
                ErrorKind::Usage => -32602,
                _ => -32000,
            },
            message: e.message.clone(),
            data: Some(RpcErrorData { kind: e.kind }),
        }
    }
}

impl From<RpcError> for CommandError {
    fn from(e: RpcError) -> Self {
        let kind = e.data.map_or(
            match e.code {
                -32602 | -32601 | -32700 => ErrorKind::Usage,
                _ => ErrorKind::Internal,
            },
            |d| d.kind,
        );
        CommandError::new(kind, e.message)
    }
}

/// What the daemon is doing for an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncPhase {
    /// Not started yet.
    Starting,
    /// Syncing the folder tree.
    FolderTree,
    /// First sync: downloading headers for all history.
    Headers,
    /// Downloading bodies for the sync window, newest first.
    Bodies,
    /// Syncing calendars and their events for the calendar window.
    Calendar,
    /// Up to date; polling (Inbox every 30 s, other folders every 5 min).
    Idle,
    /// The last round failed; waiting before trying again.
    Backoff,
    /// The account must sign in again: `fam auth login`.
    AuthRequired,
    /// The daemon is not running (reported by `fam sync status` only).
    Stopped,
}

impl SyncPhase {
    /// The snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            SyncPhase::Starting => "starting",
            SyncPhase::FolderTree => "folder_tree",
            SyncPhase::Headers => "headers",
            SyncPhase::Bodies => "bodies",
            SyncPhase::Calendar => "calendar",
            SyncPhase::Idle => "idle",
            SyncPhase::Backoff => "backoff",
            SyncPhase::AuthRequired => "auth_required",
            SyncPhase::Stopped => "stopped",
        }
    }
}

/// The daemon's live view of one account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccountLive {
    /// The account's key in `config.toml`.
    pub account: String,
    /// What it is doing.
    pub phase: SyncPhase,
    /// The last error, while it lasts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// When the last round finished (Unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_round_at: Option<i64>,
    /// When the next poll is due (Unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_poll_at: Option<i64>,
    /// Headers added or updated since the daemon started.
    pub headers_synced: u64,
    /// Bodies downloaded since the daemon started.
    pub bodies_fetched: u64,
    /// Rounds that failed since the daemon started.
    pub failures: u64,
}

/// `daemon.status` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DaemonStatus {
    /// The daemon's version.
    pub version: String,
    /// Its process ID.
    pub pid: u32,
    /// When it started (Unix seconds).
    pub started_at: i64,
    /// One entry per account it syncs.
    pub accounts: Vec<AccountLive>,
}

/// `mail.pin` params.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinParams {
    /// Local message ID.
    pub message: i64,
    /// True to pin, false to unpin.
    pub pinned: bool,
}

/// `mail.pin` result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PinResult {
    /// Local message ID.
    pub message: i64,
    /// Whether the body is now pinned.
    pub pinned: bool,
    /// Whether the body is on this device now.
    pub has_body: bool,
    /// Whether the body was downloaded to pin it.
    pub fetched: bool,
}

/// `mail.fetch_body` params.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchBodyParams {
    /// Local message ID.
    pub message: i64,
}

/// `mail.fetch_body` result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchBodyResult {
    /// Local message ID.
    pub message: i64,
    /// Whether it was downloaded (false if it was already local).
    pub fetched: bool,
    /// Whether it went into the on-demand cache (outside the window, not pinned).
    pub cached: bool,
}

/// `sync.now` params.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncNowParams {
    /// Account key; all accounts when absent.
    #[serde(default)]
    pub account: Option<String>,
}

/// `mail.search_online` params.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOnlineParams {
    /// The `$search` value, with its surrounding quotes
    /// (see [`familiar_search::to_kql`]).
    pub kql: String,
    /// Only these accounts (config name or address); all when empty.
    #[serde(default)]
    pub accounts: Vec<String>,
    /// Most results per account (Graph's cap is 1,000).
    pub limit: usize,
}

/// One account the server search failed for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccountFailure {
    /// The account name.
    pub account: String,
    /// What went wrong, as a command error kind.
    pub kind: ErrorKind,
    /// The error.
    pub message: String,
}

/// `mail.search_online` result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchOnlineResult {
    /// What the server found, newest first within each account.
    pub hits: Vec<familiar_search::OnlineHit>,
    /// Accounts searched successfully.
    #[serde(default)]
    pub searched: Vec<String>,
    /// Accounts whose search failed.
    #[serde(default)]
    pub failures: Vec<AccountFailure>,
}

/// `graph.get` params.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GraphGetParams {
    /// Account key; may be left out when famd syncs only one account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// A path under `https://graph.microsoft.com/v1.0` such as
    /// `/me/mailFolders?$top=5`, or a full link Graph returned (`@odata.nextLink`).
    pub path: String,
}

/// `sync.check` params.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CheckParams {
    /// Account key; all accounts when absent.
    #[serde(default)]
    pub account: Option<String>,
    /// Most drift entries listed per account (counts are always complete).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Skip the sync round before and between the passes. Recent changes then
    /// show as drift.
    #[serde(default)]
    pub no_settle: bool,
}

/// What kind of difference the check found.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    /// A folder on the server is not in the store.
    FolderMissing,
    /// A folder in the store is not on the server.
    FolderExtra,
    /// A folder's name or parent differs.
    FolderChanged,
    /// A message on the server is not in the store.
    MessageMissing,
    /// A message in the store is not on the server.
    MessageExtra,
    /// A message is in a different folder in the store.
    WrongFolder,
    /// Read state differs.
    ReadState,
    /// Follow-up flag differs.
    Flag,
    /// Received time differs.
    ReceivedAt,
    /// A message inside the sync window has no body on this device.
    BodyMissing,
    /// A body outside the sync window is kept although it is neither pinned nor cached.
    BodyOutsideWindow,
    /// A folder or thread counter in the store disagrees with its messages.
    Counter,
}

impl DriftKind {
    /// The snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            DriftKind::FolderMissing => "folder_missing",
            DriftKind::FolderExtra => "folder_extra",
            DriftKind::FolderChanged => "folder_changed",
            DriftKind::MessageMissing => "message_missing",
            DriftKind::MessageExtra => "message_extra",
            DriftKind::WrongFolder => "wrong_folder",
            DriftKind::ReadState => "read_state",
            DriftKind::Flag => "flag",
            DriftKind::ReceivedAt => "received_at",
            DriftKind::BodyMissing => "body_missing",
            DriftKind::BodyOutsideWindow => "body_outside_window",
            DriftKind::Counter => "counter",
        }
    }
}

/// One difference between the store and the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Drift {
    /// What differs.
    pub kind: DriftKind,
    /// The folder (its name in the store, or on the server when missing locally).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// The message's server ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    /// The message's local ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<i64>,
    /// What the store has against what the server has.
    pub detail: String,
}

/// One drift kind and how often it was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DriftCount {
    /// The kind.
    pub kind: DriftKind,
    /// How many.
    pub count: usize,
}

/// One account's check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AccountCheck {
    /// The account's key in `config.toml`.
    pub account: String,
    /// Whether the first sync had finished. Before it, drift is expected.
    pub first_sync_done: bool,
    /// Whether a sync round ran before the comparison and before re-checking.
    pub settled: bool,
    /// Folders compared.
    pub folders: usize,
    /// Messages listed on the server (inside the kept header range).
    pub server_messages: usize,
    /// Messages in the store (inside the kept header range).
    pub local_messages: usize,
    /// Differences that the first pass found but were gone when re-checked:
    /// changes that were still syncing.
    pub settled_on_recheck: usize,
    /// Total differences that remain.
    pub drift_total: usize,
    /// Remaining differences by kind.
    pub by_kind: Vec<DriftCount>,
    /// The first differences (up to the limit).
    pub drift: Vec<Drift>,
    /// Seconds the check took.
    pub seconds: f64,
}

/// `sync.check` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CheckReport {
    /// True when no account has drift.
    pub clean: bool,
    /// One entry per account checked.
    pub accounts: Vec<AccountCheck>,
}

// ---------------------------------------------------------------------------
// Client

trait Duplex: Read + Write + Send {}
impl<T: Read + Write + Send> Duplex for T {}

/// A blocking connection to `famd`.
pub struct Client {
    stream: BufReader<Box<dyn Duplex>>,
    next_id: u64,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

fn unavailable(socket: &Path, e: impl std::fmt::Display) -> CommandError {
    CommandError::new(
        ErrorKind::DaemonUnavailable,
        format!(
            "famd is not running ({}: {e}); start it with `famd`",
            socket.display()
        ),
    )
}

impl Client {
    /// Connects to the daemon listening on `socket`. Fails with
    /// [`ErrorKind::DaemonUnavailable`] when none is.
    pub fn connect(socket: &Path) -> Result<Self, CommandError> {
        Self::connect_with_timeout(socket, CALL_TIMEOUT)
    }

    /// Like [`Client::connect`], waiting up to `timeout` for each answer
    /// (Unix; Windows pipes wait without limit).
    pub fn connect_with_timeout(socket: &Path, timeout: Duration) -> Result<Self, CommandError> {
        let stream = open(socket, timeout).map_err(|e| unavailable(socket, e))?;
        Ok(Self {
            stream: BufReader::new(stream),
            next_id: 1,
        })
    }

    /// Connects to the daemon for the data directory in use.
    pub fn connect_default() -> Result<Self, CommandError> {
        Self::connect(&DaemonPaths::resolve()?.socket)
    }

    /// Calls `method` and decodes its result.
    pub fn call<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: &P,
    ) -> Result<R, CommandError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = Request {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params: serde_json::to_value(params)
                .map_err(|e| CommandError::internal(format!("encode request: {e}")))?,
        };
        let mut line = serde_json::to_string(&req)
            .map_err(|e| CommandError::internal(format!("encode request: {e}")))?;
        line.push('\n');
        let lost = |e: std::io::Error| {
            CommandError::new(
                ErrorKind::DaemonUnavailable,
                format!("lost the connection to famd: {e}"),
            )
        };
        let w = self.stream.get_mut();
        w.write_all(line.as_bytes()).map_err(lost)?;
        w.flush().map_err(lost)?;
        let mut answer = String::new();
        if self.stream.read_line(&mut answer).map_err(lost)? == 0 {
            return Err(lost(std::io::ErrorKind::UnexpectedEof.into()));
        }
        let resp: Response = serde_json::from_str(&answer)
            .map_err(|e| CommandError::internal(format!("bad answer from famd: {e}")))?;
        if resp.id != id {
            return Err(CommandError::internal(format!(
                "famd answered request {} instead of {id}",
                resp.id
            )));
        }
        if let Some(e) = resp.error {
            return Err(e.into());
        }
        serde_json::from_value(resp.result.unwrap_or(Value::Null))
            .map_err(|e| CommandError::internal(format!("bad answer from famd: {e}")))
    }
}

#[cfg(unix)]
fn open(socket: &Path, timeout: Duration) -> std::io::Result<Box<dyn Duplex>> {
    let s = std::os::unix::net::UnixStream::connect(socket)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    Ok(Box::new(s))
}

#[cfg(windows)]
fn open(socket: &Path, _timeout: Duration) -> std::io::Result<Box<dyn Duplex>> {
    // All pipe instances busy (ERROR_PIPE_BUSY): the daemon makes a new one
    // as soon as it accepts, so retry briefly.
    const ERROR_PIPE_BUSY: i32 = 231;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(socket)
        {
            Ok(f) => return Ok(Box::new(f)),
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY)
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_keep_their_kind_across_the_wire() {
        let e = CommandError::new(ErrorKind::NotFound, "message 7 not found");
        let wire = serde_json::to_string(&Response::err(3, &e)).unwrap();
        let back: Response = serde_json::from_str(&wire).unwrap();
        let e2: CommandError = back.error.unwrap().into();
        assert_eq!(e2, e);
    }

    #[test]
    fn no_daemon_is_daemon_unavailable() {
        let dir = std::env::temp_dir().join(format!("fam-no-daemon-{}", std::process::id()));
        let p = DaemonPaths::at(&dir);
        let e = Client::connect(&p.socket).unwrap_err();
        assert_eq!(e.kind, ErrorKind::DaemonUnavailable);
        assert_eq!(e.exit_code(), 11);
    }
}
