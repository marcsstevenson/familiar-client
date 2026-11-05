// SPDX-License-Identifier: MIT OR Apache-2.0
//! Row types.

use crate::BodyHash;
use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($(#[$doc:meta] $name:ident),* $(,)?) => {$(
        #[$doc]
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl rusqlite::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
                i64::column_result(v).map($name)
            }
        }
    )*};
}

id_type!(
    /// Local ID of an account.
    AccountId,
    /// Local ID of a folder.
    FolderId,
    /// Local ID of a conversation.
    ThreadId,
    /// Local ID of a message.
    MessageId,
    /// Local ID of a queued op.
    OpId,
    /// Local ID of a calendar.
    CalendarId,
    /// Local ID of an event.
    EventId,
);

/// An account to add or update, matched by `key`.
#[derive(Debug, Clone, Default)]
pub struct NewAccount {
    /// The account's key in `config.toml`.
    pub key: String,
    /// Primary email address.
    pub email: String,
    /// Name shown in the UI.
    pub display_name: String,
    /// Provider name; `graph` for now.
    pub provider: String,
    /// Directory tenant for work accounts.
    pub tenant_id: Option<String>,
}

/// A stored account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Account {
    /// Local ID.
    pub id: AccountId,
    /// The account's key in `config.toml`.
    pub key: String,
    /// Primary email address.
    pub email: String,
    /// Name shown in the UI.
    pub display_name: String,
    /// Provider name.
    pub provider: String,
    /// Directory tenant for work accounts.
    pub tenant_id: Option<String>,
    /// When it was added (Unix seconds).
    pub created_at: i64,
}

/// A folder to add or update, matched by account and `remote_id`.
#[derive(Debug, Clone, Default)]
pub struct NewFolder {
    /// Owning account.
    pub account: AccountId,
    /// Server ID.
    pub remote_id: String,
    /// Parent folder, if not top level.
    pub parent: Option<FolderId>,
    /// Display name.
    pub name: String,
    /// Well-known name (`inbox`, `sentitems`, ...), if any.
    pub well_known: Option<String>,
}

/// A stored folder with its counters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Folder {
    /// Local ID.
    pub id: FolderId,
    /// Owning account.
    pub account: AccountId,
    /// Server ID.
    pub remote_id: String,
    /// Parent folder.
    pub parent: Option<FolderId>,
    /// Display name.
    pub name: String,
    /// Well-known name, if any.
    pub well_known: Option<String>,
    /// Messages in the folder.
    pub total: i64,
    /// Unread messages in the folder.
    pub unread: i64,
}

/// A mailbox address.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    /// Display name, possibly empty.
    pub name: String,
    /// Email address.
    pub addr: String,
}

/// A stored body attached to a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BodyRef {
    /// Content hash, as returned by [`crate::WriteTx::put_body`].
    pub hash: BodyHash,
    /// True for HTML, false for plain text.
    pub is_html: bool,
}

/// A message to add or update, matched by account and `remote_id`. Updating
/// a message to a new folder is a move; the counters follow.
#[derive(Debug, Clone, Default)]
pub struct NewMessage {
    /// Owning account.
    pub account: AccountId,
    /// Folder it is in.
    pub folder: FolderId,
    /// Graph immutable ID.
    pub remote_id: String,
    /// RFC 5322 Message-ID.
    pub internet_message_id: Option<String>,
    /// Graph conversation ID; messages that share it form a thread.
    pub conversation_id: String,
    /// Subject line.
    pub subject: String,
    /// Sender.
    pub from: Address,
    /// To recipients.
    pub to: Vec<Address>,
    /// Cc recipients.
    pub cc: Vec<Address>,
    /// Received time (Unix seconds).
    pub received_at: i64,
    /// Sent time (Unix seconds).
    pub sent_at: Option<i64>,
    /// Read state.
    pub is_read: bool,
    /// Flagged state.
    pub is_flagged: bool,
    /// 0 low, 1 normal, 2 high.
    pub importance: i64,
    /// Has file attachments.
    pub has_attachments: bool,
    /// Short body preview.
    pub preview: String,
    /// Body, if fetched. `None` on update leaves the stored body alone.
    pub body: Option<BodyRef>,
}

/// A stored message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    /// Local ID.
    pub id: MessageId,
    /// Owning account.
    pub account: AccountId,
    /// Folder it is in.
    pub folder: FolderId,
    /// Conversation it belongs to.
    pub thread: ThreadId,
    /// Graph immutable ID.
    pub remote_id: String,
    /// RFC 5322 Message-ID.
    pub internet_message_id: Option<String>,
    /// Subject line.
    pub subject: String,
    /// Sender.
    pub from: Address,
    /// To recipients.
    pub to: Vec<Address>,
    /// Cc recipients.
    pub cc: Vec<Address>,
    /// Received time (Unix seconds).
    pub received_at: i64,
    /// Sent time (Unix seconds).
    pub sent_at: Option<i64>,
    /// Read state.
    pub is_read: bool,
    /// Flagged state.
    pub is_flagged: bool,
    /// 0 low, 1 normal, 2 high.
    pub importance: i64,
    /// Has file attachments.
    pub has_attachments: bool,
    /// Short body preview.
    pub preview: String,
    /// Body, if held locally.
    pub body: Option<BodyRef>,
    /// Body is kept even outside the body window.
    pub body_pinned: bool,
    /// When the body joined the on-demand cache (Unix seconds); `None` for
    /// bodies kept because they are in the window or pinned.
    pub body_cached_at: Option<i64>,
}

