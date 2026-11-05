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
    AccountId, EventId, Folder, FolderId, LOCAL_REMOTE_PREFIX, MessageId, NewAudit, NewFolder,
    NewOp, Op, OpId, OpState, Reader, WriteTx, event_target, folder_target, message_target,
    series_target,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::compose::{self, AttachmentData, ComposeRequest, DraftOp, DraftUpdateOp, Recipient};
use crate::error::{CommandError, ErrorKind};
use crate::events::{
    self, EventChanges, EventDeleteOp, EventFields, EventOp, EventRespondOp, EventUpdateOp, Rsvp,
};

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
    /// Create a draft (new, reply, reply all or forward) with its attachments.
    CreateDraft(DraftOp),
    /// Change a draft.
    UpdateDraft(DraftUpdateOp),
    /// Send a draft. Queued with the undo-send delay.
    SendDraft {
        /// Local message ID of the draft.
        message: i64,
        /// The draft's Internet message ID, read before sending, to tell a
        /// lost answer from a refusal on a retry.
        #[serde(default)]
        internet_message_id: Option<String>,
    },
    /// Create an event (or a recurring series).
    CreateEvent(EventOp),
    /// Change an event or a series.
    UpdateEvent(EventUpdateOp),
    /// Answer an invitation.
    RespondEvent(EventRespondOp),
    /// Delete an event or a series; for a meeting you organised, cancel it.
    DeleteEvent(EventDeleteOp),
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
            OpPayload::CreateDraft(_) => "create_draft",
            OpPayload::UpdateDraft(_) => "update_draft",
            OpPayload::SendDraft { .. } => "send_draft",
            OpPayload::CreateEvent(_) => "create_event",
            OpPayload::UpdateEvent(_) => "update_event",
            OpPayload::RespondEvent(_) => "respond_event",
            OpPayload::DeleteEvent(_) => "delete_event",
        }
    }

    /// The queue target: ops on one item run in order, and sync leaves the
    /// item alone while one is open.
    pub fn target(&self) -> String {
        match self {
            OpPayload::SetRead { message, .. }
            | OpPayload::SetFlag { message, .. }
            | OpPayload::MoveMessage { message, .. }
            | OpPayload::DeleteMessage { message }
            | OpPayload::SendDraft { message, .. } => message_target(MessageId(*message)),
            OpPayload::CreateDraft(d) => message_target(MessageId(d.message)),
            OpPayload::UpdateDraft(d) => message_target(MessageId(d.message)),
            OpPayload::CreateFolder { folder, .. }
            | OpPayload::RenameFolder { folder, .. }
            | OpPayload::MoveFolder { folder, .. }
            | OpPayload::DeleteFolder { folder, .. } => folder_target(FolderId(*folder)),
            OpPayload::CreateEvent(e) => event_target(EventId(e.event)),
            OpPayload::UpdateEvent(e) => event_or_series(e.event, e.series.as_deref()),
            OpPayload::RespondEvent(e) => event_or_series(e.event, e.series.as_deref()),
            OpPayload::DeleteEvent(e) => event_or_series(e.event, e.series.as_deref()),
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

fn event_or_series(event: i64, series: Option<&str>) -> String {
    match series {
        Some(m) => series_target(m),
        None => event_target(EventId(event)),
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
    /// Save a new draft (or a reply or forward).
    Compose {
        /// What to write.
        draft: ComposeRequest,
        /// Also send it, after `send_delay_secs`.
        send: bool,
        /// Undo-send delay in seconds.
        send_delay_secs: u32,
    },
    /// Change a draft.
    DraftUpdate {
        /// Local message ID of the draft.
        message: i64,
        /// New subject.
        subject: Option<String>,
        /// New whole body.
        html: Option<String>,
        /// New To (replaces).
        to: Option<Vec<Recipient>>,
        /// New Cc (replaces).
        cc: Option<Vec<Recipient>>,
        /// New Bcc (replaces).
        bcc: Option<Vec<Recipient>>,
        /// Files to add.
        attachments: Vec<AttachmentData>,
        /// Attachments to take off, by name or Graph attachment ID.
        remove_attachments: Vec<String>,
    },
    /// Send a draft after `delay_secs`.
    Send {
        /// Local message ID of the draft.
        message: i64,
        /// Undo-send delay in seconds.
        delay_secs: u32,
    },
    /// Add an event.
    EventCreate {
        /// Account key or address; found from the calendar when left out.
        account: Option<String>,
        /// Calendar name; the account's default calendar when left out.
        calendar: Option<String>,
        /// The event.
        event: EventFields,
        /// The email it comes from, linked in its body.
        from_mail: Option<i64>,
    },
    /// Change an event.
    EventUpdate {
        /// Local event ID.
        event: i64,
        /// Change the whole series it belongs to.
        series: bool,
        /// What changes.
        changes: EventChanges,
    },
    /// Delete an event (cancelling a meeting you organised).
    EventDelete {
        /// Local event ID.
        event: i64,
        /// Delete the whole series.
        series: bool,
        /// A note for the attendees' cancellation.
        comment: Option<String>,
    },
    /// Answer an invitation.
    EventRespond {
        /// Local event ID.
        event: i64,
        /// Answer for the whole series.
        series: bool,
        /// The answer.
        response: Rsvp,
        /// A note to the organiser.
        comment: Option<String>,
        /// Tell the organiser.
        send_response: bool,
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
            WriteRequest::Compose { send: true, .. } => "mail.send",
            WriteRequest::Compose { draft, .. } => match draft.response {
                None => "draft.create",
                Some(crate::compose::ResponseKind::Forward) => "draft.forward",
                Some(_) => "draft.reply",
            },
            WriteRequest::DraftUpdate { .. } => "draft.update",
            WriteRequest::Send { .. } => "draft.send",
            WriteRequest::EventCreate { .. } => "cal.add",
            WriteRequest::EventUpdate { .. } => "cal.edit",
            WriteRequest::EventDelete { .. } => "cal.delete",
            WriteRequest::EventRespond { .. } => "cal.rsvp",
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
    /// Local ID of the message, folder or event the op created, if any.
    #[serde(default)]
    pub item: Option<i64>,
    /// When it is due to go (RFC 3339), if held back by the undo-send delay.
    #[serde(default)]
    pub not_before: Option<String>,
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
    /// Hold it back this many seconds (undo send).
    pub delay_secs: u32,
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
                    delay_secs: 0,
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
                    delay_secs: 0,
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
                        delay_secs: 0,
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
                delay_secs: 0,
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
                delay_secs: 0,
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
                delay_secs: 0,
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
                delay_secs: 0,
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
        WriteRequest::Compose {
            draft,
            send,
            send_delay_secs,
        } => {
            let a = match draft.respond_to {
                Some(id) => {
                    let m = tx
                        .message(MessageId(id))
                        .map_err(store_err)?
                        .ok_or_else(|| {
                            CommandError::new(ErrorKind::NotFound, format!("no message {id}"))
                        })?;
                    accounts
                        .iter()
                        .find(|a| a.id == m.account)
                        .cloned()
                        .ok_or_else(|| CommandError::internal("message without an account"))?
                }
                None => pick_account(tx, draft.account.as_deref())?,
            };
            let op = compose::plan_draft(tx, draft, &a)?;
            if *send && op.to.is_empty() && op.cc.is_empty() && op.bcc.is_empty() {
                return Err(CommandError::usage(
                    "give at least one recipient (--to, --cc or --bcc)",
                ));
            }
            let verb = match draft.response {
                None => "draft",
                Some(compose::ResponseKind::Forward) => "forward draft",
                Some(_) => "reply draft",
            };
            out.push(Planned {
                delay_secs: 0,
                account: a.id,
                account_key: a.key.clone(),
                summary: format!("save {verb} {:?}", short(&op.subject)),
                payload: OpPayload::CreateDraft(op.clone()),
            });
            if *send {
                out.push(Planned {
                    delay_secs: *send_delay_secs,
                    account: a.id,
                    account_key: a.key.clone(),
                    summary: format!(
                        "send {:?} to {}",
                        short(&op.subject),
                        who(&op.to, &op.cc, &op.bcc)
                    ),
                    payload: OpPayload::SendDraft {
                        message: 0,
                        internet_message_id: None,
                    },
                });
            }
        }
        WriteRequest::DraftUpdate {
            message,
            subject,
            html,
            to,
            cc,
            bcc,
            attachments,
            remove_attachments,
        } => {
            let m = need_draft(tx, *message)?;
            let prev = compose::snapshot(tx, &m).map_err(store_err)?;
            out.push(Planned {
                delay_secs: 0,
                account: m.account,
                account_key: key_of(m.account),
                summary: format!(
                    "update draft {:?}",
                    short(subject.as_deref().unwrap_or(&m.subject))
                ),
                payload: OpPayload::UpdateDraft(DraftUpdateOp {
                    message: *message,
                    subject: subject.clone(),
                    html: html
                        .as_ref()
                        .map(|h| format!("<html><body>{h}</body></html>")),
                    to: to.clone(),
                    cc: cc.clone(),
                    bcc: bcc.clone(),
                    attachments: attachments.clone(),
                    attachments_done: 0,
                    remove_attachments: remove_attachments.clone(),
                    pruned: false,
                    prev,
                }),
            });
        }
        WriteRequest::Send {
            message,
            delay_secs,
        } => {
            let m = need_draft(tx, *message)?;
            if m.to.is_empty() && m.cc.is_empty() {
                // Bcc isn't stored locally; the server refuses a draft with no
                // recipients at all.
            }
            let pending_send = tx.open_ops().map_err(store_err)?.into_iter().any(|o| {
                o.kind == "send_draft"
                    && o.target == message_target(m.id)
                    && o.state != OpState::Failed
            });
            if pending_send {
                return Err(CommandError::new(
                    ErrorKind::Conflict,
                    format!("draft {message} is already being sent (fam ops list)"),
                ));
            }
            let to: Vec<Recipient> =
                m.to.iter()
                    .map(|a| Recipient {
                        name: a.name.clone(),
                        address: a.addr.clone(),
                    })
                    .collect();
            out.push(Planned {
                delay_secs: *delay_secs,
                account: m.account,
                account_key: key_of(m.account),
                summary: format!("send {:?} to {}", short(&m.subject), who(&to, &[], &[])),
                payload: OpPayload::SendDraft {
                    message: *message,
                    internet_message_id: None,
                },
            });
        }
        WriteRequest::EventCreate {
            account,
            calendar,
            event,
            from_mail,
        } => out.push(plan_event_create(
            tx,
            account.as_deref(),
            calendar.as_deref(),
            event,
            *from_mail,
        )?),
        WriteRequest::EventUpdate {
            event,
            series,
            changes,
        } => {
            let (e, master) = need_event(tx, *event, *series)?;
            if changes.is_empty() {
                return Err(CommandError::usage("nothing to change"));
            }
            let organiser_only = changes.subject.is_some()
                || changes.retimes()
                || changes.location.is_some()
                || changes.attendees.is_some();
            if organiser_only && !e.data.is_organizer {
                return Err(CommandError::usage(format!(
                    "{:?} was organised by {}; only they can change its subject, times, place or attendees",
                    e.data.subject, e.data.organizer.addr
                )));
            }
            if let Some(z) = &changes.time_zone {
                if crate::tz::parse_zone(z).is_none() {
                    return Err(CommandError::usage(format!("unknown time zone {z:?}")));
                }
            }
            if changes.retimes() {
                let all_day = changes.all_day.unwrap_or(e.data.is_all_day);
                let start = changes.start_at.unwrap_or(e.data.start_at);
                let end = changes.end_at.unwrap_or(e.data.end_at);
                events::check_times(start, end, all_day)?;
            }
            let rows = events::rows(tx, e.id.0, master.as_deref()).map_err(store_err)?;
            let mut what = Vec::new();
            for (on, w) in [
                (changes.subject.is_some(), "subject"),
                (changes.retimes(), "time"),
                (changes.location.is_some(), "place"),
                (changes.body_html.is_some(), "notes"),
                (changes.attendees.is_some(), "attendees"),
                (changes.show_as.is_some(), "free/busy"),
                (
                    changes.reminder_minutes.is_some() || changes.no_reminder,
                    "reminder",
                ),
                (changes.categories.is_some(), "categories"),
            ] {
                if on {
                    what.push(w);
                }
            }
            out.push(Planned {
                delay_secs: 0,
                account: e.data.account,
                account_key: key_of(e.data.account),
                summary: format!(
                    "change {}{:?} on {}: {}",
                    if master.is_some() { "every " } else { "" },
                    short(&e.data.subject),
                    event_when(&e.data),
                    what.join(", ")
                ),
                payload: OpPayload::UpdateEvent(EventUpdateOp {
                    event: e.id.0,
                    series: master,
                    changes: changes.clone(),
                    prev: events::snapshot(&rows),
                }),
            });
        }
        WriteRequest::EventDelete {
            event,
            series,
            comment,
        } => {
            let (e, master) = need_event(tx, *event, *series)?;
            let meeting = e.data.is_organizer && !e.data.attendees.is_empty();
            out.push(Planned {
                delay_secs: 0,
                account: e.data.account,
                account_key: key_of(e.data.account),
                summary: format!(
                    "{} {}{:?} on {}{}",
                    if meeting { "cancel" } else { "delete" },
                    if master.is_some() { "every " } else { "" },
                    short(&e.data.subject),
                    event_when(&e.data),
                    if meeting {
                        format!(" and tell {} attendees", e.data.attendees.len())
                    } else {
                        String::new()
                    }
                ),
                payload: OpPayload::DeleteEvent(EventDeleteOp {
                    event: e.id.0,
                    series: master,
                    comment: comment.clone().filter(|_| meeting),
                }),
            });
        }
        WriteRequest::EventRespond {
            event,
            series,
            response,
            comment,
            send_response,
        } => {
            let (e, master) = need_event(tx, *event, *series)?;
            if e.data.is_organizer {
                return Err(CommandError::usage(format!(
                    "you organised {:?}; there is no invitation to answer",
                    e.data.subject
                )));
            }
            let rows = events::rows(tx, e.id.0, master.as_deref()).map_err(store_err)?;
            out.push(Planned {
                delay_secs: 0,
                account: e.data.account,
                account_key: key_of(e.data.account),
                summary: format!(
                    "{} {}{:?} on {}{}",
                    response.verb(),
                    if master.is_some() { "every " } else { "" },
                    short(&e.data.subject),
                    event_when(&e.data),
                    if *send_response {
                        format!(" and tell {}", e.data.organizer.addr)
                    } else {
                        " without telling the organiser".into()
                    }
                ),
                payload: OpPayload::RespondEvent(EventRespondOp {
                    event: e.id.0,
                    series: master,
                    response: *response,
                    comment: comment.clone(),
                    send_response: *send_response,
                    prev: events::snapshot(&rows),
                }),
            });
        }
    }
    let _ = &accounts;
    Ok(out)
}

fn event_when(d: &familiar_store::EventData) -> String {
    events::when_text(
        d.start_at,
        d.end_at,
        d.is_all_day,
        d.start_tz.as_deref().unwrap_or("UTC"),
    )
}

/// An event, and its series master's server ID when `series`.
fn need_event(
    tx: &WriteTx<'_>,
    id: i64,
    series: bool,
) -> Result<(familiar_store::Event, Option<String>), CommandError> {
    let e = tx
        .event(EventId(id))
        .map_err(store_err)?
        .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("no event {id}")))?;
    let master = if series {
        Some(e.data.series_master_id.clone().ok_or_else(|| {
            CommandError::usage(format!(
                "event {id} ({:?}) isn't part of a recurring series",
                e.data.subject
            ))
        })?)
    } else {
        None
    };
    Ok((e, master))
}

fn plan_event_create(
    tx: &WriteTx<'_>,
    account: Option<&str>,
    calendar: Option<&str>,
    f: &EventFields,
    from_mail: Option<i64>,
) -> Result<Planned, CommandError> {
    let mut found = Vec::new();
    let accounts = match (account, calendar) {
        (None, Some(_)) => tx.accounts().map_err(store_err)?,
        _ => vec![pick_account(tx, account)?],
    };
    for a in &accounts {
        for c in tx.calendars(a.id).map_err(store_err)? {
            let ok = match calendar {
                Some(n) => c.name.eq_ignore_ascii_case(n.trim()),
                None => c.is_default,
            };
            if ok {
                found.push((a.clone(), c));
            }
        }
    }
    let (acc, cal) = match found.len() {
        1 => found.remove(0),
        0 => {
            let what = match calendar {
                Some(n) => format!("no calendar called {n:?}"),
                None => "no default calendar has synced yet".into(),
            };
            return Err(CommandError::new(ErrorKind::NotFound, what));
        }
        _ => {
            let mut keys: Vec<String> = found.iter().map(|(a, _)| a.key.clone()).collect();
            keys.sort();
            return Err(CommandError::usage(format!(
                "more than one account has that calendar; pick one with --account ({})",
                keys.join(", ")
            )));
        }
    };
    if !cal.can_edit {
        return Err(CommandError::new(
            ErrorKind::PermissionDenied,
            format!("{:?} is read-only for {}", cal.name, acc.key),
        ));
    }
    if crate::tz::parse_zone(&f.time_zone).is_none() {
        return Err(CommandError::usage(format!(
            "unknown time zone {:?}",
            f.time_zone
        )));
    }
    events::check_times(f.start_at, f.end_at, f.all_day)?;
    if let Some(r) = &f.recurrence {
        if r.interval == 0 {
            return Err(CommandError::usage("--every must be 1 or more"));
        }
        if r.until.is_some() && r.count.is_some() {
            return Err(CommandError::usage("give --until or --count, not both"));
        }
    }
    if let Some(m) = from_mail {
        tx.message(MessageId(m))
            .map_err(store_err)?
            .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("no message {m}")))?;
    }
    let invited: Vec<Recipient> = f.attendees.iter().chain(&f.optional).cloned().collect();
    let mut summary = format!(
        "add {:?} on {} to {}",
        short(&f.subject),
        events::when_text(f.start_at, f.end_at, f.all_day, &f.time_zone),
        cal.name
    );
    if !f.location.trim().is_empty() {
        summary.push_str(&format!(" at {:?}", short(&f.location)));
    }
    if let Some(r) = &f.recurrence {
        let every = match r.every {
            events::RepeatKind::Daily => "day",
            events::RepeatKind::Weekly => "week",
            events::RepeatKind::Monthly => "month",
            events::RepeatKind::Yearly => "year",
        };
        summary.push_str(&if r.interval > 1 {
            format!(", every {} {every}s", r.interval)
        } else {
            format!(", every {every}")
        });
        if let Some(n) = r.count {
            summary.push_str(&format!(", {n} times"));
        }
        if let Some(u) = &r.until {
            summary.push_str(&format!(" until {u}"));
        }
    }
    if !invited.is_empty() {
        summary.push_str(&format!(", inviting {}", who(&invited, &[], &[])));
    }
    let tid = placeholder_remote_id();
    Ok(Planned {
        delay_secs: 0,
        account: acc.id,
        account_key: acc.key.clone(),
        summary,
        payload: OpPayload::CreateEvent(EventOp {
            event: 0,
            calendar: cal.id.0,
            fields: f.clone(),
            transaction_id: tid
                .strip_prefix(LOCAL_REMOTE_PREFIX)
                .unwrap_or(&tid)
                .to_string(),
            remote_id: None,
            from_mail,
        }),
    })
}

