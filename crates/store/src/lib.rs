// SPDX-License-Identifier: MIT OR Apache-2.0
//! The local mail store: SQLite in WAL mode for metadata, plus message bodies
//! keyed by content hash.
//!
//! # The single-writer rule
//!
//! Exactly one process, `famd`, writes to a store. [`Writer::open`] takes an
//! exclusive lock on `store.lock` in the store directory and holds it for the
//! life of the [`Writer`]; a second writer gets [`Error::WriterLocked`]. The
//! GUI and the CLI open a [`Reader`] instead, which uses a read-only SQLite
//! connection and never takes the lock. In WAL mode readers see a consistent
//! snapshot and never block the writer, and the writer never blocks them.
//! Readers send changes to the daemon, which applies them through its
//! [`Writer`].
//!
//! Only the writer runs migrations. A reader refuses a store whose schema is
//! older or newer than the one it was built for.
//!
//! # Layout
//!
//! ```text
//! <root>/store.sqlite      metadata (plus -wal and -shm while open)
//! <root>/store.lock        held by the writer
//! <root>/bodies/ab/cd/<blake3 hex>.zst   bodies too big to keep inline
//! ```
//!
//! Bodies are zstd-compressed and named by the BLAKE3 hash of the raw bytes,
//! so identical bodies are stored once. Bodies that compress to
//! [`INLINE_BODY_MAX`] bytes or less are kept inline in SQLite, because one
//! small file per body wasted three quarters of the disk in WP 0.5.

mod bodies;
mod calendar;
mod contacts;
mod conversation;
mod error;
mod migrate;
mod model;
mod query;
mod reader;
mod shared;
mod writer;

pub use bodies::{BodyHash, INLINE_BODY_MAX};
pub use calendar::{Attendee, Calendar, Event, EventData, EventRange, NewCalendar};
pub use contacts::{Contact, NewContact, Person, SenderHistory};
pub use conversation::{
    Conversation, ConversationFolder, ConversationMessage, ConversationSummary, DRAFTS,
    LatestMessage, MessageCopy, SENT_ITEMS, TRASH_FOLDERS, normalize_subject,
};
pub use error::{Error, Result};
pub use migrate::{MIGRATIONS, Migration, SCHEMA_VERSION};
pub use model::{
    Account, AccountId, Address, AuditEntry, BodyRef, BodyStats, CalendarId, ContactId, Cursor,
    EventId, EvictReport, Folder, FolderId, FolderThread, LOCAL_REMOTE_PREFIX, Message,
    MessageFilter, MessageId, NewAccount, NewAudit, NewFolder, NewMessage, NewOp, Op, OpId,
    OpState, SearchQueueBatch, SortText, SyncState, ThreadId, event_target, folder_target,
    message_target, series_target,
};
pub use reader::Reader;
pub use shared::{SharedWriter, StoreWriter};
pub use writer::{GcReport, WriteTx, Writer};

use std::path::{Path, PathBuf};

/// The crate name, used by the workspace smoke tests.
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Where the parts of a store live, relative to its root directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePaths {
    /// The store directory.
    pub root: PathBuf,
    /// The SQLite database.
    pub db: PathBuf,
    /// The lock file the writer holds.
    pub lock: PathBuf,
    /// The directory of body files.
    pub bodies: PathBuf,
}

impl StorePaths {
    /// Paths for a store rooted at `root`.
    pub fn new(root: &Path) -> Self {
        StorePaths {
            root: root.to_path_buf(),
            db: root.join("store.sqlite"),
            lock: root.join("store.lock"),
            bodies: root.join("bodies"),
        }
    }
}

/// The current time as Unix seconds.
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    #[test]
    fn name_is_set() {
        assert_eq!(super::NAME, "familiar-store");
    }
}
