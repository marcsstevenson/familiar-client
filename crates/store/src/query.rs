// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read queries shared by [`crate::Reader`] and [`crate::WriteTx`].

use crate::bodies::BodyHash;
use crate::model::*;
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};

const ACCOUNT_COLS: &str = "id, key, email, display_name, provider, tenant_id, created_at";

fn account(r: &Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: r.get(0)?,
        key: r.get(1)?,
        email: r.get(2)?,
        display_name: r.get(3)?,
        provider: r.get(4)?,
        tenant_id: r.get(5)?,
        created_at: r.get(6)?,
    })
}

pub(crate) fn accounts(c: &Connection) -> Result<Vec<Account>> {
    let mut s = c.prepare_cached(&format!("SELECT {ACCOUNT_COLS} FROM accounts ORDER BY id"))?;
    Ok(s.query_map([], account)?.collect::<Result<_, _>>()?)
}

pub(crate) fn account_by_key(c: &Connection, key: &str) -> Result<Option<Account>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {ACCOUNT_COLS} FROM accounts WHERE key = ?1"
    ))?;
    Ok(s.query_row([key], account).optional()?)
}

const FOLDER_COLS: &str = "id, account_id, remote_id, parent_id, name, well_known, total, unread";

fn folder(r: &Row<'_>) -> rusqlite::Result<Folder> {
    Ok(Folder {
        id: r.get(0)?,
        account: r.get(1)?,
        remote_id: r.get(2)?,
        parent: r.get(3)?,
        name: r.get(4)?,
        well_known: r.get(5)?,
        total: r.get(6)?,
        unread: r.get(7)?,
    })
}

pub(crate) fn folders(c: &Connection, account: AccountId) -> Result<Vec<Folder>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {FOLDER_COLS} FROM folders WHERE account_id = ?1 ORDER BY id"
    ))?;
    Ok(s.query_map([account], folder)?.collect::<Result<_, _>>()?)
}

pub(crate) fn folder_by_id(c: &Connection, id: FolderId) -> Result<Option<Folder>> {
    let mut s = c.prepare_cached(&format!("SELECT {FOLDER_COLS} FROM folders WHERE id = ?1"))?;
    Ok(s.query_row([id], folder).optional()?)
}

pub(crate) fn folder_by_remote(
    c: &Connection,
    account: AccountId,
    remote_id: &str,
) -> Result<Option<Folder>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {FOLDER_COLS} FROM folders WHERE account_id = ?1 AND remote_id = ?2"
    ))?;
    Ok(s.query_row(params![account, remote_id], folder)
        .optional()?)
}

const MESSAGE_COLS: &str = "m.id, m.account_id, m.folder_id, m.thread_id, m.remote_id,
    m.internet_message_id, m.subject, m.from_name, m.from_addr, m.to_json, m.cc_json,
    m.received_at, m.sent_at, m.is_read, m.is_flagged, m.importance, m.has_attachments,
    m.preview, m.body_hash, m.body_is_html, m.body_pinned, m.body_cached_at";

fn json_col<T: serde::de::DeserializeOwned>(r: &Row<'_>, i: usize) -> rusqlite::Result<T> {
    let s: String = r.get(i)?;
    serde_json::from_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn message(r: &Row<'_>) -> rusqlite::Result<Message> {
    let hash: Option<Vec<u8>> = r.get(18)?;
    let is_html: bool = r.get(19)?;
    Ok(Message {
        id: r.get(0)?,
        account: r.get(1)?,
        folder: r.get(2)?,
        thread: r.get(3)?,
        remote_id: r.get(4)?,
        internet_message_id: r.get(5)?,
        subject: r.get(6)?,
        from: Address {
            name: r.get(7)?,
            addr: r.get(8)?,
        },
        to: json_col(r, 9)?,
        cc: json_col(r, 10)?,
        received_at: r.get(11)?,
        sent_at: r.get(12)?,
        is_read: r.get(13)?,
        is_flagged: r.get(14)?,
        importance: r.get(15)?,
        has_attachments: r.get(16)?,
        preview: r.get(17)?,
        body: hash
            .map(BodyHash::from_blob)
            .transpose()?
            .map(|hash| BodyRef { hash, is_html }),
        body_pinned: r.get(20)?,
        body_cached_at: r.get(21)?,
    })
}

pub(crate) fn message_by_id(c: &Connection, id: MessageId) -> Result<Option<Message>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {MESSAGE_COLS} FROM messages m WHERE m.id = ?1"
    ))?;
    Ok(s.query_row([id], message).optional()?)
}

