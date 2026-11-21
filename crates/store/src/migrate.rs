// SPDX-License-Identifier: MIT OR Apache-2.0
//! Schema migrations, tracked in SQLite's `user_version`.
//!
//! Migrations are append-only. Each runs in its own transaction together with
//! the `user_version` bump, so a crash leaves the store at the previous
//! version, never half-way.

use crate::{Error, Result};
use rusqlite::{Connection, TransactionBehavior};

/// One schema step.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// The version the store is at after this step. Versions start at 1 and
    /// have no gaps.
    pub version: u32,
    /// A short name for logs.
    pub name: &'static str,
    /// The SQL to run.
    pub sql: &'static str,
}

/// Every migration, in order.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("../migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "body cache",
        sql: include_str!("../migrations/0002_body_cache.sql"),
    },
    Migration {
        version: 3,
        name: "search queue",
        sql: include_str!("../migrations/0003_search_queue.sql"),
    },
    Migration {
        version: 4,
        name: "calendar",
        sql: include_str!("../migrations/0004_calendar.sql"),
    },
    Migration {
        version: 5,
        name: "ops source and audit log",
        sql: include_str!("../migrations/0005_ops_source.sql"),
    },
    Migration {
        version: 6,
        name: "contacts and correspondents",
        sql: include_str!("../migrations/0006_contacts.sql"),
    },
    Migration {
        version: 7,
        name: "snoozes",
        sql: include_str!("../migrations/0007_snoozes.sql"),
    },
    Migration {
        version: 8,
        name: "messages in many folders",
        sql: include_str!("../migrations/0008_message_folders.sql"),
    },
    Migration {
        version: 9,
        name: "links between events and their source email",
        sql: include_str!("../migrations/0009_event_mail.sql"),
    },
];

/// The schema version this build reads and writes.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