/// A conversation's summary within one folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderThread {
    /// The conversation.
    pub thread: ThreadId,
    /// The folder.
    pub folder: FolderId,
    /// Newest message time in this folder.
    pub last_received_at: i64,
    /// Newest message in this folder.
    pub last_message: MessageId,
    /// Messages of the thread in this folder.
    pub message_count: i64,
    /// Unread messages of the thread in this folder.
    pub unread_count: i64,
}

/// A keyset paging position: the last row of the previous page. Pages are
/// newest first, so the next page holds rows strictly older than this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// `received_at` of the last row shown.
    pub received_at: i64,
    /// ID of the last row shown (message or thread ID).
    pub id: i64,
}

/// A sync bookmark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncState {
    /// Account.
    pub account: AccountId,
    /// Resource name, e.g. `folders` or `messages:<folder remote id>`.
    pub resource: String,
    /// Folder the resource belongs to, if any.
    pub folder: Option<FolderId>,
    /// Graph delta link to resume from.
    pub delta_link: Option<String>,
    /// Last successful sync (Unix seconds).
    pub synced_at: Option<i64>,
    /// Last error, cleared on success.
    pub last_error: Option<String>,
}

/// Where a queued op is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpState {
    /// Waiting to run.
    Pending,
    /// Claimed by the sync engine.
    InFlight,
    /// Applied on the server.
    Done,
    /// Given up; needs the user.
    Failed,
}

impl OpState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            OpState::Pending => "pending",
            OpState::InFlight => "in_flight",
            OpState::Done => "done",
            OpState::Failed => "failed",
        }
    }

    pub(crate) fn parse(s: &str) -> rusqlite::Result<Self> {
        Ok(match s {
            "pending" => OpState::Pending,
            "in_flight" => OpState::InFlight,
            "done" => OpState::Done,
            "failed" => OpState::Failed,
            _ => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    format!("unknown op state {s:?}").into(),
                ));
            }
        })
    }
}

/// An op to queue.
#[derive(Debug, Clone, Default)]
pub struct NewOp {
    /// Account the op runs against.
    pub account: AccountId,
    /// What it does, e.g. `mark_read`.
    pub kind: String,
    /// What it acts on, e.g. a message remote ID. Ops with the same
    /// non-empty target run one at a time, in order.
    pub target: String,
    /// JSON arguments.
    pub payload: String,
    /// Idempotency key; generated if `None`.
    pub idempotency_key: Option<String>,
}

/// A queued op.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Op {
    /// Local ID; also the queue order.
    pub id: OpId,
    /// Account.
    pub account: AccountId,
    /// What it does.
    pub kind: String,
    /// What it acts on.
    pub target: String,
    /// JSON arguments.
    pub payload: String,
    /// Idempotency key sent to the server.
    pub idempotency_key: String,
    /// State.
    pub state: OpState,
    /// Times it has been claimed.
    pub attempts: i64,
    /// Earliest time it may run (Unix seconds).
    pub not_before: i64,
    /// Last error.
    pub last_error: Option<String>,
    /// When it was queued (Unix seconds).
    pub created_at: i64,
    /// When it last changed (Unix seconds).
    pub updated_at: i64,
}

/// How much of an account's mail has a local body. See [`crate::Reader::body_stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct BodyStats {
    /// Messages (headers) stored.
    pub messages: i64,
    /// Messages with a local body.
    pub with_body: i64,
    /// Messages inside the window that have no body yet.
    pub window_missing: i64,
    /// Messages whose body is pinned.
    pub pinned: i64,
    /// Bodies in the on-demand cache.
    pub cached: i64,
    /// Stored (compressed) bytes of the cached bodies.
    pub cached_bytes: i64,
}

/// What [`crate::WriteTx::evict_cached_bodies`] dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct EvictReport {
    /// Cached bodies before eviction.
    pub before_bytes: i64,
    /// Bodies dropped.
    pub evicted: usize,
    /// Stored bytes of the dropped bodies.
    pub evicted_bytes: i64,
}

/// A batch read from the search queue. See [`crate::Reader::search_queue`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQueueBatch {
    /// The highest queue position read; pass it to
    /// [`crate::WriteTx::clear_search_queue`] once the batch is indexed.
    /// 0 when the queue is empty.
    pub upto: i64,
    /// Messages to re-index (or drop from the index if they no longer
    /// exist), each once.
    pub ids: Vec<MessageId>,
}
