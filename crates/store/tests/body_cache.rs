// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 1.7 body queries: what's missing in the window, ageing out, the
//! on-demand cache and header pruning.

mod common;

use common::*;
use familiar_store::*;

const DAY: i64 = 86_400;

fn setup() -> (tempfile::TempDir, Writer, AccountId, Vec<MessageId>) {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let f = folder(&mut w, a, "Inbox");
    // Ten messages, one a day, newest is day 10.
    let ids = w
        .write(|tx| {
            (1..=10)
                .map(|d| tx.upsert_message(&msg(a, f, &format!("m{d}"), "c", d * DAY, true)))
                .collect::<Result<Vec<_>>>()
        })
        .unwrap();
    (dir, w, a, ids)
}

fn give_body(w: &mut Writer, id: MessageId, text: &str) {
    w.write(|tx| {
        let hash = tx.put_body(text.as_bytes())?;
        tx.set_body(
            id,
            Some(BodyRef {
                hash,
                is_html: false,
            }),
        )
    })
    .unwrap();
}

#[test]
fn wanted_bodies_are_newest_first_and_paged() {
    let (dir, mut w, a, ids) = setup();
    give_body(&mut w, ids[9], "newest");
    let r = Reader::open(dir.path()).unwrap();
    // Window starts at day 5: days 5..=9 still need bodies.
    let got = r.bodies_wanted(a, 5 * DAY, None, 3).unwrap();
    assert_eq!(got, vec![ids[8], ids[7], ids[6]]);
    let last = r.message(ids[6]).unwrap().unwrap();
    let cursor = Cursor {
        received_at: last.received_at,
        id: last.id.0,
    };
    let got = r.bodies_wanted(a, 5 * DAY, Some(cursor), 3).unwrap();
    assert_eq!(got, vec![ids[5], ids[4]]);
    let s = r.body_stats(a, 5 * DAY).unwrap();
    assert_eq!((s.messages, s.with_body, s.window_missing), (10, 1, 5));
}

#[test]
fn ageing_keeps_pinned_and_cached_bodies() {
    let (dir, mut w, a, ids) = setup();
    for (i, id) in ids.iter().enumerate() {
        give_body(&mut w, *id, &format!("body {i}"));
    }
    w.write(|tx| {
        tx.set_body_pinned(ids[0], true)?;
        tx.mark_body_cached(ids[1], Some(100))
    })
    .unwrap();
    // Window now starts at day 6: days 1..=5 age out, except pinned and cached.
    let n = w.write(|tx| tx.age_out_bodies(a, 6 * DAY)).unwrap();
    assert_eq!(n, 3);
    let r = Reader::open(dir.path()).unwrap();
    let has = |i: usize| r.message(ids[i]).unwrap().unwrap().body.is_some();
    assert!(has(0) && has(1) && !has(2) && !has(4) && has(5) && has(9));
    let s = r.body_stats(a, 6 * DAY).unwrap();
    assert_eq!((s.with_body, s.pinned, s.cached), (7, 1, 1));
    assert!(s.cached_bytes > 0);
    // The freed bodies go at the next gc.
    assert_eq!(w.gc_bodies().unwrap().rows, 3);
}

#[test]
fn cache_evicts_oldest_first_down_to_the_limit() {
    let (dir, mut w, _a, ids) = setup();
    for (i, id) in ids.iter().take(4).enumerate() {
        give_body(&mut w, *id, &format!("cached body number {i}"));
        w.write(|tx| tx.mark_body_cached(*id, Some(1000 + i as i64)))
            .unwrap();
    }
    let r = Reader::open(dir.path()).unwrap();
    let one = r
        .message(ids[0])
        .unwrap()
        .unwrap()
        .body
        .map(|_| ())
        .is_some();
    assert!(one);
    let before = w.write(|tx| tx.evict_cached_bodies(i64::MAX)).unwrap();
    assert_eq!(before.evicted, 0);
    let each = before.before_bytes / 4;
    // Room for two: the two cached first go.
    let rep = w.write(|tx| tx.evict_cached_bodies(each * 2)).unwrap();
    assert_eq!(rep.evicted, 2);
    let has = |i: usize| r.message(ids[i]).unwrap().unwrap().body.is_some();
    assert!(!has(0) && !has(1) && has(2) && has(3));
    assert_eq!(r.message(ids[0]).unwrap().unwrap().body_cached_at, None);
    // Setting a body takes it out of the cache.
    give_body(&mut w, ids[3], "window body");
    assert_eq!(r.message(ids[3]).unwrap().unwrap().body_cached_at, None);
}

#[test]
fn pruning_headers_keeps_pinned_and_counters() {
    let (dir, mut w, a, ids) = setup();
    w.write(|tx| tx.set_body_pinned(ids[1], true)).unwrap();
    assert_eq!(w.write(|tx| tx.prune_headers(a, 4 * DAY)).unwrap(), 2);
    let r = Reader::open(dir.path()).unwrap();
    assert!(r.message(ids[0]).unwrap().is_none());
    assert!(r.message(ids[1]).unwrap().is_some());
    assert_eq!(r.folders(a).unwrap()[0].total, 8);
    assert_eq!(w.repair_derived().unwrap(), 0);
}

#[test]
fn shared_writer_is_one_writer() {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedWriter::new(Writer::open(dir.path()).unwrap());
    let a = shared
        .write(|tx| {
            tx.upsert_account(&NewAccount {
                key: "k".into(),
                email: "k@example.com".into(),
                ..Default::default()
            })
        })
        .unwrap();
    let mut other = shared.clone();
    let n = StoreWriter::write(&mut other, |tx| Ok(tx.accounts()?.len())).unwrap();
    assert_eq!(n, 1);
    assert_eq!(StoreWriter::paths(&other).root, dir.path());
    assert!(matches!(
        Writer::open(dir.path()),
        Err(Error::WriterLocked(_))
    ));
    drop(shared);
    drop(other);
    assert!(Writer::open(dir.path()).is_ok());
    let _ = a;
}
