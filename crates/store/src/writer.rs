// SPDX-License-Identifier: MIT OR Apache-2.0
//! The single writer.

use crate::bodies::{self, BodyHash};
use crate::migrate::{self, MIGRATIONS};
use crate::model::*;
use crate::query;
use crate::{Error, Result, StorePaths, now};
use fs4::fs_std::FileExt;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};
use std::collections::HashSet;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// How long [`Writer::open`] waits for the lock before reporting
/// [`Error::WriterLocked`].
const LOCK_WAIT: Duration = Duration::from_secs(2);

/// The one process allowed to change a store. Holds an exclusive lock on
/// `store.lock` until dropped.
pub struct Writer {
    conn: Connection,
    paths: StorePaths,
    // Dropping the file releases the lock. Declared last so the connection
    // closes first.
    _lock: File,
}

/// What [`Writer::gc_bodies`] removed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GcReport {
    /// Body rows no message refers to.
    pub rows: usize,
    /// Body files removed, including orphans with no row.
    pub files: usize,
}

impl Writer {
    /// Opens (creating if needed) the store at `root`, takes the writer
    /// lock, runs pending migrations and returns ops that were in flight
    /// when the last writer stopped to the queue.
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with(root, MIGRATIONS)
    }

    pub(crate) fn open_with(root: &Path, migrations: &[migrate::Migration]) -> Result<Self> {
        let paths = StorePaths::new(root);
        std::fs::create_dir_all(&paths.bodies)
            .map_err(|e| Error::io("create", &paths.bodies, e))?;

        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&paths.lock)
            .map_err(|e| Error::io("open", &paths.lock, e))?;
        // Wait briefly before giving up. On Unix a child process spawned by
        // any thread inherits every open descriptor until it calls exec, so
        // a lock released by a writer that just closed can stay held for a
        // moment by someone else's fork. A real second writer holds it for
        // much longer than this.
        let deadline = std::time::Instant::now() + LOCK_WAIT;
        loop {
            match lock.try_lock_exclusive() {
                Ok(true) => break,
                Ok(false) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(false) => return Err(Error::WriterLocked(paths.root)),
                Err(e) => return Err(Error::io("lock", &paths.lock, e)),
            }
        }

        let mut conn = Connection::open_with_flags(
            &paths.db,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(Error::Sqlite(rusqlite::Error::InvalidParameterName(
                format!(
                    "could not switch {} to WAL mode (got {mode})",
                    paths.db.display()
                ),
            )));
        }
        conn.execute_batch(
            "PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -16000;
             PRAGMA mmap_size = 67108864;
             PRAGMA journal_size_limit = 67108864;",
        )?;
        migrate::migrate(&mut conn, migrations)?;
        // A writer that stopped mid-op never heard back; retry those ops.
        // The idempotency key stops the server applying one twice.
        conn.execute(
            "UPDATE ops SET state = 'pending', updated_at = ?1 WHERE state = 'in_flight'",
            [now()],
        )?;
        Ok(Writer {
            conn,
            paths,
            _lock: lock,
        })
    }

    /// The store's paths.
    pub fn paths(&self) -> &StorePaths {
        &self.paths
    }

    /// Runs `f` in one write transaction. It commits if `f` returns `Ok`
    /// and rolls back otherwise. Readers see none of it until it commits.
    pub fn write<T>(&mut self, f: impl FnOnce(&WriteTx<'_>) -> Result<T>) -> Result<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let wtx = WriteTx {
            tx,
            bodies: &self.paths.bodies,
        };
        let out = f(&wtx)?;
        wtx.tx.commit()?;
        Ok(out)
    }

    /// Deletes bodies no message refers to, then body files with no row
    /// (left by rolled-back transactions or crashes) and stray temporary
    /// files. Safe because no one else writes to the store.
    pub fn gc_bodies(&mut self) -> Result<GcReport> {
        let mut report = GcReport::default();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let unused: Vec<(Vec<u8>, bool)> = {
            let mut s = tx.prepare(
                "SELECT b.hash, b.data IS NULL FROM bodies b
                  WHERE NOT EXISTS (SELECT 1 FROM messages m WHERE m.body_hash = b.hash)",
            )?;
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        {
            let mut del = tx.prepare("DELETE FROM bodies WHERE hash = ?1")?;
            for (hash, _) in &unused {
                del.execute([hash])?;
            }
        }
        // Files the database still points at, taken in the same transaction.
        let keep: HashSet<BodyHash> = {
            let mut s = tx.prepare("SELECT hash FROM bodies WHERE data IS NULL")?;
            s.query_map([], |r| BodyHash::from_blob(r.get(0)?))?
                .collect::<rusqlite::Result<_>>()?
        };
        // Commit before touching files: if the commit failed after a file
        // was gone, a row would point at nothing.
        tx.commit()?;
        report.rows = unused.len();
        report.files = sweep(&self.paths.bodies, &keep)?;
        Ok(report)
    }

    /// Copies the WAL back into the database and truncates it. The daemon
    /// calls this when idle; SQLite also checkpoints on its own.
    pub fn checkpoint(&mut self) -> Result<()> {
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    /// Recomputes every derived row and counter from `messages` and returns
    /// the number that were wrong (and are now fixed). Zero is expected;
    /// the store-versus-server check (WP 1.12) runs this first.
    pub fn repair_derived(&mut self) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let wrong = derived_drift(&tx)?;
        if wrong > 0 {
            tx.execute_batch(
                "UPDATE folders SET
                   total  = (SELECT count(*) FROM messages m WHERE m.folder_id = folders.id),
                   unread = (SELECT count(*) FROM messages m WHERE m.folder_id = folders.id AND m.is_read = 0);
                 DELETE FROM folder_threads;
                 INSERT INTO folder_threads
                 SELECT folder_id, thread_id, max(received_at), id, count(*), sum(is_read = 0)
                   FROM messages GROUP BY folder_id, thread_id;
                 UPDATE threads SET (message_count, unread_count, last_received_at, last_message_id) =
                   (SELECT count(*), coalesce(sum(is_read = 0), 0), max(received_at), id
                      FROM messages WHERE thread_id = threads.id);
                 DELETE FROM threads WHERE message_count = 0;",
            )?;
        }
        tx.commit()?;
        Ok(wrong)
    }
}