pub(crate) fn message_by_remote(
    c: &Connection,
    account: AccountId,
    remote_id: &str,
) -> Result<Option<Message>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {MESSAGE_COLS} FROM messages m WHERE m.account_id = ?1 AND m.remote_id = ?2"
    ))?;
    Ok(s.query_row(params![account, remote_id], message)
        .optional()?)
}

pub(crate) fn messages_by_internet_id(c: &Connection, imid: &str) -> Result<Vec<Message>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {MESSAGE_COLS} FROM messages m WHERE m.internet_message_id = ?1 ORDER BY m.id"
    ))?;
    Ok(s.query_map([imid], message)?.collect::<Result<_, _>>()?)
}

/// Newest first, keyset paged.
pub(crate) fn folder_messages(
    c: &Connection,
    folder: FolderId,
    after: Option<Cursor>,
    limit: usize,
) -> Result<Vec<Message>> {
    let (t, id) = after.map_or((i64::MAX, i64::MAX), |k| (k.received_at, k.id));
    let mut s = c.prepare_cached(&format!(
        "SELECT {MESSAGE_COLS} FROM messages m
          WHERE m.folder_id = ?1 AND (m.received_at, m.id) < (?2, ?3)
          ORDER BY m.received_at DESC, m.id DESC LIMIT ?4"
    ))?;
    Ok(s.query_map(params![folder, t, id, limit as i64], message)?
        .collect::<Result<_, _>>()?)
}

/// Like [`folder_messages`], keeping only unread and/or flagged messages.
pub(crate) fn folder_messages_filtered(
    c: &Connection,
    folder: FolderId,
    filter: MessageFilter,
    after: Option<Cursor>,
    limit: usize,
) -> Result<Vec<Message>> {
    let (t, id) = after.map_or((i64::MAX, i64::MAX), |k| (k.received_at, k.id));
    let mut s = c.prepare_cached(&format!(
        "SELECT {MESSAGE_COLS} FROM messages m
          WHERE m.folder_id = ?1 AND (m.received_at, m.id) < (?2, ?3)
            AND (?5 = 0 OR m.is_read = 0) AND (?6 = 0 OR m.is_flagged = 1)
          ORDER BY m.received_at DESC, m.id DESC LIMIT ?4"
    ))?;
    Ok(s.query_map(
        params![
            folder,
            t,
            id,
            limit as i64,
            filter.unread_only,
            filter.flagged_only
        ],
        message,
    )?
    .collect::<Result<_, _>>()?)
}

