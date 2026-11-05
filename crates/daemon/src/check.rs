// SPDX-License-Identifier: MIT OR Apache-2.0
//! The store-versus-server consistency check (WP 1.12): `sync.check`.
//!
//! It lists every folder and every message on the server and compares them
//! with the store: folders (present, name, parent), messages (present, folder,
//! read state, flag, received time), bodies against the sync window, and the
//! store's own derived counters. It only reads. Nothing it finds is fixed;
//! drift means a sync bug to investigate.
//!
//! The mailbox keeps changing while the check runs, so a difference seen once
//! is not yet drift. The check runs in two passes:
//!
//! 1. Ask the engine for a round (`sync.now`) and wait until it is idle, then
//!    list the server folder by folder and compare.
//! 2. If anything differed, ask for another round, then fetch each differing
//!    message from the server again and re-read it from the store. Only what
//!    still differs is reported; the rest is counted as `settled_on_recheck`.
//!
//! Memory stays flat: one folder's listing is held at a time, plus the
//! messages that were missing from one folder or extra in another, which are
//! matched up at the end as moves (`wrong_folder`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use familiar_core::daemon::{AccountCheck, Drift, DriftCount, DriftKind, FIRST_SYNC_RESOURCE};
use familiar_core::{CommandError, ErrorKind};
use familiar_provider_graph::{Error as GraphError, GraphMessage, MailApi, Priority};
use familiar_store::{AccountId, Cursor, Folder, FolderId, Message, Reader};

use crate::engine::AccountShared;
use crate::policy::{DAY, SyncPolicy};
use crate::service::command_error;

/// Drift entries listed per account unless the caller asks otherwise.
pub const DEFAULT_LIMIT: usize = 50;

/// Most differing messages fetched again in the second pass. Past this the
/// rest are reported as found.
pub const RECHECK_LIMIT: usize = 2_000;

/// How long to wait for the engine to finish a round.
pub const SETTLE_TIMEOUT: Duration = Duration::from_secs(1800);

/// What the comparison needs to know besides the two sides.
#[derive(Debug, Clone, Copy)]
pub struct Rules {
    /// Bodies are expected for messages received at or after this.
    pub window_start: i64,
    /// A kept body older than this (neither pinned nor cached) is drift. A
    /// little before `window_start`, since ageing runs on a timer.
    pub outside_window_before: i64,
    /// Headers older than this are pruned on purpose and not compared.
    pub headers_start: Option<i64>,
}

impl Rules {
    /// The rules for `policy` at `now`.
    pub fn new(policy: &SyncPolicy, now: i64) -> Self {
        let window_start = policy.window_start(now);
        // Two ageing runs, and never less than a day: a body may sit just
        // past the edge until the next one.
        let grace = (2 * policy.ageing_interval.as_secs() as i64).max(DAY);
        Self {
            window_start,
            outside_window_before: window_start - grace,
            headers_start: policy.headers_start(now),
        }
    }

    fn compared(&self, received_at: i64) -> bool {
        self.headers_start.is_none_or(|h| received_at >= h)
    }
}

/// A message as the server lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMessage {
    /// Server ID.
    pub id: String,
    /// Server ID of its folder.
    pub folder: String,
    /// Received time (Unix seconds).
    pub received_at: i64,
    /// Read state.
    pub is_read: bool,
    /// Follow-up flag.
    pub is_flagged: bool,
}

impl RemoteMessage {
    /// From a Graph listing of `folder` (used when Graph leaves out `parentFolderId`).
    pub fn from_graph(m: &GraphMessage, folder: &str) -> Self {
        Self {
            id: m.id.clone(),
            folder: m
                .parent_folder_id
                .clone()
                .unwrap_or_else(|| folder.to_string()),
            received_at: m
                .received_date_time
                .as_deref()
                .and_then(familiar_provider_graph::parse_time)
                .unwrap_or(0),
            is_read: m.is_read,
            is_flagged: m.is_flagged(),
        }
    }
}

/// A folder on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFolder {
    /// Server ID.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Parent's server ID, when the parent is itself a listed folder.
    pub parent: Option<String>,
}

/// Local folders by ID, with remote IDs, for naming things in the report.
struct LocalFolders {
    by_id: HashMap<FolderId, Folder>,
}

