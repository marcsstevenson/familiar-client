// SPDX-License-Identifier: MIT OR Apache-2.0
//! Readers don't block the writer, and the writer doesn't block readers.

mod common;

use common::*;
use familiar_store::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Generous for a loaded CI runner, but far below the 5 s busy timeout, so a
/// blocked commit or read shows up as a failure rather than a slow pass.
const NOT_BLOCKED: Duration = Duration::from_secs(1);

#[test]
fn writer_commits_while_a_reader_holds_a_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let inbox = folder(&mut w, a, "Inbox");
    w.write(|tx| {
        tx.upsert_message(&msg(a, inbox, "m0", "c", 0, false))
            .map(|_| ())
    })
    .unwrap();

    let mut r = Reader::open(dir.path()).unwrap();
    r.snapshot(|r| {
        assert_eq!(r.folder(inbox)?.unwrap().total, 1);
        for i in 1..=20 {
            let t = Instant::now();
            w.write(|tx| {
                tx.upsert_message(&msg(a, inbox, &format!("m{i}"), "c", i, false))
                    .map(|_| ())
            })?;
            assert!(
                t.elapsed() < NOT_BLOCKED,
                "commit {i} took {:?}",
                t.elapsed()
            );
        }
        // Still the snapshot from before the writes.
        assert_eq!(r.folder(inbox)?.unwrap().total, 1);
        assert_eq!(r.messages(inbox, None, 100)?.len(), 1);
        Ok(())
    })
    .unwrap();
    // A new read sees every commit.
    assert_eq!(r.folder(inbox).unwrap().unwrap().total, 21);
}

#[test]
fn reader_reads_while_the_writer_holds_an_open_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let inbox = folder(&mut w, a, "Inbox");
    let root = dir.path().to_path_buf();
    w.write(|tx| {
        tx.upsert_message(&msg(a, inbox, "m1", "c", 1, false))?;
        // The transaction is open and uncommitted. A reader in another
        // thread reads straight away and sees the last committed state.
        let handle = std::thread::spawn(move || {
            let r = Reader::open(&root).unwrap();
            let t = Instant::now();
            let total = r.folder(inbox).unwrap().unwrap().total;
            (total, t.elapsed())
        });
        let (total, took) = handle.join().unwrap();
        assert_eq!(total, 0, "uncommitted write is invisible");
        assert!(took < NOT_BLOCKED, "read took {took:?}");
        Ok(())
    })
    .unwrap();
    assert_eq!(
        Reader::open(dir.path())
            .unwrap()
            .folder(inbox)
            .unwrap()
            .unwrap()
            .total,
        1
    );
}

#[test]
fn busy_readers_never_stall_a_sync_sized_write_load() {
    const READERS: usize = 4;
    const BATCHES: i64 = 60;
    const PER_BATCH: i64 = 50;

    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let inbox = folder(&mut w, a, "Inbox");
    let sent = folder(&mut w, a, "Sent");

    let stop = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicU64::new(0));
    let readers: Vec<_> = (0..READERS)
        .map(|n| {
            let (root, stop, reads) = (dir.path().to_path_buf(), stop.clone(), reads.clone());
            std::thread::spawn(move || -> Result<()> {
                let mut r = Reader::open(&root)?;
                while !stop.load(Ordering::Relaxed) {
                    // Long snapshots: page through the whole folder and check
                    // the counters agree with what the pages show.
                    r.snapshot(|r| {
                        let folder = if n % 2 == 0 { inbox } else { sent };
                        let f = r.folder(folder)?.unwrap();
                        let mut seen = 0;
                        let mut unread = 0;
                        let mut after = None;
                        loop {
                            let page = r.messages(folder, after, 100)?;
                            let Some(last) = page.last() else { break };
                            after = Some(Cursor {
                                received_at: last.received_at,
                                id: last.id.0,
                            });
                            seen += page.len() as i64;
                            unread += page.iter().filter(|m| !m.is_read).count() as i64;
                        }
                        assert_eq!(
                            (f.total, f.unread),
                            (seen, unread),
                            "counters match snapshot"
                        );
                        r.threads(folder, None, 50)?;
                        Ok(())
                    })?;
                    reads.fetch_add(1, Ordering::Relaxed);
                }
                Ok(())
            })
        })
        .collect();

    let mut worst = Duration::ZERO;
    let mut rng = Lcg(3);
    let start = Instant::now();
    for b in 0..BATCHES {
        let t = Instant::now();
        w.write(|tx| {
            for i in 0..PER_BATCH {
                let n = b * PER_BATCH + i;
                let f = if rng.below(4) == 0 { sent } else { inbox };
                let id = tx.upsert_message(&msg(
                    a,
                    f,
                    &format!("m{n}"),
                    &format!("c{}", n / 3),
                    n,
                    rng.below(2) == 0,
                ))?;
                if n % 10 == 0 {
                    tx.set_read(id, true)?;
                }
            }
            Ok(())
        })
        .unwrap();
        worst = worst.max(t.elapsed());
    }
    let elapsed = start.elapsed();
    stop.store(true, Ordering::Relaxed);
    for r in readers {
        r.join().unwrap().unwrap();
    }

    let total = (BATCHES * PER_BATCH) as f64;
    eprintln!(
        "wrote {total} messages in {elapsed:?} ({:.0}/s) with {READERS} readers; worst commit {worst:?}; {} reader snapshots",
        total / elapsed.as_secs_f64(),
        reads.load(Ordering::Relaxed)
    );
    assert!(worst < NOT_BLOCKED, "worst commit {worst:?}");
    assert!(reads.load(Ordering::Relaxed) > 0);
    assert_eq!(w.repair_derived().unwrap(), 0);
}