fn who(to: &[Recipient], cc: &[Recipient], bcc: &[Recipient]) -> String {
    let all: Vec<&Recipient> = to.iter().chain(cc).chain(bcc).collect();
    match all.as_slice() {
        [] => "nobody".into(),
        [one] => one.address.clone(),
        [first, rest @ ..] => format!("{} and {} more", first.address, rest.len()),
    }
}

/// A message that is a draft (in its account's Drafts folder).
fn need_draft(tx: &WriteTx<'_>, id: i64) -> Result<familiar_store::Message, CommandError> {
    let m = tx
        .message(MessageId(id))
        .map_err(store_err)?
        .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("no message {id}")))?;
    let f = tx.folder(m.folder).map_err(store_err)?;
    if f.and_then(|f| f.well_known).as_deref() != Some("drafts") {
        return Err(CommandError::usage(format!(
            "message {id} isn't a draft (it isn't in Drafts)"
        )));
    }
    Ok(m)
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
        delay_secs: 0,
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

pub(crate) fn placeholder_remote_id() -> String {
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
) -> familiar_store::Result<Option<i64>> {
    apply_inner(tx, p, account)?;
    Ok(match p {
        OpPayload::CreateFolder { folder, .. } => Some(*folder),
        OpPayload::CreateDraft(d) => Some(d.message),
        OpPayload::CreateEvent(e) => Some(e.event),
        _ => None,
    })
}

