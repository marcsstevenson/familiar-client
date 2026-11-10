// SPDX-License-Identifier: MIT OR Apache-2.0
//! Snoozed messages (schema version 7).
//!
//! Snoozing moves a message into the account's [`SNOOZED`] folder and keeps
//! a row here saying when it comes back and where to. famd looks for rows
//! that are due and moves those messages back. The row is deleted with its
//! message.

use crate::model::{FolderId, MessageId};
use crate::writer::WriteTx;
use crate::{Reader, Result, now};
use rusqlite::{Connection, params};
use serde::Serialize;

/// The name of the top-level folder snoozed mail waits in.
pub const SNOOZED: &str = "Snoozed";

/// A snoozed message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Snooze {
    /// The message.
    pub message: MessageId,
    /// When it comes back (Unix seconds).
    pub until: i64,
    /// Where it goes back to; `None` when that folder is gone (the Inbox then).
    pub return_folder: Option<FolderId>,
}

fn snooze(r: &rusqlite::Row<'_>) -> rusqlite::Result<Snooze> {
    Ok(Snooze {
        message: MessageId(r.get(0)?),
        until: r.get(1)?,
        return_folder: r.get::<_, Option<i64>>(2)?.map(FolderId),
    })
}

fn query(c: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Vec<Snooze>> {
    let mut s = c.prepare_cached(sql)?;
    let rows = s.query_map(p, snooze)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

const COLS: &str = "SELECT message_id, until, return_folder_id FROM snoozes";

impl WriteTx<'_> {
    /// Snoozes `message` until `until`. Snoozing again with no
    /// `return_folder` (from inside Snoozed) only moves the time; with one
    /// (it was moved out and snoozed from there) it goes back there instead.
    pub fn set_snooze(
        &self,
        message: MessageId,
        until: i64,
        return_folder: Option<FolderId>,
    ) -> Result<()> {
        self.tx.execute(
            "INSERT INTO snoozes (message_id, until, return_folder_id, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (message_id) DO UPDATE SET until = excluded.until,
               return_folder_id = coalesce(excluded.return_folder_id, return_folder_id)",
            params![message, until, return_folder, now()],
        )?;
        Ok(())
    }

    /// Ends a snooze, returning it (`None` if the message wasn't snoozed).
    pub fn take_snooze(&self, message: MessageId) -> Result<Option<Snooze>> {
        let s = self.snooze(message)?;
        if s.is_some() {
            self.tx
                .execute("DELETE FROM snoozes WHERE message_id = ?1", [message])?;
        }
        Ok(s)
    }

    /// The message's snooze, if it has one.
    pub fn snooze(&self, message: MessageId) -> Result<Option<Snooze>> {
        Ok(query(
            &self.tx,
            &format!("{COLS} WHERE message_id = ?1"),
            [message],
        )?
        .pop())
    }

    /// Snoozes due at `at` or before, earliest first.
    pub fn due_snoozes(&self, at: i64) -> Result<Vec<Snooze>> {
        query(
            &self.tx,
            &format!("{COLS} WHERE until <= ?1 ORDER BY until, message_id"),
            [at],
        )
    }
}

impl Reader {
    /// Every snooze, soonest back first.
    pub fn snoozes(&self) -> Result<Vec<Snooze>> {
        query(
            &self.conn,
            &format!("{COLS} ORDER BY until, message_id"),
            [],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Writer;
    use crate::model::{AccountId, NewAccount, NewFolder, NewMessage};

    fn setup(w: &mut Writer) -> (AccountId, FolderId, MessageId) {
        w.write(|t| {
            let a = t.upsert_account(&NewAccount {
                key: "home".into(),
                email: "me@example.com".into(),
                ..Default::default()
            })?;
            let f = t.upsert_folder(&NewFolder {
                account: a,
                remote_id: "inbox".into(),
                name: "Inbox".into(),
                well_known: Some("inbox".into()),
                ..Default::default()
            })?;
            let m = t.upsert_message(&NewMessage {
                account: a,
                folder: f,
                remote_id: "m1".into(),
                conversation_id: "c1".into(),
                received_at: 1,
                ..Default::default()
            })?;
            Ok((a, f, m))
        })
        .unwrap()
    }

    #[test]
    fn set_due_take_and_cascade() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = Writer::open(dir.path()).unwrap();
        let (_, inbox, m) = setup(&mut w);
        w.write(|t| t.set_snooze(m, 100, Some(inbox))).unwrap();
        // Snoozing again moves the time but keeps where it goes back to.
        w.write(|t| t.set_snooze(m, 200, None)).unwrap();
        let r = Reader::open(dir.path()).unwrap();
        assert_eq!(
            r.snoozes().unwrap(),
            vec![Snooze {
                message: m,
                until: 200,
                return_folder: Some(inbox)
            }]
        );
        // Snoozed again from another folder: it goes back there.
        w.write(|t| {
            let other = t.upsert_folder(&NewFolder {
                account: t.message(m)?.unwrap().account,
                remote_id: "archive".into(),
                name: "Archive".into(),
                ..Default::default()
            })?;
            t.set_snooze(m, 200, Some(other))?;
            assert_eq!(t.snooze(m)?.unwrap().return_folder, Some(other));
            t.set_snooze(m, 200, Some(inbox))
        })
        .unwrap();
        assert!(w.write(|t| t.due_snoozes(199)).unwrap().is_empty());
        assert_eq!(w.write(|t| t.due_snoozes(200)).unwrap().len(), 1);
        assert!(w.write(|t| t.take_snooze(m)).unwrap().is_some());
        assert!(w.write(|t| t.take_snooze(m)).unwrap().is_none());

        w.write(|t| t.set_snooze(m, 100, Some(inbox))).unwrap();
        w.write(|t| t.delete_message(m)).unwrap();
        assert!(r.snoozes().unwrap().is_empty());
    }
}