impl LocalFolders {
    fn remote(&self, id: FolderId) -> String {
        self.by_id
            .get(&id)
            .map_or_else(|| format!("local folder {}", id.0), |f| f.remote_id.clone())
    }

    fn name(&self, id: FolderId) -> String {
        self.by_id
            .get(&id)
            .map_or_else(|| format!("local folder {}", id.0), |f| f.name.clone())
    }
}

/// Compares the folder trees.
pub fn compare_folders(local: &[Folder], remote: &[RemoteFolder]) -> Vec<Drift> {
    let mut out = Vec::new();
    let by_local_id: HashMap<FolderId, &Folder> = local.iter().map(|f| (f.id, f)).collect();
    let by_remote: HashMap<&str, &Folder> =
        local.iter().map(|f| (f.remote_id.as_str(), f)).collect();
    let remote_ids: HashSet<&str> = remote.iter().map(|f| f.id.as_str()).collect();
    let remote_name: HashMap<&str, &str> = remote
        .iter()
        .map(|f| (f.id.as_str(), f.name.as_str()))
        .collect();
    for rf in remote {
        let Some(lf) = by_remote.get(rf.id.as_str()) else {
            out.push(Drift {
                kind: DriftKind::FolderMissing,
                folder: Some(rf.name.clone()),
                remote_id: Some(rf.id.clone()),
                message: None,
                detail: "on the server, not in the store".into(),
            });
            continue;
        };
        let local_parent = lf
            .parent
            .and_then(|p| by_local_id.get(&p))
            .map(|p| p.remote_id.as_str());
        let remote_parent = rf.parent.as_deref().filter(|p| remote_ids.contains(p));
        if lf.name != rf.name {
            out.push(Drift {
                kind: DriftKind::FolderChanged,
                folder: Some(lf.name.clone()),
                remote_id: Some(rf.id.clone()),
                message: None,
                detail: format!("named {:?} here, {:?} on the server", lf.name, rf.name),
            });
        }
        if local_parent != remote_parent {
            let name = |p: Option<&str>| {
                p.map_or("the top level".to_string(), |p| {
                    remote_name
                        .get(p)
                        .map_or(p.to_string(), |n| format!("{n:?}"))
                })
            };
            out.push(Drift {
                kind: DriftKind::FolderChanged,
                folder: Some(lf.name.clone()),
                remote_id: Some(rf.id.clone()),
                message: None,
                detail: format!(
                    "under {} here, {} on the server",
                    name(local_parent),
                    name(remote_parent)
                ),
            });
        }
    }
    for lf in local {
        if !remote_ids.contains(lf.remote_id.as_str()) {
            out.push(Drift {
                kind: DriftKind::FolderExtra,
                folder: Some(lf.name.clone()),
                remote_id: Some(lf.remote_id.clone()),
                message: None,
                detail: format!("in the store with {} messages, not on the server", lf.total),
            });
        }
    }
    out
}

/// Compares one message, given what each side has (either may be absent).
/// `local_folder` is the remote ID of the local row's folder.
fn compare_message(
    rules: &Rules,
    folders: &LocalFolders,
    remote: Option<&RemoteMessage>,
    remote_folder_name: &dyn Fn(&str) -> String,
    local: Option<&Message>,
) -> Vec<Drift> {
    let mut out = Vec::new();
    let drift = |kind, folder: String, detail: String| Drift {
        kind,
        folder: Some(folder),
        remote_id: remote
            .map(|r| r.id.clone())
            .or_else(|| local.map(|l| l.remote_id.clone())),
        message: local.map(|l| l.id.0),
        detail,
    };
    match (remote, local) {
        (None, None) => {}
        (Some(r), None) => {
            if rules.compared(r.received_at) {
                out.push(drift(
                    DriftKind::MessageMissing,
                    remote_folder_name(&r.folder),
                    "on the server, not in the store".into(),
                ));
            }
        }
        (None, Some(l)) => {
            if rules.compared(l.received_at) {
                out.push(drift(
                    DriftKind::MessageExtra,
                    folders.name(l.folder),
                    "in the store, not on the server".into(),
                ));
            }
        }
        (Some(r), Some(l)) => {
            let here = folders.name(l.folder);
            if folders.remote(l.folder) != r.folder {
                out.push(drift(
                    DriftKind::WrongFolder,
                    here.clone(),
                    format!(
                        "in {here:?} here, {:?} on the server",
                        remote_folder_name(&r.folder)
                    ),
                ));
            }
            if l.is_read != r.is_read {
                let s = |b: bool| if b { "read" } else { "unread" };
                out.push(drift(
                    DriftKind::ReadState,
                    here.clone(),
                    format!("{} here, {} on the server", s(l.is_read), s(r.is_read)),
                ));
            }
            if l.is_flagged != r.is_flagged {
                let s = |b: bool| if b { "flagged" } else { "not flagged" };
                out.push(drift(
                    DriftKind::Flag,
                    here.clone(),
                    format!(
                        "{} here, {} on the server",
                        s(l.is_flagged),
                        s(r.is_flagged)
                    ),
                ));
            }
            if l.received_at != r.received_at {
                out.push(drift(
                    DriftKind::ReceivedAt,
                    here.clone(),
                    format!(
                        "received {} here, {} on the server",
                        l.received_at, r.received_at
                    ),
                ));
            }
        }
    }
    if let Some(l) = local {
        if rules.compared(l.received_at) {
            out.extend(body_drift(rules, folders, l));
        }
    }
    out
}

