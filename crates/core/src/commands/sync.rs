// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam sync status` (WP 1.7): how far each account's sync has got.
//!
//! Counts come from the store, which the CLI reads directly, so they are
//! shown even when `famd` is not running. What the daemon is doing right
//! now (first sync, bodies, idle, backing off) comes from the daemon over
//! its socket; without a daemon each account shows as `stopped`.

use crate::commands::auth::{config, rfc3339};
use crate::daemon::{
    AccountLive, Client, DaemonPaths, DaemonStatus, FIRST_SYNC_RESOURCE, METHOD_STATUS, SyncPhase,
};
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use familiar_store::Reader;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `fam sync status`.
pub struct SyncStatus;

/// Input for `fam sync status`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SyncStatusInput {
    /// Only this account (its name in config.toml).
    #[serde(default)]
    pub account: Option<String>,
}

/// The daemon, as `fam sync status` found it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DaemonInfo {
    /// Whether `famd` answered.
    pub running: bool,
    /// Its version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Its process ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// When it started (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
}

/// A sync error recorded for one resource (a folder, or the folder tree).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncError {
    /// The resource: `folders`, or `messages:<folder>`.
    pub resource: String,
    /// The error.
    pub message: String,
}

/// One account's sync.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AccountSyncStatus {
    /// The account name.
    pub account: String,
    /// The mailbox address.
    pub email: String,
    /// What the daemon is doing; `stopped` without a daemon.
    pub phase: SyncPhase,
    /// The daemon's last error for the account, while it lasts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Whether the first sync of headers for all history has finished.
    pub first_sync_done: bool,
    /// Folders.
    pub folders: i64,
    /// Messages (headers) on this device.
    pub messages: i64,
    /// Unread messages.
    pub unread: i64,
    /// Messages whose body is on this device.
    pub bodies: i64,
    /// Messages inside the sync window still waiting for a body.
    pub bodies_missing: i64,
    /// Pinned bodies.
    pub pinned: i64,
    /// Bodies in the on-demand cache (older than the window).
    pub cached: i64,
    /// Compressed bytes in the on-demand cache.
    pub cached_bytes: i64,
    /// When a folder last synced (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<String>,
    /// When the next poll is due (RFC 3339, UTC), from the daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_poll_at: Option<String>,
    /// Folders whose last sync failed.
    pub errors: Vec<SyncError>,
}

/// Output of `fam sync status`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncStatusOutput {
    /// The daemon.
    pub daemon: DaemonInfo,
    /// The store directory.
    pub store: String,
    /// Compressed bytes of every body on this device.
    pub body_bytes: i64,
    /// The body window from config.toml (`sync.window`).
    pub window: String,
    /// One entry per account in the store.
    pub accounts: Vec<AccountSyncStatus>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn time(secs: i64) -> String {
    rfc3339(secs.max(0) as u64)
}

fn store_err(e: familiar_store::Error) -> CommandError {
    CommandError::internal(format!("store: {e}"))
}

