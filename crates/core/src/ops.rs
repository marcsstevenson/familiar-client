// SPDX-License-Identifier: MIT OR Apache-2.0
//! Local-first writes: what a write request turns into, and how each op
//! changes the store before, after and instead of reaching the server.
//!
//! A write is planned and applied in one store transaction: the store
//! changes at once (the message shows as read, the folder appears) and an
//! op row is queued with what the server must be told. `famd` sends queued
//! ops one at a time per account, in order, and retries until the server
//! has them, across restarts. When the server refuses an op for good, its
//! local change is undone.
//!
//! The same code runs in `famd` and, when `famd` isn't running, in `fam`
//! itself with the store's writer lock; `famd` sends the ops when it starts.
//!
//! Payloads name messages and folders by local ID. Server IDs are looked up
//! when the op runs, so an op can act on a folder an earlier op created.

use std::sync::atomic::{AtomicU64, Ordering};

use familiar_store::{
    AccountId, Folder, FolderId, LOCAL_REMOTE_PREFIX, MessageId, NewAudit, NewFolder, NewOp, Op,
    OpId, OpState, Reader, WriteTx, folder_target, message_target,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CommandError, ErrorKind};

/// What one queued op does, with what it needs to undo itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpPayload {
    /// Mark a message read or unread.
    SetRead {
        /// Local message ID.
        message: i64,
        /// New state.
        read: bool,
        /// State before.
        prev: bool,
    },
    /// Flag a message or clear its flag.
    SetFlag {
        /// Local message ID.
        message: i64,
        /// New state.
        flagged: bool,
        /// State before.
        prev: bool,
    },
    /// Move a message (also archive, and delete to Deleted Items).
    MoveMessage {
        /// Local message ID.
        message: i64,
        /// Local folder ID it goes to.
        to: i64,
        /// Local folder ID it came from.
        from: i64,
    },
    /// Delete a message for good. Applied locally once the server has it.
    DeleteMessage {
        /// Local message ID.
        message: i64,
    },
    /// Create a folder. The local folder exists at once with a placeholder
    /// server ID.
    CreateFolder {
        /// Local ID of the new folder.
        folder: i64,
        /// Local parent folder ID; `None` for the top level.
        parent: Option<i64>,
        /// Name.
        name: String,
    },
    /// Rename a folder.
    RenameFolder {
        /// Local folder ID.
        folder: i64,
        /// New name.
        name: String,
        /// Name before.
        prev: String,
    },
    /// Move a folder under another, or to the top level.
    MoveFolder {
        /// Local folder ID.
        folder: i64,
        /// New parent; `None` for the top level.
        to: Option<i64>,
        /// Parent before.
        from: Option<i64>,
    },
    /// Delete a folder: into Deleted Items, or for good when `permanent`
    /// (it is already there).
    DeleteFolder {
        /// Local folder ID.
        folder: i64,
        /// Parent before.
        from: Option<i64>,
        /// Local ID of Deleted Items, where it goes unless `permanent`.
        deleted_items: Option<i64>,
        /// Delete for good.
        permanent: bool,
    },
}

impl OpPayload {
    /// The op kind stored in the queue, e.g. `set_read`.
    pub fn kind(&self) -> &'static str {
        match self {
            OpPayload::SetRead { .. } => "set_read",
            OpPayload::SetFlag { .. } => "set_flag",
            OpPayload::MoveMessage { .. } => "move_message",
            OpPayload::DeleteMessage { .. } => "delete_message",
            OpPayload::CreateFolder { .. } => "create_folder",
            OpPayload::RenameFolder { .. } => "rename_folder",
            OpPayload::MoveFolder { .. } => "move_folder",
            OpPayload::DeleteFolder { .. } => "delete_folder",
        }
    }

    /// The queue target: ops on one item run in order, and sync leaves the
    /// item alone while one is open.
    pub fn target(&self) -> String {
        match self {
            OpPayload::SetRead { message, .. }
            | OpPayload::SetFlag { message, .. }
            | OpPayload::MoveMessage { message, .. }
            | OpPayload::DeleteMessage { message } => message_target(MessageId(*message)),
            OpPayload::CreateFolder { folder, .. }
            | OpPayload::RenameFolder { folder, .. }
            | OpPayload::MoveFolder { folder, .. }
            | OpPayload::DeleteFolder { folder, .. } => folder_target(FolderId(*folder)),
        }
    }

    /// Whether the op needs local folder `f` to exist on the server.
    pub fn uses_folder(&self, f: i64) -> bool {
        match self {
            OpPayload::MoveMessage { to, .. } => *to == f,
            OpPayload::CreateFolder { parent, .. } => *parent == Some(f),
            OpPayload::RenameFolder { folder, .. } | OpPayload::DeleteFolder { folder, .. } => {
                *folder == f
            }
            OpPayload::MoveFolder { folder, to, .. } => *folder == f || *to == Some(f),
            _ => false,
        }
    }

    /// Reads a queued op's payload.
    pub fn of(op: &Op) -> Result<Self, CommandError> {
        serde_json::from_str(&op.payload).map_err(|e| {
            CommandError::internal(format!("op {} has an unreadable payload: {e}", op.id.0))
        })
    }
}