fn body_drift(rules: &Rules, folders: &LocalFolders, l: &Message) -> Option<Drift> {
    let (kind, detail) = if l.received_at >= rules.window_start && l.body.is_none() {
        (
            DriftKind::BodyMissing,
            "inside the sync window without a body",
        )
    } else if l.received_at < rules.outside_window_before
        && l.body.is_some()
        && !l.body_pinned
        && l.body_cached_at.is_none()
    {
        (
            DriftKind::BodyOutsideWindow,
            "outside the sync window with a body that is neither pinned nor cached",
        )
    } else {
        return None;
    };
    Some(Drift {
        kind,
        folder: Some(folders.name(l.folder)),
        remote_id: Some(l.remote_id.clone()),
        message: Some(l.id.0),
        detail: detail.into(),
    })
}

/// The first pass's result.
#[derive(Debug, Default)]
pub struct Pass {
    /// Folder and counter drift.
    pub structural: Vec<Drift>,
    /// Message drift, keyed by server ID (all of one message's entries together).
    pub messages: BTreeMap<String, Vec<Drift>>,
    /// Folders compared.
    pub folders: usize,
    /// Server messages compared.
    pub server_messages: usize,
    /// Store messages compared.
    pub local_messages: usize,
}

impl Pass {
    fn total(&self) -> usize {
        self.structural.len() + self.messages.values().map(Vec::len).sum::<usize>()
    }
}

/// Lists one folder's messages on the server.
async fn list_remote(api: &MailApi, folder: &str) -> Result<Vec<RemoteMessage>, GraphError> {
    let mut out = Vec::new();
    let mut link: Option<String> = None;
    loop {
        let page = api
            .message_states_page(folder, link.as_deref(), Priority::User)
            .await?;
        out.extend(
            page.items
                .iter()
                .map(|m| RemoteMessage::from_graph(m, folder)),
        );
        match page.next_link {
            Some(n) => link = Some(n),
            None => return Ok(out),
        }
    }
}

fn list_local(r: &Reader, folder: FolderId) -> familiar_store::Result<Vec<Message>> {
    let mut out = Vec::new();
    let mut after = None;
    loop {
        let page = r.messages(folder, after, 5000)?;
        let Some(last) = page.last() else {
            return Ok(out);
        };
        after = Some(Cursor {
            received_at: last.received_at,
            id: last.id.0,
        });
        out.extend(page);
    }
}

fn store_error(e: familiar_store::Error) -> CommandError {
    CommandError::internal(format!("store: {e}"))
}

/// The server's visible folder tree.
pub async fn remote_folders(api: &MailApi) -> Result<Vec<RemoteFolder>, CommandError> {
    Ok(api
        .folder_tree(Priority::User)
        .await
        .map_err(command_error)?
        .into_iter()
        .map(|f| RemoteFolder {
            id: f.id,
            name: f.display_name,
            parent: f.parent_folder_id,
        })
        .collect())
}

