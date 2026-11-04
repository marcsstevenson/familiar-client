// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read-only access for the GUI and CLI.

use crate::bodies::{self, BodyHash};
use crate::migrate::{self, SCHEMA_VERSION};
use crate::model::*;
use crate::query;
use crate::{Error, Result, StorePaths};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::time::Duration;

/// A read-only view of a store. Any number of readers, in any number of
/// processes, can be open alongside the one [`crate::Writer`]. Readers never
/// block the writer.
///
/// Each call reads the latest committed state. To make several calls see
/// the same state, wrap them in [`Reader::snapshot`].
pub struct Reader {
    conn: Connection,
    paths: StorePaths,
}

impl Reader {
    /// Opens the store at `root` for reading. The writer must have created
    /// and migrated it.
    pub fn open(root: &Path) -> Result<Self> {
        let paths = StorePaths::new(root);
        if !paths.db.exists() {
            return Err(Error::NoStore(paths.root));
        }
        let conn = Connection::open_with_flags(
            &paths.db,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )?;
        // A reader can be briefly busy while the WAL index is rebuilt after
        // a crash; it never waits on the writer's transactions.
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA query_only = ON;
             PRAGMA cache_size = -8000;
             PRAGMA mmap_size = 67108864;",
        )?;
        let found = migrate::version(&conn)?;
        if found < SCHEMA_VERSION {
            return Err(Error::SchemaTooOld {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        if found > SCHEMA_VERSION {
            return Err(Error::SchemaTooNew {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        Ok(Reader { conn, paths })
    }

    /// The store's paths.
    pub fn paths(&self) -> &StorePaths {
        &self.paths
    }

    /// Runs `f` inside one read transaction, so every call in it sees the
    /// same committed state even while the writer commits.
    pub fn snapshot<T>(&mut self, f: impl FnOnce(&Reader) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN DEFERRED")?;
        let out = f(self);
        // A read transaction has nothing to commit; ending it releases the
        // snapshot so the WAL can be checkpointed.
        self.conn.execute_batch("COMMIT")?;
        out
    }

    /// All accounts.
    pub fn accounts(&self) -> Result<Vec<Account>> {
        query::accounts(&self.conn)
    }

    /// An account by its config key.
    pub fn account_by_key(&self, key: &str) -> Result<Option<Account>> {
        query::account_by_key(&self.conn, key)
    }

    /// An account's folders with their stored counters.
    pub fn folders(&self, account: AccountId) -> Result<Vec<Folder>> {
        query::folders(&self.conn, account)
    }

    /// A folder by ID.
    pub fn folder(&self, id: FolderId) -> Result<Option<Folder>> {
        query::folder_by_id(&self.conn, id)
    }

    /// A folder by its server ID.
    pub fn folder_by_remote(&self, account: AccountId, remote_id: &str) -> Result<Option<Folder>> {
        query::folder_by_remote(&self.conn, account, remote_id)
    }

    /// A page of a folder's messages, newest first. Pass the last row of the
    /// previous page as `after` to get the next.
    pub fn messages(
        &self,
        folder: FolderId,
        after: Option<Cursor>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        query::folder_messages(&self.conn, folder, after, limit)
    }

    /// A page of a folder's conversations, newest first.
    pub fn threads(
        &self,
        folder: FolderId,
        after: Option<Cursor>,
        limit: usize,
    ) -> Result<Vec<FolderThread>> {
        query::folder_threads(&self.conn, folder, after, limit)
    }

    /// Every message of a conversation, in all folders, oldest first.
    pub fn thread(&self, thread: ThreadId) -> Result<Vec<Message>> {
        query::thread_messages(&self.conn, thread)
    }

    /// A message by ID.
    pub fn message(&self, id: MessageId) -> Result<Option<Message>> {
        query::message_by_id(&self.conn, id)
    }

    /// A message by its server ID.
    pub fn message_by_remote(
        &self,
        account: AccountId,
        remote_id: &str,
    ) -> Result<Option<Message>> {
        query::message_by_remote(&self.conn, account, remote_id)
    }

    /// Messages with this RFC 5322 Message-ID, in any account or folder.
    pub fn messages_by_internet_id(&self, internet_message_id: &str) -> Result<Vec<Message>> {
        query::messages_by_internet_id(&self.conn, internet_message_id)
    }

    /// A body's raw bytes.
    pub fn body(&self, hash: &BodyHash) -> Result<Vec<u8>> {
        bodies::get(&self.conn, &self.paths.bodies, hash)
    }

    /// One sync bookmark.
    pub fn sync_state(&self, account: AccountId, resource: &str) -> Result<Option<SyncState>> {
        query::sync_state_of(&self.conn, account, resource)
    }

    /// All of an account's sync bookmarks.
    pub fn sync_states(&self, account: AccountId) -> Result<Vec<SyncState>> {
        query::sync_states(&self.conn, account)
    }

    /// Queued ops that are not done, in queue order.
    pub fn open_ops(&self) -> Result<Vec<Op>> {
        query::open_ops(&self.conn)
    }
}