// ---------------------------------------------------------------------------
// Requests

/// A write as a command asks for it, before it is split into ops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WriteRequest {
    /// Mark messages read (or unread).
    MarkRead {
        /// Local message IDs.
        messages: Vec<i64>,
        /// `false` marks them unread.
        read: bool,
    },
    /// Flag messages (or clear their flags).
    Flag {
        /// Local message IDs.
        messages: Vec<i64>,
        /// `false` clears the flag.
        flagged: bool,
    },
    /// Move messages to a folder (name, path or well-known name), looked up
    /// in each message's own account.
    Move {
        /// Local message IDs.
        messages: Vec<i64>,
        /// The destination folder.
        to: String,
    },
    /// Move messages to each account's Archive folder.
    Archive {
        /// Local message IDs.
        messages: Vec<i64>,
    },
    /// Move messages to Deleted Items, or delete them for good with
    /// `permanent` (only for messages already in Deleted Items).
    Delete {
        /// Local message IDs.
        messages: Vec<i64>,
        /// Delete for good.
        permanent: bool,
    },
    /// Create a folder.
    FolderCreate {
        /// The account (config name or address); may be left out with one account.
        account: Option<String>,
        /// The new folder's name.
        name: String,
        /// Parent folder; `None` for the top level.
        parent: Option<String>,
    },
    /// Rename a folder.
    FolderRename {
        /// The account.
        account: Option<String>,
        /// The folder.
        folder: String,
        /// Its new name.
        name: String,
    },
    /// Move a folder under another, or to the top level.
    FolderMove {
        /// The account.
        account: Option<String>,
        /// The folder.
        folder: String,
        /// The new parent; `None` for the top level.
        to: Option<String>,
    },
    /// Delete a folder: into Deleted Items, or for good when it is already
    /// there (`permanent` must then be set).
    FolderDelete {
        /// The account.
        account: Option<String>,
        /// The folder.
        folder: String,
        /// Delete for good.
        permanent: bool,
    },
}

impl WriteRequest {
    /// The command it comes from, e.g. `mail.move`, for the audit log.
    pub fn command(&self) -> &'static str {
        match self {
            WriteRequest::MarkRead { .. } => "mail.read",
            WriteRequest::Flag { .. } => "mail.flag",
            WriteRequest::Move { .. } => "mail.move",
            WriteRequest::Archive { .. } => "mail.archive",
            WriteRequest::Delete { .. } => "mail.delete",
            WriteRequest::FolderCreate { .. } => "folder.create",
            WriteRequest::FolderRename { .. } => "folder.rename",
            WriteRequest::FolderMove { .. } => "folder.move",
            WriteRequest::FolderDelete { .. } => "folder.delete",
        }
    }
}

/// `ops.submit`: plan a write, and apply and queue it unless `dry_run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitParams {
    /// The write.
    pub request: WriteRequest,
    /// Who asks: `cli`, `gui`, `mcp` or an agent name.
    pub source: String,
    /// Only say what would happen.
    pub dry_run: bool,
}

/// One op of a submitted write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QueuedOp {
    /// Op ID, for `fam ops wait` and `fam ops list`; `None` on a dry run.
    pub id: Option<i64>,
    /// Account key.
    pub account: String,
    /// Op kind, e.g. `move_message`.
    pub kind: String,
    /// What it does, in words.
    pub summary: String,
    /// `planned` (dry run), `pending`, `in_flight`, `done` or `failed`.
    pub state: String,
    /// Why it failed, or the last retry's error.
    #[serde(default)]
    pub error: Option<String>,
}