/// The first pass: every folder and message on both sides.
pub async fn full_pass(
    api: &MailApi,
    root: &std::path::Path,
    account: AccountId,
    rules: &Rules,
) -> Result<Pass, CommandError> {
    let mut pass = Pass::default();
    let remote_tree = remote_folders(api).await?;
    let r = Reader::open(root).map_err(store_error)?;
    let local_tree = r.folders(account).map_err(store_error)?;
    pass.structural = compare_folders(&local_tree, &remote_tree);
    let folders = LocalFolders {
        by_id: local_tree.iter().map(|f| (f.id, f.clone())).collect(),
    };
    let remote_names: HashMap<String, String> = remote_tree
        .iter()
        .map(|f| (f.id.clone(), f.name.clone()))
        .collect();
    let remote_name = |id: &str| {
        remote_names
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    };
    let local_by_remote: HashMap<&str, &Folder> = local_tree
        .iter()
        .map(|f| (f.remote_id.as_str(), f))
        .collect();

    // Messages seen on one side of a folder only, matched up at the end.
    let mut only_remote: HashMap<String, RemoteMessage> = HashMap::new();
    let mut only_local: HashMap<String, Message> = HashMap::new();
    let add = |pass: &mut Pass, d: Vec<Drift>| {
        for d in d {
            let key = d.remote_id.clone().unwrap_or_default();
            pass.messages.entry(key).or_default().push(d);
        }
    };

    for rf in &remote_tree {
        let remote = list_remote(api, &rf.id).await.map_err(command_error)?;
        pass.folders += 1;
        pass.server_messages += remote
            .iter()
            .filter(|m| rules.compared(m.received_at))
            .count();
        let mut local: HashMap<String, Message> = match local_by_remote.get(rf.id.as_str()) {
            Some(lf) => list_local(&r, lf.id)
                .map_err(store_error)?
                .into_iter()
                .map(|m| (m.remote_id.clone(), m))
                .collect(),
            None => HashMap::new(),
        };
        pass.local_messages += local
            .values()
            .filter(|m| rules.compared(m.received_at))
            .count();
        for m in remote {
            match local.remove(&m.id) {
                Some(l) => add(
                    &mut pass,
                    compare_message(rules, &folders, Some(&m), &remote_name, Some(&l)),
                ),
                None => {
                    only_remote.insert(m.id.clone(), m);
                }
            }
        }
        only_local.extend(local);
    }
    // Store folders the server doesn't have: their messages are extra (or moved).
    let remote_ids: HashSet<&str> = remote_tree.iter().map(|f| f.id.as_str()).collect();
    for lf in local_tree
        .iter()
        .filter(|f| !remote_ids.contains(f.remote_id.as_str()))
    {
        let local = list_local(&r, lf.id).map_err(store_error)?;
        pass.local_messages += local
            .iter()
            .filter(|m| rules.compared(m.received_at))
            .count();
        only_local.extend(local.into_iter().map(|m| (m.remote_id.clone(), m)));
    }
    for (id, m) in only_remote {
        let l = only_local.remove(&id);
        add(
            &mut pass,
            compare_message(rules, &folders, Some(&m), &remote_name, l.as_ref()),
        );
    }
    for (_, l) in only_local {
        add(
            &mut pass,
            compare_message(rules, &folders, None, &remote_name, Some(&l)),
        );
    }
    pass.messages.retain(|_, v| !v.is_empty());

    let wrong = r.derived_drift().map_err(store_error)?;
    if wrong > 0 {
        pass.structural.push(counter_drift(wrong));
    }
    Ok(pass)
}

fn counter_drift(wrong: usize) -> Drift {
    Drift {
        kind: DriftKind::Counter,
        folder: None,
        remote_id: None,
        message: None,
        detail: format!("{wrong} folder or thread counters disagree with their messages"),
    }
}