/// Builds the status for the store and socket at `paths`, with the body
/// window `window` (as in `sync.window`) ending at `now` (Unix seconds).
/// Used by the command and by tests.
pub fn status_at(
    paths: &DaemonPaths,
    window: &str,
    account: Option<&str>,
    now: i64,
) -> Result<SyncStatusOutput, CommandError> {
    let window_days = familiar_config::validate::parse_period(window).ok_or_else(|| {
        CommandError::new(ErrorKind::Config, format!("bad sync.window {window:?}"))
    })?;
    let since = now - i64::from(window_days) * 86_400;

    let live: Option<DaemonStatus> = Client::connect(&paths.socket)
        .and_then(|mut c| c.call(METHOD_STATUS, &serde_json::json!({})))
        .ok();
    let daemon = match &live {
        Some(s) => DaemonInfo {
            running: true,
            version: Some(s.version.clone()),
            pid: Some(s.pid),
            started_at: Some(time(s.started_at)),
        },
        None => DaemonInfo {
            running: false,
            version: None,
            pid: None,
            started_at: None,
        },
    };

    let mut out = SyncStatusOutput {
        daemon,
        store: paths.store.display().to_string(),
        body_bytes: 0,
        window: window.to_string(),
        accounts: Vec::new(),
    };
    let mut reader = match Reader::open(&paths.store) {
        Ok(r) => r,
        Err(familiar_store::Error::NoStore(_)) => {
            if let Some(a) = account {
                return Err(CommandError::new(
                    ErrorKind::NotFound,
                    format!("account {a:?} has not synced on this device"),
                ));
            }
            return Ok(out);
        }
        Err(e) => return Err(store_err(e)),
    };
    let live_of = |key: &str| -> Option<&AccountLive> {
        live.as_ref()?.accounts.iter().find(|a| a.account == key)
    };
    reader
        .snapshot(|r| {
            out.body_bytes = r.stored_body_bytes()?;
            for acc in r.accounts()? {
                if account.is_some_and(|a| a != acc.key) {
                    continue;
                }
                let folders = r.folders(acc.id)?;
                let stats = r.body_stats(acc.id, since)?;
                let states = r.sync_states(acc.id)?;
                let first_sync_done = states
                    .iter()
                    .any(|s| s.resource == FIRST_SYNC_RESOURCE && s.synced_at.is_some());
                let last = states
                    .iter()
                    .filter(|s| s.resource != FIRST_SYNC_RESOURCE)
                    .filter_map(|s| s.synced_at)
                    .max();
                let errors = states
                    .iter()
                    .filter_map(|s| {
                        s.last_error.as_ref().map(|e| SyncError {
                            resource: s.resource.clone(),
                            message: e.clone(),
                        })
                    })
                    .collect();
                let l = live_of(&acc.key);
                out.accounts.push(AccountSyncStatus {
                    account: acc.key.clone(),
                    email: acc.email.clone(),
                    phase: l.map_or(SyncPhase::Stopped, |l| l.phase),
                    detail: l.and_then(|l| l.detail.clone()),
                    first_sync_done,
                    folders: folders.len() as i64,
                    messages: folders.iter().map(|f| f.total).sum(),
                    unread: folders.iter().map(|f| f.unread).sum(),
                    bodies: stats.with_body,
                    bodies_missing: stats.window_missing,
                    pinned: stats.pinned,
                    cached: stats.cached,
                    cached_bytes: stats.cached_bytes,
                    last_synced_at: last.map(time),
                    next_poll_at: l.and_then(|l| l.next_poll_at).map(time),
                    errors,
                });
            }
            Ok(())
        })
        .map_err(store_err)?;
    if let Some(a) = account {
        if out.accounts.is_empty() {
            return Err(CommandError::new(
                ErrorKind::NotFound,
                format!("account {a:?} has not synced on this device"),
            ));
        }
    }
    Ok(out)
}

fn human_bytes(n: i64) -> String {
    let n = n.max(0) as f64;
    if n >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", n / (1024.0 * 1024.0 * 1024.0))
    } else if n >= 1024.0 * 1024.0 {
        format!("{:.1} MB", n / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KB", n / 1024.0)
    }
}

impl Command for SyncStatus {
    type Input = SyncStatusInput;
    type Output = SyncStatusOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["sync", "status"],
        summary: "Show how far each account's sync has got",
        description: "Shows, per account, what famd is doing (first sync, bodies, idle), how many headers and bodies are on this device, what is still to fetch for the sync window, pinned and cached bodies, and any folder errors. Counts are read from the store, so they show even when famd is not running.",
        access: Access::Read,
        errors: &[ErrorKind::Config, ErrorKind::NotFound],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: SyncStatusInput,
    ) -> Result<SyncStatusOutput, CommandError> {
        let window = config()?.map_or_else(|| "1y".to_string(), |c| c.sync.window);
        status_at(
            &DaemonPaths::resolve()?,
            &window,
            input.account.as_deref(),
            now(),
        )
    }

    fn render_text(&self, o: &SyncStatusOutput) -> String {
        let mut s = match (&o.daemon.running, &o.daemon.pid) {
            (true, Some(pid)) => format!("famd running (pid {pid})\n"),
            _ => "famd is not running; counts are from the store\n".to_string(),
        };
        if o.accounts.is_empty() {
            s.push_str("No accounts have synced on this device yet.");
            return s;
        }
        for a in &o.accounts {
            s.push_str(&format!(
                "\n{} <{}>: {}{}\n",
                a.account,
                a.email,
                a.phase.as_str().replace('_', " "),
                if a.first_sync_done {
                    ""
                } else {
                    " (first sync)"
                }
            ));
            if let Some(d) = &a.detail {
                s.push_str(&format!("  last error: {d}\n"));
            }
            s.push_str(&format!(
                "  {} messages in {} folders, {} unread\n",
                a.messages, a.folders, a.unread
            ));
            s.push_str(&format!(
                "  bodies: {} on this device, {} still to fetch for the {} window, {} pinned, {} cached ({})\n",
                a.bodies,
                a.bodies_missing,
                o.window,
                a.pinned,
                a.cached,
                human_bytes(a.cached_bytes)
            ));
            if let Some(t) = &a.last_synced_at {
                s.push_str(&format!("  last synced {t}"));
                if let Some(n) = &a.next_poll_at {
                    s.push_str(&format!(", next poll {n}"));
                }
                s.push('\n');
            }
            for e in &a.errors {
                s.push_str(&format!("  error in {}: {}\n", e.resource, e.message));
            }
        }
        s.push_str(&format!(
            "\nBodies use {} on disk.",
            human_bytes(o.body_bytes)
        ));
        s
    }
}