/// What a write did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WriteOutput {
    /// True when nothing was changed (`--dry-run`).
    pub dry_run: bool,
    /// True when the command waited for the server (`--wait`).
    pub waited: bool,
    /// The ops, in queue order.
    pub ops: Vec<QueuedOp>,
}

impl WriteOutput {
    /// Lines for people.
    pub fn render(&self) -> String {
        if self.ops.is_empty() {
            return "Nothing to do.".into();
        }
        let mut out = String::new();
        for o in &self.ops {
            let state = match o.state.as_str() {
                "planned" => "would",
                "pending" | "in_flight" => "queued",
                "done" => "done",
                "failed" => "FAILED",
                s => s,
            };
            out.push_str(&format!("{state}: {} ({})", o.summary, o.account));
            if let Some(id) = o.id {
                out.push_str(&format!(" [op {id}]"));
            }
            if let Some(e) = &o.error {
                out.push_str(&format!(" - {e}"));
            }
            out.push('\n');
        }
        out.pop();
        out
    }
}

/// `ops.wait`: wait until the ops are done or failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitParams {
    /// Op IDs.
    pub ops: Vec<i64>,
    /// Give up after this many seconds.
    pub timeout_secs: u64,
}

/// One op as `fam ops list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpView {
    /// Op ID.
    pub id: i64,
    /// Account key.
    pub account: String,
    /// Kind.
    pub kind: String,
    /// What it does.
    pub summary: String,
    /// `pending`, `in_flight`, `done` or `failed`.
    pub state: String,
    /// Times tried.
    pub attempts: i64,
    /// Last error.
    pub error: Option<String>,
    /// Who asked.
    pub source: String,
    /// When it was queued (RFC 3339).
    pub queued_at: String,
    /// Earliest next try (RFC 3339), for a pending op waiting out a retry.
    pub retry_at: Option<String>,
}

/// The state's name.
pub fn state_name(s: OpState) -> &'static str {
    match s {
        OpState::Pending => "pending",
        OpState::InFlight => "in_flight",
        OpState::Done => "done",
        OpState::Failed => "failed",
    }
}

/// An op as `fam ops list` shows it.
pub fn view(op: &Op, account: &str) -> OpView {
    OpView {
        id: op.id.0,
        account: account.to_string(),
        kind: op.kind.clone(),
        summary: op.summary.clone(),
        state: state_name(op.state).into(),
        attempts: op.attempts,
        error: op.last_error.clone(),
        source: op.source.clone(),
        queued_at: crate::commands::auth::rfc3339(op.created_at.max(0) as u64),
        retry_at: (op.state == OpState::Pending && op.not_before > op.updated_at)
            .then(|| crate::commands::auth::rfc3339(op.not_before.max(0) as u64)),
    }
}

// ---------------------------------------------------------------------------
// Planning

/// One op a request turns into.
#[derive(Debug, Clone)]
pub struct Planned {
    /// Account.
    pub account: AccountId,
    /// Its key.
    pub account_key: String,
    /// The op.
    pub payload: OpPayload,
    /// In words.
    pub summary: String,
}

fn store_err(e: familiar_store::Error) -> CommandError {
    match e {
        familiar_store::Error::NotFound(m) => CommandError::new(ErrorKind::NotFound, m),
        e => CommandError::internal(format!("store: {e}")),
    }
}

/// Finds a folder by path (`Inbox/Club`), well-known name or name.
pub fn find_folder<'a>(
    all: &'a [Folder],
    wanted: &str,
    account_key: &str,
) -> Result<Option<&'a Folder>, CommandError> {
    let paths = crate::commands::folder::folder_paths(all);
    let pick =
        |f: &dyn Fn(&Folder) -> bool| -> Vec<&'a Folder> { all.iter().filter(|x| f(x)).collect() };
    let mut matches = pick(&|f| {
        paths
            .get(&f.id)
            .is_some_and(|p| p.eq_ignore_ascii_case(wanted))
    });
    if matches.is_empty() {
        matches = pick(&|f| {
            f.well_known
                .as_deref()
                .is_some_and(|w| w.eq_ignore_ascii_case(wanted))
        });
    }
    if matches.is_empty() {
        matches = pick(&|f| f.name.eq_ignore_ascii_case(wanted));
    }
    if matches.len() > 1 {
        let mut options: Vec<&str> = matches
            .iter()
            .filter_map(|f| paths.get(&f.id).map(String::as_str))
            .collect();
        options.sort_unstable();
        return Err(CommandError::usage(format!(
            "{account_key} has more than one folder called {wanted:?}; give its path: {}",
            options.join(", ")
        )));
    }
    Ok(matches.first().copied())
}