/// Threads in a folder, newest first, keyset paged on (last_received_at, thread id).
pub(crate) fn folder_threads(
    c: &Connection,
    folder: FolderId,
    after: Option<Cursor>,
    limit: usize,
) -> Result<Vec<FolderThread>> {
    let (t, id) = after.map_or((i64::MAX, i64::MAX), |k| (k.received_at, k.id));
    let mut s = c.prepare_cached(
        "SELECT thread_id, folder_id, last_received_at, last_message_id, message_count, unread_count
           FROM folder_threads
          WHERE folder_id = ?1 AND (last_received_at, thread_id) < (?2, ?3)
          ORDER BY last_received_at DESC, thread_id DESC LIMIT ?4",
    )?;
    let rows = s.query_map(params![folder, t, id, limit as i64], |r| {
        Ok(FolderThread {
            thread: r.get(0)?,
            folder: r.get(1)?,
            last_received_at: r.get(2)?,
            last_message: r.get(3)?,
            message_count: r.get(4)?,
            unread_count: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Every message of a conversation, across folders, oldest first.
pub(crate) fn thread_messages(c: &Connection, thread: ThreadId) -> Result<Vec<Message>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {MESSAGE_COLS} FROM messages m WHERE m.thread_id = ?1
          ORDER BY m.received_at, m.id"
    ))?;
    Ok(s.query_map([thread], message)?.collect::<Result<_, _>>()?)
}

pub(crate) fn thread_id(
    c: &Connection,
    account: AccountId,
    conversation_id: &str,
) -> Result<Option<ThreadId>> {
    let mut s =
        c.prepare_cached("SELECT id FROM threads WHERE account_id = ?1 AND conversation_id = ?2")?;
    Ok(s.query_row(params![account, conversation_id], |r| r.get(0))
        .optional()?)
}

fn sync_state(r: &Row<'_>) -> rusqlite::Result<SyncState> {
    Ok(SyncState {
        account: r.get(0)?,
        resource: r.get(1)?,
        folder: r.get(2)?,
        delta_link: r.get(3)?,
        synced_at: r.get(4)?,
        last_error: r.get(5)?,
    })
}

const SYNC_COLS: &str = "account_id, resource, folder_id, delta_link, synced_at, last_error";

pub(crate) fn sync_state_of(
    c: &Connection,
    account: AccountId,
    resource: &str,
) -> Result<Option<SyncState>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {SYNC_COLS} FROM sync_state WHERE account_id = ?1 AND resource = ?2"
    ))?;
    Ok(s.query_row(params![account, resource], sync_state)
        .optional()?)
}

pub(crate) fn sync_states(c: &Connection, account: AccountId) -> Result<Vec<SyncState>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {SYNC_COLS} FROM sync_state WHERE account_id = ?1 ORDER BY resource"
    ))?;
    Ok(s.query_map([account], sync_state)?
        .collect::<Result<_, _>>()?)
}

pub(crate) const OP_COLS: &str = "id, account_id, kind, target, payload, idempotency_key, state,
    attempts, not_before, last_error, created_at, updated_at, source, summary";

pub(crate) fn op(r: &Row<'_>) -> rusqlite::Result<Op> {
    let state: String = r.get(6)?;
    Ok(Op {
        id: r.get(0)?,
        account: r.get(1)?,
        kind: r.get(2)?,
        target: r.get(3)?,
        payload: r.get(4)?,
        idempotency_key: r.get(5)?,
        state: OpState::parse(&state)?,
        attempts: r.get(7)?,
        not_before: r.get(8)?,
        last_error: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        source: r.get(12)?,
        summary: r.get(13)?,
    })
}

/// Recent ops, newest first: open ones plus done ones changed at or after
/// `done_since`.
pub(crate) fn recent_ops(c: &Connection, done_since: i64, limit: usize) -> Result<Vec<Op>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {OP_COLS} FROM ops WHERE state != 'done' OR updated_at >= ?1
          ORDER BY id DESC LIMIT ?2"
    ))?;
    Ok(s.query_map(params![done_since, limit as i64], op)?
        .collect::<Result<_, _>>()?)
}

pub(crate) const AUDIT_COLS: &str = "id, at, account_id, source, command, outcome, op_id, detail";

pub(crate) fn audit(r: &Row<'_>) -> rusqlite::Result<AuditEntry> {
    Ok(AuditEntry {
        id: r.get(0)?,
        at: r.get(1)?,
        account: r.get(2)?,
        source: r.get(3)?,
        command: r.get(4)?,
        outcome: r.get(5)?,
        op: r.get(6)?,
        detail: r.get(7)?,
    })
}

/// Audit rows at or after `since`, newest first.
pub(crate) fn audit_log(c: &Connection, since: i64, limit: usize) -> Result<Vec<AuditEntry>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {AUDIT_COLS} FROM audit WHERE at >= ?1 ORDER BY id DESC LIMIT ?2"
    ))?;
    Ok(s.query_map(params![since, limit as i64], audit)?
        .collect::<Result<_, _>>()?)
}

