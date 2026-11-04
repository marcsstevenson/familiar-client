// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages, counters, threads, paging, bodies and sync state.

mod common;

use common::*;
use familiar_store::*;

#[test]
fn counters_and_threads_follow_inserts_moves_reads_and_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let inbox = folder(&mut w, a, "Inbox");
    let sent = folder(&mut w, a, "SentItems");
    let archive = folder(&mut w, a, "Archive");

    let (m1, m2, m3) = w
        .write(|tx| {
            Ok((
                tx.upsert_message(&msg(a, inbox, "m1", "conv-1", 100, false))?,
                tx.upsert_message(&msg(a, sent, "m2", "conv-1", 200, true))?,
                tx.upsert_message(&msg(a, inbox, "m3", "conv-1", 300, false))?,
            ))
        })
        .unwrap();

    let r = Reader::open(dir.path()).unwrap();
    let f = |id| r.folder(id).unwrap().unwrap();
    assert_eq!((f(inbox).total, f(inbox).unread), (2, 2));
    assert_eq!((f(sent).total, f(sent).unread), (1, 0));

    // One conversation across Inbox and Sent Items.
    let thread = r.message(m1).unwrap().unwrap().thread;
    let all: Vec<_> = r.thread(thread).unwrap().iter().map(|m| m.id).collect();
    assert_eq!(all, [m1, m2, m3]);
    let inbox_threads = r.threads(inbox, None, 10).unwrap();
    assert_eq!(inbox_threads.len(), 1);
    assert_eq!(inbox_threads[0].message_count, 2);
    assert_eq!(inbox_threads[0].unread_count, 2);
    assert_eq!(inbox_threads[0].last_message, m3);

    // Read, move by upsert (as delta sync reports it) and by move_message.
    w.write(|tx| {
        tx.set_read(m1, true)?;
        let mut moved = msg(a, archive, "m3", "conv-1", 300, false);
        moved.subject = "Renamed".into();
        tx.upsert_message(&moved)?;
        Ok(())
    })
    .unwrap();
    assert_eq!((f(inbox).total, f(inbox).unread), (1, 0));
    assert_eq!((f(archive).total, f(archive).unread), (1, 1));
    let t = r.threads(inbox, None, 10).unwrap();
    assert_eq!((t[0].last_message, t[0].message_count), (m1, 1));
    assert_eq!(r.message(m3).unwrap().unwrap().subject, "Renamed");

    w.write(|tx| tx.move_message(m1, archive)).unwrap();
    assert!(r.threads(inbox, None, 10).unwrap().is_empty());
    assert_eq!(f(inbox).total, 0);

    // Deleting every message removes the thread.
    w.write(|tx| {
        tx.delete_message(m1)?;
        tx.delete_message(m2)?;
        tx.delete_message(m3)
    })
    .unwrap();
    assert!(r.thread(thread).unwrap().is_empty());
    assert_eq!(f(archive).total, 0);
    assert_eq!(w.repair_derived().unwrap(), 0);
}