fn need_folder<'a>(
    all: &'a [Folder],
    wanted: &str,
    account_key: &str,
) -> Result<&'a Folder, CommandError> {
    find_folder(all, wanted, account_key)?.ok_or_else(|| {
        CommandError::new(
            ErrorKind::NotFound,
            format!("{account_key} has no folder {wanted:?}"),
        )
    })
}

fn well_known<'a>(all: &'a [Folder], name: &str) -> Option<&'a Folder> {
    all.iter().find(|f| f.well_known.as_deref() == Some(name))
}

fn is_under(all: &[Folder], id: FolderId, ancestor: FolderId) -> bool {
    let mut at = Some(id);
    for _ in 0..64 {
        match at {
            Some(a) if a == ancestor => return true,
            Some(a) => at = all.iter().find(|f| f.id == a).and_then(|f| f.parent),
            None => return false,
        }
    }
    false
}

/// The account a folder request names (or the only one).
fn pick_account(
    tx: &WriteTx<'_>,
    only: Option<&str>,
) -> Result<familiar_store::Account, CommandError> {
    let all = tx.accounts().map_err(store_err)?;
    match only {
        Some(k) => all
            .into_iter()
            .find(|a| a.key.eq_ignore_ascii_case(k) || a.email.eq_ignore_ascii_case(k))
            .ok_or_else(|| {
                CommandError::new(
                    ErrorKind::NotFound,
                    format!("no account {k:?} on this device"),
                )
            }),
        None => {
            if all.len() == 1 {
                Ok(all.into_iter().next().expect("one"))
            } else if all.is_empty() {
                Err(CommandError::new(
                    ErrorKind::NotFound,
                    "no account has synced on this device",
                ))
            } else {
                let mut keys: Vec<String> = all.into_iter().map(|a| a.key).collect();
                keys.sort();
                Err(CommandError::usage(format!(
                    "more than one account; pick one with --account ({})",
                    keys.join(", ")
                )))
            }
        }
    }
}

fn short(s: &str) -> String {
    let s = if s.is_empty() { "(no subject)" } else { s };
    if s.chars().count() > 50 {
        let cut: String = s.chars().take(49).collect();
        format!("{cut}…")
    } else {
        s.to_string()
    }
}