/// Counts derived rows that disagree with a full recount from `messages`.
pub(crate) fn derived_drift(c: &Connection) -> Result<usize> {
    let n: i64 = c.query_row(
        "SELECT
           (SELECT count(*) FROM folders f WHERE
               f.total  != (SELECT count(*) FROM messages m WHERE m.folder_id = f.id) OR
               f.unread != (SELECT count(*) FROM messages m WHERE m.folder_id = f.id AND m.is_read = 0))
         + (SELECT count(*) FROM (
               SELECT folder_id, thread_id, max(received_at) t, count(*) n, sum(is_read = 0) u
                 FROM messages GROUP BY folder_id, thread_id
               EXCEPT
               SELECT folder_id, thread_id, last_received_at, message_count, unread_count
                 FROM folder_threads))
         + (SELECT count(*) FROM (
               SELECT folder_id, thread_id, last_received_at, message_count, unread_count
                 FROM folder_threads
               EXCEPT
               SELECT folder_id, thread_id, max(received_at), count(*), sum(is_read = 0)
                 FROM messages GROUP BY folder_id, thread_id))
         + (SELECT count(*) FROM threads t WHERE
               t.message_count != (SELECT count(*) FROM messages m WHERE m.thread_id = t.id) OR
               t.unread_count  != (SELECT count(*) FROM messages m WHERE m.thread_id = t.id AND m.is_read = 0) OR
               t.last_received_at IS NOT (SELECT max(received_at) FROM messages m WHERE m.thread_id = t.id))",
        [],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}

/// Removes body files not in `keep`, and temporary files. Returns how many.
fn sweep(root: &Path, keep: &HashSet<BodyHash>) -> Result<usize> {
    let mut removed = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(Error::io("list", &dir, e)),
        };
        for entry in entries {
            let entry = entry.map_err(|e| Error::io("list", &dir, e))?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|e| Error::io("stat", &path, e))?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let stale = match bodies::hash_of_file_name(&name) {
                Some(hash) => !keep.contains(&hash),
                None => name.ends_with(".tmp"),
            };
            if stale {
                std::fs::remove_file(&path).map_err(|e| Error::io("remove", &path, e))?;
                removed += 1;
            }
        }
    }
    Ok(removed)
}