/// The store's current schema version.
pub(crate) fn version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Brings the store up to the last of `migrations`. Returns the versions
/// applied. Refuses a store newer than `migrations` knows.
pub(crate) fn migrate(conn: &mut Connection, migrations: &[Migration]) -> Result<Vec<u32>> {
    for (i, m) in migrations.iter().enumerate() {
        assert_eq!(
            m.version as usize,
            i + 1,
            "migration versions must start at 1 with no gaps"
        );
    }
    let target = migrations.len() as u32;
    let current = version(conn)?;
    if current > target {
        return Err(Error::SchemaTooNew {
            found: current,
            supported: target,
        });
    }
    let mut applied = Vec::new();
    for m in &migrations[current as usize..] {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(m.sql)?;
        // PRAGMA arguments can't be bound; the version is our own integer.
        tx.execute_batch(&format!("PRAGMA user_version = {}", m.version))?;
        tx.commit()?;
        applied.push(m.version);
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        c
    }

    fn tables(conn: &Connection) -> Vec<String> {
        let mut s = conn
            .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
            .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn fresh_store_gets_every_table() {
        let mut c = mem();
        assert_eq!(
            migrate(&mut c, MIGRATIONS).unwrap(),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9]
        );
        assert_eq!(version(&c).unwrap(), SCHEMA_VERSION);
        assert_eq!(
            tables(&c),
            [
                "accounts",
                "audit",
                "bodies",
                "calendars",
                "contact_addresses",
                "contacts",
                "correspondents",
                "event_mail",
                "events",
                "folder_threads",
                "folders",
                "message_folders",
                "messages",
                "ops",
                "search_queue",
                "snoozes",
                "sqlite_sequence",
                "sync_state",
                "threads"
            ]
        );
        let triggers: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'trigger'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(triggers, 15);
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        let mut c = mem();
        migrate(&mut c, MIGRATIONS).unwrap();
        assert!(migrate(&mut c, MIGRATIONS).unwrap().is_empty());
    }

    #[test]
    fn upgrades_step_by_step_and_keeps_data() {
        const V2: Migration = Migration {
            version: 2,
            name: "add account colour",
            sql: "ALTER TABLE accounts ADD COLUMN colour TEXT NOT NULL DEFAULT 'blue';",
        };
        let mut c = mem();
        migrate(&mut c, &MIGRATIONS[..1]).unwrap();
        c.execute(
            "INSERT INTO accounts (key, email, created_at) VALUES ('hotmail', 'a@b', 0)",
            [],
        )
        .unwrap();

        let both = [MIGRATIONS[0], V2];
        assert_eq!(migrate(&mut c, &both).unwrap(), vec![2]);
        let colour: String = c
            .query_row(
                "SELECT colour FROM accounts WHERE key = 'hotmail'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(colour, "blue");
    }

    #[test]
    fn version_8_puts_existing_messages_in_their_folders() {
        let mut c = mem();
        migrate(&mut c, &MIGRATIONS[..7]).unwrap();
        c.execute_batch(
            "INSERT INTO accounts (id, key, email, created_at) VALUES (1, 'hotmail', 'a@b', 0);
             INSERT INTO folders (id, account_id, remote_id, name) VALUES (1, 1, 'in', 'Inbox'),
                                                                          (2, 1, 'old', 'Old');
             INSERT INTO threads (id, account_id, conversation_id) VALUES (1, 1, 'c1');
             INSERT INTO messages (account_id, folder_id, thread_id, remote_id, received_at, is_read)
             VALUES (1, 1, 1, 'm1', 10, 0), (1, 1, 1, 'm2', 20, 1), (1, 2, 1, 'm3', 30, 0);",
        )
        .unwrap();
        assert_eq!(migrate(&mut c, &MIGRATIONS[..8]).unwrap(), vec![8]);
        let rows: i64 = c
            .query_row(
                "SELECT count(*) FROM message_folders mf JOIN messages m
                    ON m.id = mf.message_id AND m.folder_id = mf.folder_id",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 3);
        assert_eq!(crate::writer::derived_drift(&c).unwrap(), 0);
        // The new triggers keep it that way.
        c.execute_batch(
            "UPDATE messages SET folder_id = 2 WHERE remote_id = 'm1';
             UPDATE messages SET is_read = 1 WHERE remote_id = 'm3';
             DELETE FROM messages WHERE remote_id = 'm2';",
        )
        .unwrap();
        assert_eq!(crate::writer::derived_drift(&c).unwrap(), 0);
        let (total, unread): (i64, i64) = c
            .query_row("SELECT total, unread FROM folders WHERE id = 2", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((total, unread), (2, 1));
    }

    #[test]
    fn failed_migration_rolls_back_whole_step() {
        const BAD: Migration = Migration {
            version: 2,
            name: "bad",
            sql: "CREATE TABLE half (x); SELECT no_such_function();",
        };
        let mut c = mem();
        migrate(&mut c, &MIGRATIONS[..1]).unwrap();
        assert!(migrate(&mut c, &[MIGRATIONS[0], BAD]).is_err());
        assert_eq!(version(&c).unwrap(), 1);
        assert!(!tables(&c).contains(&"half".to_string()));
    }

    #[test]
    fn refuses_newer_store() {
        let mut c = mem();
        c.execute_batch("PRAGMA user_version = 99").unwrap();
        assert!(matches!(
            migrate(&mut c, MIGRATIONS),
            Err(Error::SchemaTooNew {
                found: 99,
                supported: SCHEMA_VERSION
            })
        ));
    }

    #[test]
    fn shipped_migrations_have_licence_header_and_no_transaction_control() {
        for m in MIGRATIONS {
            assert!(
                m.sql.starts_with("-- SPDX-License-Identifier:"),
                "{}",
                m.name
            );
            // Each step runs inside our transaction; its own BEGIN/COMMIT would break that.
            let upper = m.sql.to_uppercase();
            assert!(!upper.contains("BEGIN TRANSACTION"), "{}", m.name);
            assert!(!upper.contains("\nCOMMIT"), "{}", m.name);
        }
    }
}