/// Turns a request into ops, reading the store only.
pub fn plan(tx: &WriteTx<'_>, req: &WriteRequest) -> Result<Vec<Planned>, CommandError> {
    let accounts = tx.accounts().map_err(store_err)?;
    let key_of = |a: AccountId| {
        accounts
            .iter()
            .find(|x| x.id == a)
            .map(|x| x.key.clone())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    let messages = |ids: &[i64]| -> Result<Vec<familiar_store::Message>, CommandError> {
        if ids.is_empty() {
            return Err(CommandError::usage("give at least one message ID"));
        }
        let mut seen = std::collections::HashSet::new();
        let mut v = Vec::new();
        for id in ids {
            if !seen.insert(*id) {
                continue;
            }
            v.push(
                tx.message(MessageId(*id))
                    .map_err(store_err)?
                    .ok_or_else(|| {
                        CommandError::new(ErrorKind::NotFound, format!("no message {id}"))
                    })?,
            );
        }
        Ok(v)
    };
    match req {
        WriteRequest::MarkRead {
            messages: ids,
            read,
        } => {
            for m in messages(ids)? {
                if m.is_read == *read {
                    continue;
                }
                out.push(Planned {
                    account: m.account,
                    account_key: key_of(m.account),
                    summary: format!(
                        "mark {} {:?}",
                        if *read { "read" } else { "unread" },
                        short(&m.subject)
                    ),
                    payload: OpPayload::SetRead {
                        message: m.id.0,
                        read: *read,
                        prev: m.is_read,
                    },
                });
            }
        }
        WriteRequest::Flag {
            messages: ids,
            flagged,
        } => {
            for m in messages(ids)? {
                if m.is_flagged == *flagged {
                    continue;
                }
                out.push(Planned {
                    account: m.account,
                    account_key: key_of(m.account),
                    summary: format!(
                        "{} {:?}",
                        if *flagged { "flag" } else { "unflag" },
                        short(&m.subject)
                    ),
                    payload: OpPayload::SetFlag {
                        message: m.id.0,
                        flagged: *flagged,
                        prev: m.is_flagged,
                    },
                });
            }
        }
        WriteRequest::Move { messages: ids, to } => {
            for m in messages(ids)? {
                let key = key_of(m.account);
                let folders = tx.folders(m.account).map_err(store_err)?;
                let dest = need_folder(&folders, to, &key)?;
                push_move(&mut out, &m, dest, &key, "move");
            }
        }
        WriteRequest::Archive { messages: ids } => {
            for m in messages(ids)? {
                let key = key_of(m.account);
                let folders = tx.folders(m.account).map_err(store_err)?;
                let dest = well_known(&folders, "archive").ok_or_else(|| {
                    CommandError::new(ErrorKind::NotFound, format!("{key} has no Archive folder"))
                })?;
                push_move(&mut out, &m, dest, &key, "archive");
            }
        }
        WriteRequest::Delete {
            messages: ids,
            permanent,
        } => {
            for m in messages(ids)? {
                let key = key_of(m.account);
                let folders = tx.folders(m.account).map_err(store_err)?;
                let deleted = well_known(&folders, "deleteditems").ok_or_else(|| {
                    CommandError::new(
                        ErrorKind::NotFound,
                        format!("{key} has no Deleted Items folder"),
                    )
                })?;
                let in_deleted = is_under(&folders, m.folder, deleted.id);
                match (in_deleted, permanent) {
                    (false, false) => push_move(&mut out, &m, deleted, &key, "delete"),
                    (true, true) => out.push(Planned {
                        account: m.account,
                        account_key: key,
                        summary: format!("delete {:?} for good", short(&m.subject)),
                        payload: OpPayload::DeleteMessage { message: m.id.0 },
                    }),
                    (true, false) => {
                        return Err(CommandError::usage(format!(
                            "message {} is already in Deleted Items; add --permanent to delete it for good",
                            m.id.0
                        )));
                    }
                    (false, true) => {
                        return Err(CommandError::usage(format!(
                            "message {} isn't in Deleted Items; delete it without --permanent first",
                            m.id.0
                        )));
                    }
                }
            }
        }
        WriteRequest::FolderCreate {
            account,
            name,
            parent,
        } => {
            let a = pick_account(tx, account.as_deref())?;
            let name = name.trim();
            if name.is_empty() || name.contains('/') {
                return Err(CommandError::usage(
                    "a folder name can't be empty or contain '/'; use --parent for a subfolder",
                ));
            }
            let folders = tx.folders(a.id).map_err(store_err)?;
            let parent = parent
                .as_deref()
                .map(|p| need_folder(&folders, p, &a.key))
                .transpose()?;
            let clash = folders
                .iter()
                .any(|f| f.parent == parent.map(|p| p.id) && f.name.eq_ignore_ascii_case(name));
            if clash {
                return Err(CommandError::new(
                    ErrorKind::Conflict,
                    format!("{} already has a folder {name:?} there", a.key),
                ));
            }
            let where_ = parent.map_or("the top level".to_string(), |p| format!("{:?}", p.name));
            out.push(Planned {
                account: a.id,
                account_key: a.key.clone(),
                summary: format!("create folder {name:?} under {where_}"),
                payload: OpPayload::CreateFolder {
                    folder: 0,
                    parent: parent.map(|p| p.id.0),
                    name: name.to_string(),
                },
            });
        }
        WriteRequest::FolderRename {
            account,
            folder,
            name,
        } => {
            let a = pick_account(tx, account.as_deref())?;
            let folders = tx.folders(a.id).map_err(store_err)?;
            let f = need_folder(&folders, folder, &a.key)?;
            refuse_well_known(f, "renamed")?;
            let name = name.trim();
            if name.is_empty() || name.contains('/') {
                return Err(CommandError::usage(
                    "a folder name can't be empty or contain '/'",
                ));
            }
            if f.name == name {
                return Ok(out);
            }
            out.push(Planned {
                account: a.id,
                account_key: a.key.clone(),
                summary: format!("rename folder {:?} to {name:?}", f.name),
                payload: OpPayload::RenameFolder {
                    folder: f.id.0,
                    name: name.to_string(),
                    prev: f.name.clone(),
                },
            });
        }
        WriteRequest::FolderMove {
            account,
            folder,
            to,
        } => {
            let a = pick_account(tx, account.as_deref())?;
            let folders = tx.folders(a.id).map_err(store_err)?;
            let f = need_folder(&folders, folder, &a.key)?;
            refuse_well_known(f, "moved")?;
            let dest = to
                .as_deref()
                .map(|p| need_folder(&folders, p, &a.key))
                .transpose()?;
            if let Some(d) = dest {
                if is_under(&folders, d.id, f.id) {
                    return Err(CommandError::usage("a folder can't move into itself"));
                }
            }
            if f.parent == dest.map(|d| d.id) {
                return Ok(out);
            }
            let where_ = dest.map_or("the top level".to_string(), |p| format!("{:?}", p.name));
            out.push(Planned {
                account: a.id,
                account_key: a.key.clone(),
                summary: format!("move folder {:?} under {where_}", f.name),
                payload: OpPayload::MoveFolder {
                    folder: f.id.0,
                    to: dest.map(|d| d.id.0),
                    from: f.parent.map(|p| p.0),
                },
            });
        }
        WriteRequest::FolderDelete {
            account,
            folder,
            permanent,
        } => {
            let a = pick_account(tx, account.as_deref())?;
            let folders = tx.folders(a.id).map_err(store_err)?;
            let f = need_folder(&folders, folder, &a.key)?;
            refuse_well_known(f, "deleted")?;
            let deleted = well_known(&folders, "deleteditems");
            let in_deleted = deleted.is_some_and(|d| is_under(&folders, f.id, d.id));
            if in_deleted != *permanent {
                return Err(CommandError::usage(if in_deleted {
                    format!(
                        "folder {:?} is already in Deleted Items; add --permanent to delete it for good",
                        f.name
                    )
                } else {
                    format!(
                        "folder {:?} isn't in Deleted Items; delete it without --permanent first",
                        f.name
                    )
                }));
            }
            out.push(Planned {
                account: a.id,
                account_key: a.key.clone(),
                summary: if *permanent {
                    format!("delete folder {:?} for good", f.name)
                } else {
                    format!("delete folder {:?} (to Deleted Items)", f.name)
                },
                payload: OpPayload::DeleteFolder {
                    folder: f.id.0,
                    from: f.parent.map(|p| p.0),
                    deleted_items: deleted.map(|d| d.id.0),
                    permanent: *permanent,
                },
            });
        }
    }
    let _ = &accounts;
    Ok(out)
}

fn refuse_well_known(f: &Folder, what: &str) -> Result<(), CommandError> {
    if f.well_known.is_some() {
        return Err(CommandError::usage(format!(
            "{:?} is a built-in folder and can't be {what}",
            f.name
        )));
    }
    Ok(())
}

fn push_move(
    out: &mut Vec<Planned>,
    m: &familiar_store::Message,
    dest: &Folder,
    key: &str,
    verb: &str,
) {
    if m.folder == dest.id {
        return;
    }
    out.push(Planned {
        account: m.account,
        account_key: key.to_string(),
        summary: format!("{verb} {:?} to {}", short(&m.subject), dest.name),
        payload: OpPayload::MoveMessage {
            message: m.id.0,
            to: dest.id.0,
            from: m.folder.0,
        },
    });
}

// ---------------------------------------------------------------------------
// Applying

static PLACEHOLDER: AtomicU64 = AtomicU64::new(0);

fn placeholder_remote_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let n = PLACEHOLDER.fetch_add(1, Ordering::Relaxed);
    format!("{LOCAL_REMOTE_PREFIX}{nanos:x}-{n}")
}