/// Audit rows at or after `since`, newest first, only of `source` and
/// `outcome` when given (`source` ending in `*` matches the start).
pub(crate) fn audit_filtered(
    c: &Connection,
    since: i64,
    source: Option<&str>,
    outcome: Option<&str>,
    limit: usize,
) -> Result<Vec<AuditEntry>> {
    let esc = |s: &str| {
        s.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    };
    let source = source.map(|s| match s.strip_suffix('*') {
        Some(p) => format!("{}%", esc(p)),
        None => esc(s),
    });
    let mut s = c.prepare_cached(&format!(
        "SELECT {AUDIT_COLS} FROM audit WHERE at >= ?1
           AND (?2 IS NULL OR source LIKE ?2 ESCAPE '\\')
           AND (?3 IS NULL OR outcome = ?3)
         ORDER BY id DESC LIMIT ?4"
    ))?;
    Ok(
        s.query_map(params![since, source, outcome, limit as i64], audit)?
            .collect::<Result<_, _>>()?,
    )
}

pub(crate) fn op_by_id(c: &Connection, id: OpId) -> Result<Option<Op>> {
    let mut s = c.prepare_cached(&format!("SELECT {OP_COLS} FROM ops WHERE id = ?1"))?;
    Ok(s.query_row([id], op).optional()?)
}

/// Ops not yet done (pending, in flight or failed), in queue order.
pub(crate) fn open_ops(c: &Connection) -> Result<Vec<Op>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {OP_COLS} FROM ops WHERE state != 'done' ORDER BY id"
    ))?;
    Ok(s.query_map([], op)?.collect::<Result<_, _>>()?)
}

/// Messages of `account` received at or after `since` with no body, newest
/// first, strictly older than `before` when given.
pub(crate) fn bodies_wanted(
    c: &Connection,
    account: AccountId,
    since: i64,
    before: Option<Cursor>,
    limit: usize,
) -> Result<Vec<MessageId>> {
    let (t, id) = before.map_or((i64::MAX, i64::MAX), |k| (k.received_at, k.id));
    let mut s = c.prepare_cached(
        "SELECT id FROM messages
          WHERE account_id = ?1 AND received_at >= ?2 AND body_hash IS NULL
            AND (received_at, id) < (?3, ?4)
          ORDER BY received_at DESC, id DESC LIMIT ?5",
    )?;
    Ok(
        s.query_map(params![account, since, t, id, limit as i64], |r| r.get(0))?
            .collect::<Result<_, _>>()?,
    )
}

pub(crate) fn body_stats(c: &Connection, account: AccountId, since: i64) -> Result<BodyStats> {
    let mut s = c.prepare_cached(
        "SELECT count(*),
                coalesce(sum(m.body_hash IS NOT NULL), 0),
                coalesce(sum(m.body_hash IS NULL AND m.received_at >= ?2), 0),
                coalesce(sum(m.body_pinned), 0),
                coalesce(sum(m.body_cached_at IS NOT NULL AND m.body_hash IS NOT NULL), 0),
                coalesce(sum(CASE WHEN m.body_cached_at IS NOT NULL THEN b.stored_size END), 0)
           FROM messages m LEFT JOIN bodies b ON b.hash = m.body_hash
          WHERE m.account_id = ?1",
    )?;
    Ok(s.query_row(params![account, since], |r| {
        Ok(BodyStats {
            messages: r.get(0)?,
            with_body: r.get(1)?,
            window_missing: r.get(2)?,
            pinned: r.get(3)?,
            cached: r.get(4)?,
            cached_bytes: r.get(5)?,
        })
    })?)
}

pub(crate) fn stored_body_bytes(c: &Connection) -> Result<i64> {
    Ok(c.query_row(
        "SELECT coalesce(sum(stored_size), 0) FROM bodies",
        [],
        |r| r.get(0),
    )?)
}

pub(crate) fn require<T>(v: Option<T>, what: impl FnOnce() -> String) -> Result<T> {
    v.ok_or_else(|| Error::NotFound(what()))
}

