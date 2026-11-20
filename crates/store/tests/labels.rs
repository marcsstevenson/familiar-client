// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages in more than one folder (Gmail labels, WP G.2).

mod common;

use common::*;
use familiar_store::*;

struct Gmail {
    a: AccountId,
    inbox: FolderId,
    sent: FolderId,
    trash: FolderId,
    acme: FolderId,
    family: FolderId,
    all: FolderId,
}

fn gmail(w: &mut Writer) -> Gmail {
    let a = account(w, "gmail");
    let inbox = folder(w, a, "Inbox");
    let sent = folder(w, a, "SentItems");
    let trash = folder(w, a, "DeletedItems");
    let label = |w: &mut Writer, name: &str| {
        w.write(|tx| {
            tx.upsert_folder(&NewFolder {
                account: a,
                remote_id: format!("Label_{name}"),
                name: name.into(),
                ..Default::default()
            })
        })
        .unwrap()
    };
    let acme = label(w, "Acme");
    let family = label(w, "Family");
    let all = w.write(|tx| tx.ensure_all_mail(a)).unwrap();
    Gmail {
        a,
        inbox,
        sent,
        trash,
        acme,
        family,
        all,
    }
}

fn counts(r: &Reader, f: FolderId) -> (i64, i64) {
    let f = r.folder(f).unwrap().unwrap();
    (f.total, f.unread)
}

fn ids(ms: &[Message]) -> Vec<&str> {
    ms.iter().map(|m| m.remote_id.as_str()).collect()
}

#[test]
fn one_message_in_several_folders_counts_and_lists_in_each() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let g = gmail(&mut w);
    let id = w
        .write(|tx| {
            let id = tx.upsert_message(&msg(g.a, g.inbox, "m1", "t1", 100, false))?;
            tx.set_message_folders(id, &[g.acme, g.inbox])?;
            Ok(id)
        })
        .unwrap();
    let r = Reader::open(dir.path()).unwrap();

    // Inbox is primary; All Mail is added; the given order is kept for the rest.
    assert_eq!(r.message(id).unwrap().unwrap().folder, g.inbox);
    assert_eq!(r.message_folders(id).unwrap(), [g.inbox, g.acme, g.all]);
    for f in [g.inbox, g.acme, g.all] {
        assert_eq!(counts(&r, f), (1, 1), "folder {f:?}");
        assert_eq!(ids(&r.messages(f, None, 10).unwrap()), ["m1"]);
        assert_eq!(r.threads(f, None, 10).unwrap().len(), 1);
        let c = r.conversations(f, None, 10).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].folder, f);
    }
    assert_eq!(counts(&r, g.family), (0, 0));

    // Reading it clears the unread count everywhere.
    w.write(|tx| tx.set_read(id, true)).unwrap();
    for f in [g.inbox, g.acme, g.all] {
        assert_eq!(counts(&r, f), (1, 0));
    }

    // A list over two of its folders shows it once.
    assert_eq!(r.message_order(&[g.inbox, g.acme], false).unwrap().len(), 1);
    assert_eq!(
        r.message_sort_text(&[g.inbox, g.acme], false, SortText::Subject)
            .unwrap()
            .len(),
        1
    );

    // Its conversation counts it in each folder but shows it once.
    let c = r.conversation_of(id).unwrap().unwrap();
    assert_eq!(c.messages.len(), 1);
    let per: Vec<_> = c.folders.iter().map(|f| (f.folder, f.count)).collect();
    assert_eq!(per, [(g.inbox, 1), (g.acme, 1), (g.all, 1)]);
    assert_eq!(w.repair_derived().unwrap(), 0);
}

#[test]
fn archive_relabel_trash_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let g = gmail(&mut w);
    let id = w
        .write(|tx| {
            let id = tx.upsert_message(&msg(g.a, g.inbox, "m1", "t1", 100, false))?;
            tx.set_message_folders(id, &[g.inbox, g.acme])?;
            Ok(id)
        })
        .unwrap();
    let r = Reader::open(dir.path()).unwrap();

    // Archive: Inbox comes off, the label becomes the primary location.
    w.write(|tx| tx.set_message_folders(id, &[g.acme])).unwrap();
    assert_eq!(r.message(id).unwrap().unwrap().folder, g.acme);
    assert_eq!(counts(&r, g.inbox), (0, 0));
    assert!(r.threads(g.inbox, None, 10).unwrap().is_empty());
    assert_eq!(counts(&r, g.acme), (1, 1));

    // No labels left: only All Mail, which then is the primary location.
    w.write(|tx| tx.set_message_folders(id, &[])).unwrap();
    assert_eq!(r.message_folders(id).unwrap(), [g.all]);
    assert_eq!(r.message(id).unwrap().unwrap().folder, g.all);
    assert_eq!(counts(&r, g.acme), (0, 0));

    // Trash takes it out of All Mail, as on Gmail, and is primary.
    w.write(|tx| tx.set_message_folders(id, &[g.family, g.trash]))
        .unwrap();
    assert_eq!(r.message(id).unwrap().unwrap().folder, g.trash);
    assert_eq!(r.message_folders(id).unwrap(), [g.trash, g.family]);
    assert_eq!(counts(&r, g.all), (0, 0));
    assert_eq!(w.repair_derived().unwrap(), 0);

    // A re-sync of the header (same primary folder) keeps the labels.
    w.write(|tx| {
        let mut m = msg(g.a, g.trash, "m1", "t1", 100, true);
        m.subject = "edited".into();
        tx.upsert_message(&m)
    })
    .unwrap();
    assert_eq!(r.message_folders(id).unwrap(), [g.trash, g.family]);
    assert_eq!(counts(&r, g.family), (1, 0));

    w.write(|tx| tx.delete_message(id)).unwrap();
    for f in [g.inbox, g.acme, g.family, g.trash, g.all] {
        assert_eq!(counts(&r, f), (0, 0));
        assert!(r.threads(f, None, 10).unwrap().is_empty());
    }
    assert!(r.message_folders(id).unwrap().is_empty());
    assert_eq!(w.repair_derived().unwrap(), 0);
}