/// Makes an op's change in the store, before the server has it.
pub fn apply_local(
    tx: &WriteTx<'_>,
    p: &mut OpPayload,
    account: AccountId,
) -> familiar_store::Result<()> {
    match p {
        OpPayload::SetRead { message, read, .. } => tx.set_read(MessageId(*message), *read),
        OpPayload::SetFlag {
            message, flagged, ..
        } => tx.set_flagged(MessageId(*message), *flagged),
        OpPayload::MoveMessage { message, to, .. } => {
            tx.move_message(MessageId(*message), FolderId(*to))
        }
        OpPayload::DeleteMessage { .. } => Ok(()),
        OpPayload::CreateFolder {
            folder,
            parent,
            name,
        } => {
            let id = tx.upsert_folder(&NewFolder {
                account,
                remote_id: placeholder_remote_id(),
                parent: parent.map(FolderId),
                name: name.clone(),
                well_known: None,
            })?;
            *folder = id.0;
            Ok(())
        }
        OpPayload::RenameFolder { folder, name, .. } => tx.rename_folder(FolderId(*folder), name),
        OpPayload::MoveFolder { folder, to, .. } => {
            tx.set_folder_parent(FolderId(*folder), to.map(FolderId))
        }
        OpPayload::DeleteFolder {
            folder,
            deleted_items,
            permanent,
            ..
        } => {
            if *permanent {
                Ok(())
            } else {
                tx.set_folder_parent(FolderId(*folder), deleted_items.map(FolderId))
            }
        }
    }
}