/// The second pass: everything that differed, looked at again.
pub async fn recheck(
    api: &MailApi,
    root: &std::path::Path,
    account: AccountId,
    rules: &Rules,
    first: Pass,
) -> Result<Pass, CommandError> {
    let r = Reader::open(root).map_err(store_error)?;
    let local_tree = r.folders(account).map_err(store_error)?;
    let remote_tree = if first
        .structural
        .iter()
        .any(|d| d.kind != DriftKind::Counter)
    {
        Some(remote_folders(api).await?)
    } else {
        None
    };
    let mut out = Pass {
        folders: first.folders,
        server_messages: first.server_messages,
        local_messages: first.local_messages,
        ..Pass::default()
    };
    if let Some(t) = &remote_tree {
        out.structural = compare_folders(&local_tree, t);
    }
    let wrong = r.derived_drift().map_err(store_error)?;
    if wrong > 0 {
        out.structural.push(counter_drift(wrong));
    }
    let folders = LocalFolders {
        by_id: local_tree.iter().map(|f| (f.id, f.clone())).collect(),
    };
    let remote_names: HashMap<String, String> = remote_tree
        .iter()
        .flatten()
        .map(|f| (f.id.clone(), f.name.clone()))
        .chain(
            local_tree
                .iter()
                .map(|f| (f.remote_id.clone(), f.name.clone())),
        )
        .collect();
    let remote_name = |id: &str| {
        remote_names
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    };
    for (n, (id, found)) in first.messages.into_iter().enumerate() {
        if n >= RECHECK_LIMIT {
            out.messages.insert(id, found);
            continue;
        }
        let remote = api
            .message(&id, Priority::User)
            .await
            .map_err(command_error)?
            .map(|m| RemoteMessage::from_graph(&m, ""));
        let local = r.message_by_remote(account, &id).map_err(store_error)?;
        let d = compare_message(
            rules,
            &folders,
            remote.as_ref(),
            &remote_name,
            local.as_ref(),
        );
        if !d.is_empty() {
            out.messages.insert(id, d);
        }
    }
    Ok(out)
}

/// Asks the engine for a round and waits until it has gone idle after it.
pub async fn settle(shared: &AccountShared, timeout: Duration) -> Result<(), CommandError> {
    let ticket = shared.sync_now();
    let start = Instant::now();
    while !shared.served(ticket) {
        if start.elapsed() > timeout {
            let live = shared.snapshot();
            return Err(CommandError::new(
                ErrorKind::Server,
                format!(
                    "the account did not finish a sync round within {} s (phase {}{})",
                    timeout.as_secs(),
                    live.phase.as_str(),
                    live.detail.map(|d| format!(": {d}")).unwrap_or_default()
                ),
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

/// Options for one account's check.
pub struct CheckRun<'a> {
    /// The account's key.
    pub key: &'a str,
    /// Its store ID.
    pub account: AccountId,
    /// Graph reads.
    pub api: &'a MailApi,
    /// Its engine's shared state, to ask for rounds.
    pub shared: &'a AccountShared,
    /// The store directory.
    pub root: &'a std::path::Path,
    /// What to expect.
    pub rules: Rules,
    /// Run sync rounds before each pass.
    pub settle: bool,
    /// Drift entries to list.
    pub limit: usize,
}

/// Checks one account.
pub async fn check_account(run: CheckRun<'_>) -> Result<AccountCheck, CommandError> {
    let t = Instant::now();
    let first_sync_done = Reader::open(run.root)
        .and_then(|r| r.sync_state(run.account, FIRST_SYNC_RESOURCE))
        .map_err(store_error)?
        .is_some_and(|s| s.synced_at.is_some());
    // Before the first sync is done a round can take hours; don't wait for it.
    let settle = run.settle && first_sync_done;
    if settle {
        settle_or_fail(run.shared).await?;
    }
    let first = full_pass(run.api, run.root, run.account, &run.rules).await?;
    let found = first.total();
    let pass = if found > 0 {
        if settle {
            settle_or_fail(run.shared).await?;
        }
        recheck(run.api, run.root, run.account, &run.rules, first).await?
    } else {
        first
    };
    let mut all: Vec<Drift> = pass.structural;
    all.extend(pass.messages.into_values().flatten());
    let mut by_kind: BTreeMap<DriftKind, usize> = BTreeMap::new();
    for d in &all {
        *by_kind.entry(d.kind).or_default() += 1;
    }
    let drift_total = all.len();
    all.truncate(run.limit);
    Ok(AccountCheck {
        account: run.key.to_string(),
        first_sync_done,
        settled: settle,
        folders: pass.folders,
        server_messages: pass.server_messages,
        local_messages: pass.local_messages,
        settled_on_recheck: found.saturating_sub(drift_total),
        drift_total,
        by_kind: by_kind
            .into_iter()
            .map(|(kind, count)| DriftCount { kind, count })
            .collect(),
        drift: all,
        seconds: t.elapsed().as_secs_f64(),
    })
}

async fn settle_or_fail(shared: &AccountShared) -> Result<(), CommandError> {
    settle(shared, SETTLE_TIMEOUT).await
}