#[test]
fn deleting_a_label_keeps_its_messages_in_their_other_folders() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let g = gmail(&mut w);
    let id = w
        .write(|tx| {
            let id = tx.upsert_message(&msg(g.a, g.inbox, "m1", "t1", 100, false))?;
            tx.set_message_folders(id, &[g.inbox, g.acme])?;
            Ok(id)
        })
        .unwrap();
    w.write(|tx| tx.delete_folder(g.acme)).unwrap();
    let r = Reader::open(dir.path()).unwrap();
    assert_eq!(r.message_folders(id).unwrap(), [g.inbox, g.all]);
    assert_eq!(counts(&r, g.inbox), (1, 1));
    assert_eq!(w.repair_derived().unwrap(), 0);
}

#[test]
fn folders_of_another_account_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let g = gmail(&mut w);
    let other = account(&mut w, "hotmail");
    let theirs = folder(&mut w, other, "Inbox");
    let id = w
        .write(|tx| tx.upsert_message(&msg(g.a, g.inbox, "m1", "t1", 100, false)))
        .unwrap();
    assert!(matches!(
        w.write(|tx| tx.set_message_folders(id, &[theirs])),
        Err(Error::NotFound(_))
    ));
    let r = Reader::open(dir.path()).unwrap();
    assert_eq!(r.message_folders(id).unwrap(), [g.inbox]);
}

#[test]
fn a_sent_label_marks_the_conversation_as_sent() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let g = gmail(&mut w);
    w.write(|tx| {
        let first = tx.upsert_message(&msg(g.a, g.inbox, "m1", "t1", 100, true))?;
        tx.set_message_folders(first, &[g.inbox])?;
        // Sent to yourself: in Inbox and Sent, primary Inbox.
        let reply = tx.upsert_message(&msg(g.a, g.inbox, "m2", "t1", 200, false))?;
        tx.set_message_folders(reply, &[g.sent, g.inbox])
    })
    .unwrap();
    let r = Reader::open(dir.path()).unwrap();
    let c = r.conversations(g.inbox, None, 10).unwrap();
    assert_eq!(c.len(), 1);
    assert!(c[0].has_sent);
    assert_eq!(
        (c[0].folder_message_count, c[0].folder_unread_count),
        (2, 1)
    );
    let c = r.conversations(g.sent, None, 10).unwrap();
    assert_eq!((c[0].folder_message_count, c[0].message_count), (1, 2));
}

#[test]
fn derived_rows_match_a_full_recount_after_random_label_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let g = gmail(&mut w);
    let labels = [g.inbox, g.sent, g.trash, g.acme, g.family];
    let mut rng = Lcg(11);
    let mut known: Vec<MessageId> = Vec::new();
    for round in 0..30 {
        w.write(|tx| {
            for i in 0..25 {
                let action = if known.is_empty() { 0 } else { rng.below(7) };
                match action {
                    0 | 1 => {
                        let f = labels[rng.below(labels.len() as u64) as usize];
                        let id = tx.upsert_message(&msg(
                            g.a,
                            f,
                            &format!("r{round}-{i}"),
                            &format!("c{}", rng.below(20)),
                            rng.below(1000) as i64,
                            rng.below(2) == 0,
                        ))?;
                        known.push(id);
                    }
                    2 | 3 => {
                        let id = known[rng.below(known.len() as u64) as usize];
                        let set: Vec<FolderId> = labels
                            .iter()
                            .copied()
                            .filter(|_| rng.below(3) == 0)
                            .collect();
                        tx.set_message_folders(id, &set)?;
                    }
                    4 => {
                        let id = known[rng.below(known.len() as u64) as usize];
                        tx.set_read(id, rng.below(2) == 0)?;
                    }
                    5 => {
                        // Re-sync with a new time and maybe a new thread.
                        let id = known[rng.below(known.len() as u64) as usize];
                        let m = tx.message(id)?.unwrap();
                        tx.upsert_message(&msg(
                            g.a,
                            m.folder,
                            &m.remote_id,
                            &format!("c{}", rng.below(20)),
                            rng.below(1000) as i64,
                            m.is_read,
                        ))?;
                    }
                    _ => {
                        let k = rng.below(known.len() as u64) as usize;
                        tx.delete_message(known.swap_remove(k))?;
                    }
                }
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(w.repair_derived().unwrap(), 0, "drift after round {round}");
    }
    // Every page of every folder lists each of its messages once.
    let r = Reader::open(dir.path()).unwrap();
    for f in labels.iter().chain([&g.all]) {
        let mut seen = Vec::new();
        let mut after = None;
        loop {
            let page = r.messages(*f, after, 7).unwrap();
            let Some(last) = page.last() else { break };
            after = Some(Cursor {
                received_at: last.received_at,
                id: last.id.0,
            });
            seen.extend(page.iter().map(|m| m.id));
        }
        assert_eq!(seen.len() as i64, r.folder(*f).unwrap().unwrap().total);
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), seen.len());
    }
}