fn apply_inner(
    tx: &WriteTx<'_>,
    p: &mut OpPayload,
    account: AccountId,
) -> familiar_store::Result<()> {
    match p {
        OpPayload::CreateDraft(d) => compose::apply_draft(tx, d, account).map(|_| ()),
        OpPayload::UpdateDraft(d) => compose::apply_update(tx, d),
        OpPayload::SendDraft { .. } => Ok(()),
        OpPayload::CreateEvent(e) => events::apply_create(tx, e),
        OpPayload::UpdateEvent(e) => events::apply_update(tx, e),
        OpPayload::RespondEvent(e) => events::apply_respond(tx, e),
        OpPayload::DeleteEvent(_) => Ok(()),
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
        OpPayload::CreateDraft(d) => compose::undo_draft(tx, d),
        OpPayload::UpdateDraft(d) => compose::undo_update(tx, d),
        OpPayload::SendDraft { .. } => Ok(()),
        OpPayload::CreateEvent(e) => events::undo_create(tx, e),
        OpPayload::UpdateEvent(e) => events::restore(tx, &e.prev),
        OpPayload::RespondEvent(e) => events::restore(tx, &e.prev),
        // Hidden while queued; shows again once the op is gone.
        OpPayload::DeleteEvent(_) => Ok(()),
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
    /// The server ID of a draft the op created.
    pub message_remote_id: Option<String>,
}

/// Records what the server did, once an op has landed.
pub fn finish_local(
    tx: &WriteTx<'_>,
    p: &OpPayload,
    account: AccountId,
    result: &OpResult,
) -> familiar_store::Result<()> {
    match p {
        OpPayload::CreateDraft(d) => match &result.message_remote_id {
            Some(r) => tx.set_message_remote_id(MessageId(d.message), r),
            None => Ok(()),
        },
        // The sent copy arrives in Sent Items with the next sync.
        OpPayload::SendDraft { message, .. } => match tx.delete_message(MessageId(*message)) {
            Err(familiar_store::Error::NotFound(_)) => Ok(()),
            r => r,
        },
        OpPayload::DeleteEvent(e) => events::finish_delete(tx, e.event, e.series.as_deref()),
        OpPayload::RespondEvent(e) if e.response == Rsvp::Decline => {
            events::finish_delete(tx, e.event, e.series.as_deref())
        }
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
///
/// A write from an agent source is checked against `grants` first and
/// refused whole, with nothing changed, if any op isn't allowed; see
/// [`crate::agent`]. Log the refusal with [`record_denied`] in a new
/// transaction, since this one is rolled back.
pub fn submit(
    tx: &WriteTx<'_>,
    params: &SubmitParams,
    grants: &crate::agent::Grants,
) -> Result<WriteOutput, CommandError> {
    let planned = plan(tx, &params.request)?;
    if crate::agent::is_agent(&params.source) {
        if let Some(d) = crate::agent::check(tx, grants, &planned) {
            return Err(d.error());
        }
    }
    let mut ops = Vec::with_capacity(planned.len());
    let mut last_item: Option<i64> = None;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    for mut p in planned {
        let not_before = if p.delay_secs > 0 {
            now + i64::from(p.delay_secs)
        } else {
            0
        };
        let due = (not_before > 0).then(|| crate::commands::auth::rfc3339(not_before as u64));
        if params.dry_run {
            ops.push(QueuedOp {
                id: None,
                account: p.account_key,
                kind: p.payload.kind().into(),
                summary: p.summary,
                state: "planned".into(),
                error: None,
                item: None,
                not_before: due,
            });
            continue;
        }
        // A send planned with its draft refers to the draft made just before.
        if let OpPayload::SendDraft { message, .. } = &mut p.payload {
            if *message == 0 {
                *message =
                    last_item.ok_or_else(|| CommandError::internal("send without a draft"))?;
            }
        }
        let item = apply_local(tx, &mut p.payload, p.account).map_err(store_err)?;
        if item.is_some() {
            last_item = item;
        }
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
                not_before,
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
            item,
            not_before: due,
        });
    }
    Ok(WriteOutput {
        dry_run: params.dry_run,
        waited: false,
        ops,
    })
}

/// Writes a `denied` audit row for a write [`submit`] refused for want of a
/// grant. Does nothing for a dry run or a write that is allowed.
pub fn record_denied(
    tx: &WriteTx<'_>,
    params: &SubmitParams,
    grants: &crate::agent::Grants,
) -> Result<(), CommandError> {
    if params.dry_run || !crate::agent::is_agent(&params.source) {
        return Ok(());
    }
    let planned = plan(tx, &params.request)?;
    if let Some(d) = crate::agent::check(tx, grants, &planned) {
        tx.add_audit(&NewAudit {
            account: Some(d.account),
            source: params.source.clone(),
            command: params.request.command().into(),
            outcome: "denied".into(),
            op: None,
            detail: format!(
                "needs {}: {}",
                crate::agent::access_name(d.access),
                d.summary
            ),
        })
        .map_err(store_err)?;
    }
    Ok(())
}

/// Submits `params` through `w`, logging a refusal. For callers holding
/// the writer themselves (`fam` without famd).
pub fn submit_with<W: familiar_store::StoreWriter>(
    w: &mut W,
    params: &SubmitParams,
    grants: &crate::agent::Grants,
) -> Result<WriteOutput, CommandError> {
    let r = with_tx(w, |tx| submit(tx, params, grants));
    if matches!(&r, Err(e) if e.kind == ErrorKind::PermissionDenied) {
        with_tx(w, |tx| record_denied(tx, params, grants))?;
    }
    r
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