#[test]
fn derived_rows_match_a_full_recount_after_random_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let b = account(&mut w, "parklands");
    let folders: Vec<(AccountId, FolderId)> = [(a, "Inbox"), (a, "Sent"), (a, "Old"), (b, "Inbox")]
        .into_iter()
        .map(|(acc, name)| (acc, folder(&mut w, acc, name)))
        .collect();
    let mut rng = Lcg(7);
    let mut known: Vec<(AccountId, MessageId)> = Vec::new();
    for round in 0..40 {
        w.write(|tx| {
            for i in 0..25 {
                let action = if known.is_empty() { 0 } else { rng.below(6) };
                match action {
                    0 | 1 => {
                        let (acc, fo) = folders[rng.below(folders.len() as u64) as usize];
                        let conv = format!("c{}", rng.below(30));
                        let remote = format!("r{round}-{i}");
                        let at = rng.below(1000) as i64;
                        let id = tx.upsert_message(&msg(
                            acc,
                            fo,
                            &remote,
                            &conv,
                            at,
                            rng.below(2) == 0,
                        ))?;
                        known.push((acc, id));
                    }
                    2 => {
                        let (_, id) = known[rng.below(known.len() as u64) as usize];
                        tx.set_read(id, rng.below(2) == 0)?;
                    }
                    3 => {
                        let (acc, id) = known[rng.below(known.len() as u64) as usize];
                        let same: Vec<_> = folders.iter().filter(|f| f.0 == acc).collect();
                        tx.move_message(id, same[rng.below(same.len() as u64) as usize].1)?;
                    }
                    4 => {
                        // Re-sync: same message, maybe new conversation and time.
                        let (acc, id) = known[rng.below(known.len() as u64) as usize];
                        let m = tx.message(id)?.unwrap();
                        let mut n = msg(
                            acc,
                            m.folder,
                            &m.remote_id,
                            &format!("c{}", rng.below(30)),
                            rng.below(1000) as i64,
                            m.is_read,
                        );
                        n.is_flagged = true;
                        tx.upsert_message(&n)?;
                    }
                    _ => {
                        let k = rng.below(known.len() as u64) as usize;
                        let (_, id) = known.swap_remove(k);
                        tx.delete_message(id)?;
                    }
                }
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(w.repair_derived().unwrap(), 0, "drift after round {round}");
    }
    // repair_derived must notice drift when there is some.
    let paths = StorePaths::new(dir.path());
    drop(w);
    rusqlite::Connection::open(&paths.db)
        .unwrap()
        .execute_batch("UPDATE folders SET total = total + 5")
        .unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    assert_eq!(w.repair_derived().unwrap(), folders.len());
    assert_eq!(w.repair_derived().unwrap(), 0);
}

#[test]
fn keyset_pages_cover_a_folder_exactly_once_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let inbox = folder(&mut w, a, "Inbox");
    w.write(|tx| {
        for i in 0..237 {
            // Many equal timestamps, so the id tie-break matters.
            tx.upsert_message(&msg(
                a,
                inbox,
                &format!("m{i}"),
                &format!("c{}", i % 50),
                (i / 7) as i64,
                false,
            ))?;
        }
        Ok(())
    })
    .unwrap();
    let r = Reader::open(dir.path()).unwrap();

    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = r.messages(inbox, after, 20).unwrap();
        let Some(last) = page.last() else { break };
        after = Some(Cursor {
            received_at: last.received_at,
            id: last.id.0,
        });
        seen.extend(page.iter().map(|m| (m.received_at, m.id)));
    }
    assert_eq!(seen.len(), 237);
    assert!(
        seen.windows(2).all(|p| p[0] > p[1]),
        "strictly newest first"
    );

    let mut threads = Vec::new();
    let mut after = None;
    loop {
        let page = r.threads(inbox, after, 7).unwrap();
        let Some(last) = page.last() else { break };
        after = Some(Cursor {
            received_at: last.last_received_at,
            id: last.thread.0,
        });
        threads.extend(page.iter().map(|t| (t.last_received_at, t.thread)));
    }
    assert_eq!(threads.len(), 50);
    assert!(threads.windows(2).all(|p| p[0] > p[1]));
}

#[test]
fn bodies_are_deduplicated_inline_when_small_and_files_when_large() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let inbox = folder(&mut w, a, "Inbox");

    let small = b"<p>Hello</p>".to_vec();
    // Incompressible, so it is well over the inline limit after zstd.
    let mut rng = Lcg(1);
    let large: Vec<u8> = (0..3 * INLINE_BODY_MAX).map(|_| rng.next() as u8).collect();

    let (hs, hl, m1, m2) = w
        .write(|tx| {
            let hs = tx.put_body(&small)?;
            let hl = tx.put_body(&large)?;
            assert_eq!(tx.put_body(&small)?, hs, "same content, same hash");
            let mut n1 = msg(a, inbox, "m1", "c", 1, false);
            n1.body = Some(BodyRef {
                hash: hs,
                is_html: true,
            });
            let mut n2 = msg(a, inbox, "m2", "c", 2, false);
            n2.body = Some(BodyRef {
                hash: hl,
                is_html: false,
            });
            Ok((hs, hl, tx.upsert_message(&n1)?, tx.upsert_message(&n2)?))
        })
        .unwrap();

    let files = body_files(dir.path());
    assert_eq!(files.len(), 1, "only the large body has a file");
    assert!(files[0].ends_with(format!("{}.zst", hl.to_hex())));
    let hex = hl.to_hex();
    assert!(
        files[0].ends_with(
            std::path::Path::new(&hex[0..2])
                .join(&hex[2..4])
                .join(format!("{hex}.zst"))
        )
    );

    let r = Reader::open(dir.path()).unwrap();
    assert_eq!(r.body(&hs).unwrap(), small);
    assert_eq!(r.body(&hl).unwrap(), large);
    let m = r.message(m1).unwrap().unwrap();
    assert_eq!(
        m.body,
        Some(BodyRef {
            hash: hs,
            is_html: true
        })
    );

    // A header-only re-sync keeps the body.
    w.write(|tx| {
        tx.upsert_message(&msg(a, inbox, "m1", "c", 1, true))
            .map(|_| ())
    })
    .unwrap();
    assert_eq!(r.message(m1).unwrap().unwrap().body.unwrap().hash, hs);

    // A corrupt file is detected, not returned.
    std::fs::write(&files[0], b"not zstd").unwrap();
    assert!(matches!(r.body(&hl), Err(Error::CorruptBody { .. })));

    // Dropping a body from the window, then GC, frees the file and the row.
    w.write(|tx| tx.set_body(m2, None)).unwrap();
    let report = w.gc_bodies().unwrap();
    assert_eq!(report, GcReport { rows: 1, files: 1 });
    assert!(body_files(dir.path()).is_empty());
    assert!(matches!(r.body(&hl), Err(Error::NotFound(_))));
    assert_eq!(r.body(&hs).unwrap(), small, "still referenced");
}