/// Messages with these IDs, in no particular order. IDs with no row are
/// left out.
pub(crate) fn messages_by_ids(c: &Connection, ids: &[MessageId]) -> Result<Vec<Message>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let list = serde_json::to_string(&ids.iter().map(|i| i.0).collect::<Vec<_>>())?;
    let mut s = c.prepare_cached(&format!(
        "SELECT {MESSAGE_COLS} FROM messages m
          WHERE m.id IN (SELECT value FROM json_each(?1))"
    ))?;
    Ok(s.query_map([list], message)?.collect::<Result<_, _>>()?)
}

/// The oldest `limit` rows of the search queue: the distinct message IDs in
/// them and the highest `seq` read.
pub(crate) fn search_queue(c: &Connection, limit: usize) -> Result<SearchQueueBatch> {
    let mut s =
        c.prepare_cached("SELECT seq, message_id FROM search_queue ORDER BY seq LIMIT ?1")?;
    let mut upto = 0;
    let mut seen = std::collections::HashSet::new();
    let mut ids = Vec::new();
    let rows = s.query_map([limit as i64], |r| {
        Ok((r.get::<_, i64>(0)?, MessageId(r.get(1)?)))
    })?;
    for row in rows {
        let (seq, id) = row?;
        upto = upto.max(seq);
        if seen.insert(id) {
            ids.push(id);
        }
    }
    Ok(SearchQueueBatch { upto, ids })
}

pub(crate) fn search_queue_len(c: &Connection) -> Result<i64> {
    Ok(c.query_row("SELECT count(*) FROM search_queue", [], |r| r.get(0))?)
}

/// IDs of unread (`unread`) or flagged messages, from the partial indexes.
pub(crate) fn ids_where(c: &Connection, flagged: bool) -> Result<Vec<MessageId>> {
    let sql = if flagged {
        "SELECT id FROM messages WHERE is_flagged = 1"
    } else {
        "SELECT id FROM messages WHERE is_read = 0"
    };
    let mut s = c.prepare_cached(sql)?;
    Ok(s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?)
}

/// See [`crate::Reader::message_order`].
pub(crate) fn message_order(
    c: &Connection,
    folders: &[FolderId],
    unread_only: bool,
) -> Result<Vec<(MessageId, i64)>> {
    let mut out = Vec::new();
    // One indexed scan per folder, merged by time, keeps each query on
    // messages_folder_time and avoids a sort over the whole table.
    for f in folders {
        let mut s = c.prepare_cached(
            "SELECT m.id, m.received_at FROM messages m
              WHERE m.folder_id = ?1 AND (?2 = 0 OR m.is_read = 0)
              ORDER BY m.received_at DESC, m.id DESC",
        )?;
        let rows = s.query_map(params![f, unread_only], |r| {
            Ok((MessageId(r.get(0)?), r.get::<_, i64>(1)?))
        })?;
        for r in rows {
            out.push(r?);
        }
    }
    if folders.len() > 1 {
        out.sort_unstable_by_key(|&(id, at)| std::cmp::Reverse((at, id)));
    }
    Ok(out)
}

/// See [`crate::Reader::message_sort_text`].
pub(crate) fn message_sort_text(
    c: &Connection,
    folders: &[FolderId],
    unread_only: bool,
    column: SortText,
) -> Result<Vec<(MessageId, String)>> {
    let col = match column {
        SortText::From => "CASE WHEN m.from_name = '' THEN m.from_addr ELSE m.from_name END",
        SortText::Subject => "m.subject",
    };
    let mut out = Vec::new();
    for f in folders {
        let mut s = c.prepare_cached(&format!(
            "SELECT m.id, {col} FROM messages m
              WHERE m.folder_id = ?1 AND (?2 = 0 OR m.is_read = 0)"
        ))?;
        let rows = s.query_map(params![f, unread_only], |r| {
            Ok((MessageId(r.get(0)?), r.get::<_, String>(1)?))
        })?;
        for r in rows {
            out.push(r?);
        }
    }
    Ok(out)
}