static OP_KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn new_idempotency_key() -> String {
    let mut h = blake3::Hasher::new();
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    h.update(&t.as_nanos().to_le_bytes());
    h.update(&std::process::id().to_le_bytes());
    h.update(&OP_KEY_COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    let hex = h.finalize().to_hex();
    format!("fam-{}", &hex[..32])
}

/// An open write transaction. Every change goes through one of these.
pub struct WriteTx<'a> {
    tx: Transaction<'a>,
    bodies: &'a Path,
}

impl WriteTx<'_> {
    /// The underlying connection, for reads inside the transaction.
    fn c(&self) -> &Connection {
        &self.tx
    }

    // Accounts --------------------------------------------------------------

    /// Adds an account, or updates the one with the same key.
    pub fn upsert_account(&self, a: &NewAccount) -> Result<AccountId> {
        let provider = if a.provider.is_empty() {
            "graph"
        } else {
            &a.provider
        };
        Ok(self.tx.query_row(
            "INSERT INTO accounts (key, email, display_name, provider, tenant_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (key) DO UPDATE SET
               email = excluded.email, display_name = excluded.display_name,
               provider = excluded.provider, tenant_id = excluded.tenant_id
             RETURNING id",
            params![a.key, a.email, a.display_name, provider, a.tenant_id, now()],
            |r| r.get(0),
        )?)
    }

    /// Deletes an account and everything stored for it.
    pub fn delete_account(&self, id: AccountId) -> Result<()> {
        self.tx
            .execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        Ok(())
    }

    /// All accounts.
    pub fn accounts(&self) -> Result<Vec<Account>> {
        query::accounts(self.c())
    }

    // Folders ---------------------------------------------------------------

    /// Adds a folder, or updates the one with the same account and server ID.
    /// Counters are left alone; they follow the messages.
    pub fn upsert_folder(&self, f: &NewFolder) -> Result<FolderId> {
        Ok(self.tx.query_row(
            "INSERT INTO folders (account_id, remote_id, parent_id, name, well_known)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (account_id, remote_id) DO UPDATE SET
               parent_id = excluded.parent_id, name = excluded.name,
               well_known = excluded.well_known
             RETURNING id",
            params![f.account, f.remote_id, f.parent, f.name, f.well_known],
            |r| r.get(0),
        )?)
    }

    /// Deletes a folder with its messages and sync state. Subfolders move
    /// to the top level; the provider deletes them separately.
    pub fn delete_folder(&self, id: FolderId) -> Result<()> {
        self.tx.execute("DELETE FROM folders WHERE id = ?1", [id])?;
        Ok(())
    }

    /// A folder by ID.
    pub fn folder(&self, id: FolderId) -> Result<Option<Folder>> {
        query::folder_by_id(self.c(), id)
    }

    /// A folder by server ID.
    pub fn folder_by_remote(&self, account: AccountId, remote_id: &str) -> Result<Option<Folder>> {
        query::folder_by_remote(self.c(), account, remote_id)
    }

    // Bodies ----------------------------------------------------------------

    /// Stores raw body bytes, once per distinct content, and returns their
    /// hash for [`NewMessage::body`] or [`WriteTx::set_body`].
    pub fn put_body(&self, bytes: &[u8]) -> Result<BodyHash> {
        bodies::put(self.c(), self.bodies, bytes)
    }

    /// A body's raw bytes.
    pub fn body(&self, hash: &BodyHash) -> Result<Vec<u8>> {
        bodies::get(self.c(), self.bodies, hash)
    }

    /// Sets or clears a message's body. Clearing (when a message leaves the
    /// body window) keeps the header; [`Writer::gc_bodies`] frees the bytes.
    /// Either way the body leaves the on-demand cache; see
    /// [`WriteTx::mark_body_cached`].
    pub fn set_body(&self, id: MessageId, body: Option<BodyRef>) -> Result<()> {
        let n = self.tx.execute(
            "UPDATE messages SET body_hash = ?2, body_is_html = ?3, body_cached_at = NULL
              WHERE id = ?1",
            params![
                id,
                body.as_ref().map(|b| &b.hash.0[..]),
                body.is_some_and(|b| b.is_html)
            ],
        )?;
        found(n, || format!("message {}", id.0))
    }

    /// Pins or unpins a message's body.
    pub fn set_body_pinned(&self, id: MessageId, pinned: bool) -> Result<()> {
        let n = self.tx.execute(
            "UPDATE messages SET body_pinned = ?2 WHERE id = ?1",
            params![id, pinned],
        )?;
        found(n, || format!("message {}", id.0))
    }

    /// Puts a message's body in the on-demand cache (`at` is Unix seconds),
    /// or takes it out with `None`. Cached bodies are kept outside the window
    /// until [`WriteTx::evict_cached_bodies`] needs the room.
    pub fn mark_body_cached(&self, id: MessageId, at: Option<i64>) -> Result<()> {
        let n = self.tx.execute(
            "UPDATE messages SET body_cached_at = ?2 WHERE id = ?1",
            params![id, at],
        )?;
        found(n, || format!("message {}", id.0))
    }

    /// Messages of `account` received at or after `since` with no body,
    /// newest first. See [`crate::Reader::bodies_wanted`].
    pub fn bodies_wanted(
        &self,
        account: AccountId,
        since: i64,
        before: Option<Cursor>,
        limit: usize,
    ) -> Result<Vec<MessageId>> {
        query::bodies_wanted(self.c(), account, since, before, limit)
    }

    /// Drops the bodies of `account`'s messages received before `before`
    /// that are neither pinned nor cached: they have aged out of the window.
    /// Headers stay. Returns how many. [`Writer::gc_bodies`] frees the bytes.
    pub fn age_out_bodies(&self, account: AccountId, before: i64) -> Result<usize> {
        Ok(self.tx.execute(
            "UPDATE messages
                SET body_hash = NULL, body_is_html = 0
              WHERE account_id = ?1 AND received_at < ?2 AND body_hash IS NOT NULL
                AND body_pinned = 0 AND body_cached_at IS NULL",
            params![account, before],
        )?)
    }

    /// Drops cached bodies, least recently cached first, until the cache
    /// holds at most `limit` stored bytes. Pinned bodies are never cached.
    pub fn evict_cached_bodies(&self, limit: i64) -> Result<EvictReport> {
        let rows: Vec<(MessageId, i64)> = {
            let mut s = self.tx.prepare_cached(
                "SELECT m.id, coalesce(b.stored_size, 0)
                   FROM messages m
                   LEFT JOIN bodies b ON b.hash = m.body_hash
                  WHERE m.body_cached_at IS NOT NULL
                  ORDER BY m.body_cached_at, m.id",
            )?;
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut report = EvictReport {
            before_bytes: rows.iter().map(|r| r.1).sum(),
            ..Default::default()
        };
        let mut total = report.before_bytes;
        for (id, size) in rows {
            if total <= limit {
                break;
            }
            self.tx.execute(
                "UPDATE messages SET body_hash = NULL, body_is_html = 0, body_cached_at = NULL
                  WHERE id = ?1",
                [id],
            )?;
            total -= size;
            report.evicted += 1;
            report.evicted_bytes += size;
        }
        Ok(report)
    }

    /// Deletes `account`'s unpinned headers received before `before`, for
    /// `sync.headers` set to a period. Returns how many.
    pub fn prune_headers(&self, account: AccountId, before: i64) -> Result<usize> {
        Ok(self.tx.execute(
            "DELETE FROM messages
              WHERE account_id = ?1 AND received_at < ?2 AND body_pinned = 0",
            params![account, before],
        )?)
    }

    // Messages --------------------------------------------------------------

    fn thread_for(&self, account: AccountId, conversation_id: &str) -> Result<ThreadId> {
        if let Some(id) = query::thread_id(self.c(), account, conversation_id)? {
            return Ok(id);
        }
        Ok(self.tx.query_row(
            "INSERT INTO threads (account_id, conversation_id) VALUES (?1, ?2) RETURNING id",
            params![account, conversation_id],
            |r| r.get(0),
        )?)
    }

    /// Adds a message, or updates the one with the same account and server
    /// ID. A different folder is a move. Folder counters and threads follow.
    pub fn upsert_message(&self, m: &NewMessage) -> Result<MessageId> {
        let thread = self.thread_for(m.account, &m.conversation_id)?;
        let to = serde_json::to_string(&m.to)?;
        let cc = serde_json::to_string(&m.cc)?;
        let mut s = self.tx.prepare_cached(
            "INSERT INTO messages (account_id, folder_id, thread_id, remote_id, internet_message_id,
                subject, from_name, from_addr, to_json, cc_json, received_at, sent_at, is_read,
                is_flagged, importance, has_attachments, preview, body_hash, body_is_html)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT (account_id, remote_id) DO UPDATE SET
               folder_id = excluded.folder_id, thread_id = excluded.thread_id,
               internet_message_id = excluded.internet_message_id, subject = excluded.subject,
               from_name = excluded.from_name, from_addr = excluded.from_addr,
               to_json = excluded.to_json, cc_json = excluded.cc_json,
               received_at = excluded.received_at, sent_at = excluded.sent_at,
               is_read = excluded.is_read, is_flagged = excluded.is_flagged,
               importance = excluded.importance, has_attachments = excluded.has_attachments,
               preview = excluded.preview,
               body_hash = coalesce(excluded.body_hash, body_hash),
               body_is_html = CASE WHEN excluded.body_hash IS NULL THEN body_is_html
                                   ELSE excluded.body_is_html END
             RETURNING id",
        )?;
        let id = s.query_row(
            params![
                m.account,
                m.folder,
                thread,
                m.remote_id,
                m.internet_message_id,
                m.subject,
                m.from.name,
                m.from.addr,
                to,
                cc,
                m.received_at,
                m.sent_at,
                m.is_read,
                m.is_flagged,
                m.importance,
                m.has_attachments,
                m.preview,
                m.body.as_ref().map(|b| &b.hash.0[..]),
                m.body.is_some_and(|b| b.is_html),
            ],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    /// Sets a message's read state.
    pub fn set_read(&self, id: MessageId, is_read: bool) -> Result<()> {
        let n = self.tx.execute(
            "UPDATE messages SET is_read = ?2 WHERE id = ?1 AND is_read != ?2",
            params![id, is_read],
        )?;
        if n == 0 {
            query::require(query::message_by_id(self.c(), id)?, || {
                format!("message {}", id.0)
            })?;
        }
        Ok(())
    }

    /// Sets a message's flag.
    pub fn set_flagged(&self, id: MessageId, is_flagged: bool) -> Result<()> {
        let n = self.tx.execute(
            "UPDATE messages SET is_flagged = ?2 WHERE id = ?1",
            params![id, is_flagged],
        )?;
        found(n, || format!("message {}", id.0))
    }

    /// Moves a message to another folder of the same account.
    pub fn move_message(&self, id: MessageId, folder: FolderId) -> Result<()> {
        let n = self.tx.execute(
            "UPDATE messages SET folder_id = ?2 WHERE id = ?1 AND folder_id != ?2",
            params![id, folder],
        )?;
        if n == 0 {
            query::require(query::message_by_id(self.c(), id)?, || {
                format!("message {}", id.0)
            })?;
        }
        Ok(())
    }

    /// Deletes a message from the store (not from the server).
    pub fn delete_message(&self, id: MessageId) -> Result<()> {
        let n = self
            .tx
            .execute("DELETE FROM messages WHERE id = ?1", [id])?;
        found(n, || format!("message {}", id.0))
    }

    /// Messages with these IDs, in no particular order.
    pub fn messages_by_ids(&self, ids: &[MessageId]) -> Result<Vec<Message>> {
        query::messages_by_ids(self.c(), ids)
    }

    // Search queue -----------------------------------------------------------

    /// The oldest `limit` entries of the search index queue.
    pub fn search_queue(&self, limit: usize) -> Result<SearchQueueBatch> {
        query::search_queue(self.c(), limit)
    }

    /// Removes queue entries up to and including `upto`, once the index has
    /// committed them. Returns how many were removed.
    pub fn clear_search_queue(&self, upto: i64) -> Result<usize> {
        Ok(self
            .tx
            .execute("DELETE FROM search_queue WHERE seq <= ?1", [upto])?)
    }

    /// Queues every message for indexing, as when the index is rebuilt.
    /// Returns how many were queued.
    pub fn requeue_all_for_search(&self) -> Result<usize> {
        self.tx.execute("DELETE FROM search_queue", [])?;
        Ok(self.tx.execute(
            "INSERT INTO search_queue (message_id) SELECT id FROM messages ORDER BY received_at DESC",
            [],
        )?)
    }

    /// A message by ID.
    pub fn message(&self, id: MessageId) -> Result<Option<Message>> {
        query::message_by_id(self.c(), id)
    }

    /// A message by server ID.
    pub fn message_by_remote(
        &self,
        account: AccountId,
        remote_id: &str,
    ) -> Result<Option<Message>> {
        query::message_by_remote(self.c(), account, remote_id)
    }

    /// Messages with this RFC 5322 Message-ID.
    pub fn messages_by_internet_id(&self, internet_message_id: &str) -> Result<Vec<Message>> {
        query::messages_by_internet_id(self.c(), internet_message_id)
    }

    // Sync state ------------------------------------------------------------

    /// Records a successful sync of `resource` and the delta link to resume
    /// from. Clears any error.
    pub fn set_sync_state(
        &self,
        account: AccountId,
        resource: &str,
        folder: Option<FolderId>,
        delta_link: Option<&str>,
        synced_at: i64,
    ) -> Result<()> {
        self.tx.execute(
            "INSERT INTO sync_state (account_id, resource, folder_id, delta_link, synced_at, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)
             ON CONFLICT (account_id, resource) DO UPDATE SET
               folder_id = excluded.folder_id, delta_link = excluded.delta_link,
               synced_at = excluded.synced_at, last_error = NULL",
            params![account, resource, folder, delta_link, synced_at],
        )?;
        Ok(())
    }

    /// Records a sync error for `resource`, keeping its delta link.
    pub fn set_sync_error(&self, account: AccountId, resource: &str, error: &str) -> Result<()> {
        self.tx.execute(
            "INSERT INTO sync_state (account_id, resource, last_error) VALUES (?1, ?2, ?3)
             ON CONFLICT (account_id, resource) DO UPDATE SET last_error = excluded.last_error",
            params![account, resource, error],
        )?;
        Ok(())
    }

    /// Forgets a resource's delta link, so the next sync starts over (after
    /// the server answers 410 Gone, for example).
    pub fn reset_sync_state(&self, account: AccountId, resource: &str) -> Result<()> {
        self.tx.execute(
            "DELETE FROM sync_state WHERE account_id = ?1 AND resource = ?2",
            params![account, resource],
        )?;
        Ok(())
    }

    /// One sync bookmark.
    pub fn sync_state(&self, account: AccountId, resource: &str) -> Result<Option<SyncState>> {
        query::sync_state_of(self.c(), account, resource)
    }

    // Op queue --------------------------------------------------------------

    /// Queues an op. Make the local change in the same transaction, so the
    /// store and the queue always agree.
    pub fn enqueue_op(&self, op: &NewOp) -> Result<OpId> {
        let key = op
            .idempotency_key
            .clone()
            .unwrap_or_else(new_idempotency_key);
        let payload = if op.payload.is_empty() {
            "{}"
        } else {
            &op.payload
        };
        let t = now();
        Ok(self.tx.query_row(
            "INSERT INTO ops (account_id, kind, target, payload, idempotency_key, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6) RETURNING id",
            params![op.account, op.kind, op.target, payload, key, t],
            |r| r.get(0),
        )?)
    }

    /// Claims the oldest op that may run at `now`, marking it in flight.
    /// An op waits while an earlier op on the same non-empty target is
    /// still pending or in flight, so ops on one message apply in order.
    pub fn claim_op(&self, now: i64) -> Result<Option<Op>> {
        let mut s = self.tx.prepare_cached(&format!(
            "UPDATE ops SET state = 'in_flight', attempts = attempts + 1, updated_at = ?1
              WHERE id = (
                SELECT o.id FROM ops o
                 WHERE o.state = 'pending' AND o.not_before <= ?1
                   AND (o.target = '' OR NOT EXISTS (
                         SELECT 1 FROM ops e
                          WHERE e.account_id = o.account_id AND e.target = o.target
                            AND e.id < o.id AND e.state IN ('pending', 'in_flight')))
                 ORDER BY o.id LIMIT 1)
             RETURNING {}",
            query::OP_COLS
        ))?;
        use rusqlite::OptionalExtension;
        Ok(s.query_row([now], query::op).optional()?)
    }

    /// Marks an in-flight op done.
    pub fn complete_op(&self, id: OpId) -> Result<()> {
        self.set_op_state(id, OpState::Done, None, None)
    }

    /// Returns an in-flight op to the queue, to run no earlier than
    /// `not_before` (for `Retry-After` and backoff).
    pub fn retry_op(&self, id: OpId, error: &str, not_before: i64) -> Result<()> {
        self.set_op_state(id, OpState::Pending, Some(error), Some(not_before))
    }

    /// Gives up on an op. It stays visible until the user deals with it.
    pub fn fail_op(&self, id: OpId, error: &str) -> Result<()> {
        self.set_op_state(id, OpState::Failed, Some(error), None)
    }

    fn set_op_state(
        &self,
        id: OpId,
        state: OpState,
        error: Option<&str>,
        not_before: Option<i64>,
    ) -> Result<()> {
        let n = self.tx.execute(
            "UPDATE ops SET state = ?2, last_error = coalesce(?3, last_error),
                    not_before = coalesce(?4, not_before), updated_at = ?5
              WHERE id = ?1",
            params![id, state.as_str(), error, not_before, now()],
        )?;
        found(n, || format!("op {}", id.0))
    }

    /// Deletes done ops last changed before `before`. Returns how many.
    pub fn prune_done_ops(&self, before: i64) -> Result<usize> {
        Ok(self.tx.execute(
            "DELETE FROM ops WHERE state = 'done' AND updated_at < ?1",
            [before],
        )?)
    }

    /// An op by ID.
    pub fn op(&self, id: OpId) -> Result<Option<Op>> {
        query::op_by_id(self.c(), id)
    }

    /// Ops not yet done, in queue order.
    pub fn open_ops(&self) -> Result<Vec<Op>> {
        query::open_ops(self.c())
    }
}

fn found(n: usize, what: impl FnOnce() -> String) -> Result<()> {
    if n == 0 {
        Err(Error::NotFound(what()))
    } else {
        Ok(())
    }
}