#[test]
fn gc_removes_files_left_by_a_rolled_back_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let mut rng = Lcg(2);
    let large: Vec<u8> = (0..3 * INLINE_BODY_MAX).map(|_| rng.next() as u8).collect();
    let out: Result<()> = w.write(|tx| {
        tx.put_body(&large)?;
        Err(Error::NotFound("abort on purpose".into()))
    });
    assert!(out.is_err());
    assert_eq!(
        body_files(dir.path()).len(),
        1,
        "file written before the row"
    );
    // A stray temporary file from a crash mid-write.
    let tmp = dir.path().join("bodies").join("ab");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("x.zst.tmp"), b"partial").unwrap();

    assert_eq!(w.gc_bodies().unwrap(), GcReport { rows: 0, files: 2 });
    assert!(body_files(dir.path()).is_empty());

    // Storing it again after GC works and reads back.
    let h = w.write(|tx| tx.put_body(&large)).unwrap();
    assert_eq!(Reader::open(dir.path()).unwrap().body(&h).unwrap(), large);
}

fn body_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("bodies")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn sync_state_keeps_delta_links_errors_and_goes_with_its_folder() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let inbox = folder(&mut w, a, "Inbox");
    let res = "messages:folder-Inbox";
    w.write(|tx| {
        tx.set_sync_state(a, "folders", None, Some("https://graph/delta?t=1"), 10)?;
        tx.set_sync_state(a, res, Some(inbox), Some("https://graph/delta?t=2"), 11)?;
        tx.set_sync_error(a, res, "429 Too Many Requests")
    })
    .unwrap();
    let r = Reader::open(dir.path()).unwrap();
    let s = r.sync_state(a, res).unwrap().unwrap();
    assert_eq!(s.delta_link.as_deref(), Some("https://graph/delta?t=2"));
    assert_eq!(s.last_error.as_deref(), Some("429 Too Many Requests"));

    // Success clears the error; 410 Gone resets the link.
    w.write(|tx| tx.set_sync_state(a, res, Some(inbox), Some("https://graph/delta?t=3"), 12))
        .unwrap();
    assert_eq!(r.sync_state(a, res).unwrap().unwrap().last_error, None);
    w.write(|tx| tx.reset_sync_state(a, "folders")).unwrap();
    assert!(r.sync_state(a, "folders").unwrap().is_none());

    // Deleting the folder takes its messages and sync state with it.
    w.write(|tx| {
        tx.upsert_message(&msg(a, inbox, "m1", "c", 1, false))?;
        tx.delete_folder(inbox)
    })
    .unwrap();
    assert!(r.sync_states(a).unwrap().is_empty());
    assert!(r.message_by_remote(a, "m1").unwrap().is_none());
    assert_eq!(w.repair_derived().unwrap(), 0);
}

#[test]
fn moves_can_be_matched_by_internet_message_id() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let inbox = folder(&mut w, a, "Inbox");
    let id = w
        .write(|tx| tx.upsert_message(&msg(a, inbox, "m1", "c", 1, false)))
        .unwrap();
    let r = Reader::open(dir.path()).unwrap();
    let found = r.messages_by_internet_id("<m1@example.com>").unwrap();
    assert_eq!(found.iter().map(|m| m.id).collect::<Vec<_>>(), [id]);
}

#[test]
fn unknown_ids_are_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let out = w.write(|tx| tx.set_read(MessageId(999), true));
    assert!(matches!(out, Err(Error::NotFound(_))));
    let out = w.write(|tx| tx.delete_message(MessageId(999)));
    assert!(matches!(out, Err(Error::NotFound(_))));
}