/// Undoes an op's local change (it was refused, or cancelled).
pub fn undo_local(tx: &WriteTx<'_>, p: &OpPayload) -> familiar_store::Result<()> {
    let gone = |r: familiar_store::Result<()>| match r {
        Err(familiar_store::Error::NotFound(_)) => Ok(()),
        r => r,
    };
    match p {
        OpPayload::SetRead { message, prev, .. } => gone(tx.set_read(MessageId(*message), *prev)),
        OpPayload::SetFlag { message, prev, .. } => {
            gone(tx.set_flagged(MessageId(*message), *prev))
        }
        OpPayload::MoveMessage { message, from, .. } => {
            if tx.folder(FolderId(*from))?.is_some() {
                gone(tx.move_message(MessageId(*message), FolderId(*from)))
            } else {
                Ok(())
            }
        }
        OpPayload::DeleteMessage { .. } => Ok(()),
        OpPayload::CreateFolder { folder, .. } => {
            // Only a folder the server never made; anything moved into it
            // has been moved back by its own (cancelled) op first.
            match tx.folder(FolderId(*folder))? {
                Some(f) if f.remote_id.starts_with(LOCAL_REMOTE_PREFIX) => tx.delete_folder(f.id),
                _ => Ok(()),
            }
        }
        OpPayload::RenameFolder { folder, prev, .. } => {
            gone(tx.rename_folder(FolderId(*folder), prev))
        }
        OpPayload::MoveFolder { folder, from, .. } => {
            gone(tx.set_folder_parent(FolderId(*folder), from.map(FolderId)))
        }
        OpPayload::DeleteFolder {
            folder,
            from,
            permanent,
            ..
        } => {
            if *permanent {
                Ok(())
            } else {
                gone(tx.set_folder_parent(FolderId(*folder), from.map(FolderId)))
            }
        }
    }
}

/// What the server answered, as far as the store needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpResult {
    /// The server ID of a folder the op created.
    pub folder_remote_id: Option<String>,
}

/// Records what the server did, once an op has landed.
pub fn finish_local(
    tx: &WriteTx<'_>,
    p: &OpPayload,
    account: AccountId,
    result: &OpResult,
) -> familiar_store::Result<()> {
    match p {
        OpPayload::DeleteMessage { message } => match tx.delete_message(MessageId(*message)) {
            Err(familiar_store::Error::NotFound(_)) => Ok(()),
            r => r,
        },
        OpPayload::DeleteFolder {
            folder,
            permanent: true,
            ..
        } => tx.delete_folder(FolderId(*folder)),
        OpPayload::CreateFolder { folder, .. } => {
            let Some(remote) = &result.folder_remote_id else {
                return Ok(());
            };
            // Sync may have seen the new folder first and stored a second
            // copy; the local one (which ops refer to) wins.
            if let Some(dup) = tx.folder_by_remote(account, remote)? {
                if dup.id.0 != *folder {
                    tx.delete_folder(dup.id)?;
                }
            }
            tx.set_folder_remote_id(FolderId(*folder), remote)
        }
        _ => Ok(()),
    }
}

