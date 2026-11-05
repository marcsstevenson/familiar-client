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
        assert_eq!(migrate(&mut c, MIGRATIONS).unwrap(), vec![1, 2, 3, 4]);
        assert_eq!(version(&c).unwrap(), SCHEMA_VERSION);
        assert_eq!(
            tables(&c),
            [
                "accounts",
                "bodies",
                "calendars",
                "events",
                "folder_threads",
                "folders",
                "messages",
                "ops",
                "search_queue",
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
        assert_eq!(triggers, 6);
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
