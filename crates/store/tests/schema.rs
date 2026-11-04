// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening, migrating and the single-writer rule, on real files.

mod common;

use common::*;
use familiar_store::*;

#[test]
fn writer_creates_and_reopens_store() {
    let dir = tempfile::tempdir().unwrap();
    let a = {
        let mut w = Writer::open(dir.path()).unwrap();
        account(&mut w, "hotmail")
    };
    // Reopening runs no migrations and keeps the data.
    let mut w = Writer::open(dir.path()).unwrap();
    let accounts = w.write(|tx| tx.accounts()).unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].id, a);
    assert_eq!(accounts[0].provider, "graph");

    let r = Reader::open(dir.path()).unwrap();
    assert_eq!(r.accounts().unwrap(), accounts);
}

#[test]
fn reader_needs_a_store() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(Reader::open(dir.path()), Err(Error::NoStore(_))));
}

#[test]
fn reader_refuses_unmigrated_store() {
    let dir = tempfile::tempdir().unwrap();
    let paths = StorePaths::new(dir.path());
    rusqlite::Connection::open(&paths.db)
        .unwrap()
        .execute_batch("PRAGMA journal_mode = WAL; CREATE TABLE x (y);")
        .unwrap();
    assert!(matches!(
        Reader::open(dir.path()),
        Err(Error::SchemaTooOld { found: 0, .. })
    ));
}

#[test]
fn newer_store_is_refused_by_writer_and_reader() {
    let dir = tempfile::tempdir().unwrap();
    drop(Writer::open(dir.path()).unwrap());
    let paths = StorePaths::new(dir.path());
    rusqlite::Connection::open(&paths.db)
        .unwrap()
        .execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1))
        .unwrap();
    assert!(matches!(
        Writer::open(dir.path()),
        Err(Error::SchemaTooNew { .. })
    ));
    assert!(matches!(
        Reader::open(dir.path()),
        Err(Error::SchemaTooNew { .. })
    ));
}

#[test]
fn second_writer_is_refused_until_first_closes() {
    let dir = tempfile::tempdir().unwrap();
    let w = Writer::open(dir.path()).unwrap();
    assert!(matches!(
        Writer::open(dir.path()),
        Err(Error::WriterLocked(_))
    ));
    // Readers don't need the lock.
    Reader::open(dir.path()).unwrap();
    drop(w);
    Writer::open(dir.path()).unwrap();
}

const CHILD_ENV: &str = "FAMILIAR_STORE_LOCK_CHILD";

/// Run by `writer_lock_holds_across_processes` in a child process.
#[test]
fn lock_child() {
    let Ok(dir) = std::env::var(CHILD_ENV) else {
        return;
    };
    let code = match Writer::open(std::path::Path::new(&dir)) {
        Err(Error::WriterLocked(_)) => 3,
        Ok(_) => 0,
        Err(_) => 1,
    };
    std::process::exit(code);
}

#[test]
fn writer_lock_holds_across_processes() {
    let dir = tempfile::tempdir().unwrap();
    let child = || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lock_child", "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, dir.path())
            .status()
            .unwrap()
            .code()
    };
    let w = Writer::open(dir.path()).unwrap();
    assert_eq!(child(), Some(3), "child must see the lock");
    drop(w);
    assert_eq!(child(), Some(0), "lock is released on drop");
}

#[test]
fn reader_opens_after_writer_has_closed() {
    // The last connection to close removes the WAL; a read-only connection
    // must still open the store.
    let dir = tempfile::tempdir().unwrap();
    {
        let mut w = Writer::open(dir.path()).unwrap();
        account(&mut w, "hotmail");
        w.checkpoint().unwrap();
    }
    let r = Reader::open(dir.path()).unwrap();
    assert_eq!(r.accounts().unwrap().len(), 1);
}

#[test]
fn readers_cannot_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    account(&mut w, "hotmail");
    let paths = StorePaths::new(dir.path());
    // The same flags Reader uses.
    let c = rusqlite::Connection::open_with_flags(
        &paths.db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert!(c.execute("DELETE FROM accounts", []).is_err());
}

#[test]
fn deleting_an_account_removes_everything_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let b = account(&mut w, "parklands");
    let fa = folder(&mut w, a, "Inbox");
    let fb = folder(&mut w, b, "Inbox");
    w.write(|tx| {
        tx.upsert_message(&msg(a, fa, "m1", "c1", 10, false))?;
        tx.upsert_message(&msg(b, fb, "m2", "c1", 10, false))?;
        tx.set_sync_state(a, "folders", None, Some("delta-a"), 1)?;
        tx.enqueue_op(&NewOp {
            account: a,
            kind: "mark_read".into(),
            target: "m1".into(),
            ..Default::default()
        })?;
        tx.delete_account(a)
    })
    .unwrap();
    let r = Reader::open(dir.path()).unwrap();
    assert_eq!(r.accounts().unwrap().len(), 1);
    assert!(r.folders(a).unwrap().is_empty());
    assert!(r.sync_states(a).unwrap().is_empty());
    assert!(r.open_ops().unwrap().is_empty());
    assert!(r.message_by_remote(a, "m1").unwrap().is_none());
    assert_eq!(r.folder(fb).unwrap().unwrap().total, 1);
    assert_eq!(w.repair_derived().unwrap(), 0);
}