/// Plans `params.request` and, unless it is a dry run, applies and queues
/// it, all in `tx`. Each queued write is also recorded in the audit log.
pub fn submit(tx: &WriteTx<'_>, params: &SubmitParams) -> Result<WriteOutput, CommandError> {
    let planned = plan(tx, &params.request)?;
    let mut ops = Vec::with_capacity(planned.len());
    for mut p in planned {
        if params.dry_run {
            ops.push(QueuedOp {
                id: None,
                account: p.account_key,
                kind: p.payload.kind().into(),
                summary: p.summary,
                state: "planned".into(),
                error: None,
            });
            continue;
        }
        apply_local(tx, &mut p.payload, p.account).map_err(store_err)?;
        let payload =
            serde_json::to_string(&p.payload).map_err(|e| CommandError::internal(e.to_string()))?;
        let id = tx
            .enqueue_op(&NewOp {
                account: p.account,
                kind: p.payload.kind().into(),
                target: p.payload.target(),
                payload,
                idempotency_key: None,
                source: params.source.clone(),
                summary: p.summary.clone(),
            })
            .map_err(store_err)?;
        tx.add_audit(&NewAudit {
            account: Some(p.account),
            source: if params.source.is_empty() {
                "cli".into()
            } else {
                params.source.clone()
            },
            command: params.request.command().into(),
            outcome: "queued".into(),
            op: Some(id),
            detail: p.summary.clone(),
        })
        .map_err(store_err)?;
        ops.push(QueuedOp {
            id: Some(id.0),
            account: p.account_key,
            kind: p.payload.kind().into(),
            summary: p.summary,
            state: "pending".into(),
            error: None,
        });
    }
    Ok(WriteOutput {
        dry_run: params.dry_run,
        waited: false,
        ops,
    })
}

/// Cancels a pending or failed op and undoes its local change. Returns
/// `false` if it is already on its way or done.
pub fn cancel(tx: &WriteTx<'_>, id: OpId) -> Result<bool, CommandError> {
    let Some(op) = tx.cancel_op(id).map_err(store_err)? else {
        return Ok(false);
    };
    if op.state == OpState::Pending {
        if let Ok(p) = OpPayload::of(&op) {
            undo_local(tx, &p).map_err(store_err)?;
        }
    }
    Ok(true)
}

/// Updates each op's state from the store.
pub fn refresh(r: &Reader, out: &mut WriteOutput) -> Result<(), CommandError> {
    for o in &mut out.ops {
        if let Some(id) = o.id {
            if let Some(op) = r.op(OpId(id)).map_err(store_err)? {
                o.state = state_name(op.state).into();
                o.error = op.last_error;
            } else {
                o.state = "cancelled".into();
            }
        }
    }
    Ok(())
}

/// The result value for `ops.submit` over the socket.
pub fn to_value(o: &WriteOutput) -> Value {
    serde_json::to_value(o).unwrap_or(Value::Null)
}

/// Queues a failed op again, making its local change again first.
pub fn retry(tx: &WriteTx<'_>, id: OpId) -> Result<bool, CommandError> {
    let Some(op) = tx.op(id).map_err(store_err)? else {
        return Err(CommandError::new(
            ErrorKind::NotFound,
            format!("no op {}", id.0),
        ));
    };
    if op.state != OpState::Failed {
        return Ok(false);
    }
    let mut p = OpPayload::of(&op)?;
    apply_local(tx, &mut p, op.account).map_err(store_err)?;
    let payload = serde_json::to_string(&p).map_err(|e| CommandError::internal(e.to_string()))?;
    tx.set_op_payload(id, &payload).map_err(store_err)?;
    tx.requeue_failed(id).map_err(store_err)
}

/// Runs `f` in one store transaction, rolled back if it returns an error.
pub fn with_tx<W: familiar_store::StoreWriter, T>(
    w: &mut W,
    f: impl FnOnce(&WriteTx<'_>) -> Result<T, CommandError>,
) -> Result<T, CommandError> {
    let mut out = None;
    let r = w.write(|tx| match f(tx) {
        Ok(v) => {
            out = Some(Ok(v));
            Ok(())
        }
        Err(e) => {
            out = Some(Err(e));
            Err(familiar_store::Error::Aborted)
        }
    });
    match (r, out) {
        (_, Some(o)) => o,
        (Err(e), None) => Err(store_err(e)),
        (Ok(()), None) => Err(CommandError::internal("write did not run")),
    }
}
