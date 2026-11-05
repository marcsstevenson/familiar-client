// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 1.8: conversations assembled locally from `conversationId` across all
//! folders, in particular Inbox plus Sent Items.

mod common;

use common::*;
use familiar_store::*;
use std::collections::{BTreeMap, BTreeSet};

/// remote id -> (conversation, folder, received_at, read, Message-ID)
type Model = BTreeMap<String, (String, FolderId, i64, bool, String)>;
type Entry = (String, FolderId, i64, bool, String);

struct Mailbox {
    _dir: tempfile::TempDir,
    w: Writer,
    r: Reader,
    acc: AccountId,
    inbox: FolderId,
    sent: FolderId,
    archive: FolderId,
    deleted: FolderId,
}

fn addr(name: &str, addr: &str) -> Address {
    Address {
        name: name.into(),
        addr: addr.into(),
    }
}

fn me() -> Address {
    addr("Pat Example", "pat@example.com")
}

fn mailbox() -> Mailbox {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let acc = w
        .write(|tx| {
            tx.upsert_account(&NewAccount {
                key: "hotmail".into(),
                // Mixed case: from_me compares addresses case-insensitively.
                email: "Pat@Example.com".into(),
                ..Default::default()
            })
        })
        .unwrap();
    let mk = |w: &mut Writer, remote: &str, name: &str, wk: Option<&str>| {
        w.write(|tx| {
            tx.upsert_folder(&NewFolder {
                account: acc,
                remote_id: remote.into(),
                name: name.into(),
                well_known: wk.map(Into::into),
                ..Default::default()
            })
        })
        .unwrap()
    };
    let inbox = mk(&mut w, "fld-inbox", "Inbox", Some("inbox"));
    let sent = mk(&mut w, "fld-sent", "Sent Items", Some(SENT_ITEMS));
    let archive = mk(&mut w, "fld-club", "Cricket Club", None);
    let deleted = mk(&mut w, "fld-deleted", "Deleted Items", Some("deleteditems"));
    let r = Reader::open(dir.path()).unwrap();
    Mailbox {
        _dir: dir,
        w,
        r,
        acc,
        inbox,
        sent,
        archive,
        deleted,
    }
}

impl Mailbox {
    #[allow(clippy::too_many_arguments)]
    fn put(
        &mut self,
        folder: FolderId,
        remote: &str,
        conv: &str,
        subject: &str,
        from: Address,
        to: Vec<Address>,
        at: i64,
        read: bool,
    ) -> MessageId {
        let mut m = msg(self.acc, folder, remote, conv, at, read);
        m.subject = subject.into();
        m.from = from;
        m.to = to;
        self.w.write(|tx| tx.upsert_message(&m)).unwrap()
    }

    /// The classic split: Sam writes to the Inbox, Pat replies (Sent Items),
    /// Sam answers again (Inbox).
    fn practice(&mut self) -> [MessageId; 3] {
        let sam = addr("Sam Walker", "sam.walker@club.example");
        [
            self.put(
                self.inbox,
                "m1",
                "conv-practice",
                "Practice moved to Thursday",
                sam.clone(),
                vec![me()],
                1_000,
                true,
            ),
            self.put(
                self.sent,
                "m2",
                "conv-practice",
                "RE: Practice moved to Thursday",
                me(),
                vec![sam.clone()],
                2_000,
                true,
            ),
            self.put(
                self.inbox,
                "m3",
                "conv-practice",
                "RE: RE: Practice moved to Thursday",
                sam,
                vec![me(), addr("", "jordan@example.org")],
                3_000,
                false,
            ),
        ]
    }

    fn conv_of(&self, id: MessageId) -> Conversation {
        self.r.conversation_of(id).unwrap().expect("conversation")
    }
}

fn ids(c: &Conversation) -> Vec<MessageId> {
    c.messages.iter().map(|m| m.message.id).collect()
}

#[test]
fn inbox_and_sent_items_assemble_into_one_conversation() {
    let mut mb = mailbox();
    let [m1, m2, m3] = mb.practice();
    // A message in another conversation stays out.
    mb.put(
        mb.inbox,
        "other",
        "conv-other",
        "Committee",
        addr("Casey", "casey@club.example"),
        vec![me()],
        2_500,
        false,
    );

    // The same conversation whichever message it is opened from.
    let c = mb.conv_of(m1);
    assert_eq!(c, mb.conv_of(m2));
    assert_eq!(c, mb.conv_of(m3));
    let thread =
        mb.r.thread_by_conversation(mb.acc, "conv-practice")
            .unwrap()
            .unwrap();
    assert_eq!(c.thread, thread);
    assert_eq!(mb.r.conversation(thread).unwrap().unwrap(), c);

    assert_eq!(c.conversation_id, "conv-practice");
    assert_eq!(ids(&c), vec![m1, m2, m3], "oldest first, across folders");
    assert_eq!(c.subject, "Practice moved to Thursday");
    assert_eq!((c.message_count, c.unread_count), (3, 1));
    assert_eq!(c.last_received_at, 3_000);
    assert!(c.has_sent());

    let folders: Vec<_> = c
        .folders
        .iter()
        .map(|f| (f.name.as_str(), f.well_known.as_deref(), f.count))
        .collect();
    assert_eq!(
        folders,
        vec![
            ("Inbox", Some("inbox"), 2),
            ("Sent Items", Some(SENT_ITEMS), 1)
        ]
    );

    let shape: Vec<_> = c
        .messages
        .iter()
        .map(|m| (m.folder_name.as_str(), m.is_sent(), m.from_me, m.in_trash))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("Inbox", false, false, false),
            ("Sent Items", true, true, false),
            ("Inbox", false, false, false),
        ]
    );

    let people: Vec<_> = c
        .participants
        .iter()
        .map(|a| (a.name.as_str(), a.addr.as_str()))
        .collect();
    assert_eq!(
        people,
        vec![
            ("Sam Walker", "sam.walker@club.example"),
            ("Pat Example", "pat@example.com"),
            ("", "jordan@example.org"),
        ]
    );
}

#[test]
fn threaded_inbox_list_counts_the_whole_conversation() {
    let mut mb = mailbox();
    let [m1, m2, m3] = mb.practice();
    let other = mb.put(
        mb.inbox,
        "other",
        "conv-other",
        "FW: Committee",
        addr("Casey", "casey@club.example"),
        vec![me()],
        2_500,
        false,
    );

    let rows = mb.r.conversations(mb.inbox, None, 10).unwrap();
    assert_eq!(rows.len(), 2);
    let (p, o) = (&rows[0], &rows[1]);
    assert_eq!(p.subject, "Practice moved to Thursday");
    assert_eq!((p.last_received_at, p.last_message), (3_000, m3));
    assert_eq!((p.folder_message_count, p.folder_unread_count), (2, 1));
    assert_eq!((p.message_count, p.unread_count), (3, 1));
    assert!(p.has_sent);
    assert_eq!(p.latest.id, m3);
    assert_eq!(o.subject, "Committee");
    assert_eq!(o.last_message, other);
    assert!(!o.has_sent);

    // Sent Items lists the same conversation once, sorted by its own copy.
    let sent = mb.r.conversations(mb.sent, None, 10).unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].thread, p.thread);
    assert_eq!(
        (sent[0].last_message, sent[0].last_received_at),
        (m2, 2_000)
    );
    assert_eq!(sent[0].latest.id, m3, "newest anywhere is the Inbox reply");
    assert_eq!(sent[0].message_count, 3);

    // A newer reply sent from Sent Items becomes the Inbox row's latest
    // message without moving the row (it's sorted by the Inbox copy).
    let m4 = mb.put(
        mb.sent,
        "m4",
        "conv-practice",
        "RE: Practice moved to Thursday",
        me(),
        vec![],
        4_000,
        true,
    );
    let rows = mb.r.conversations(mb.inbox, None, 10).unwrap();
    assert_eq!(rows[0].last_received_at, 3_000);
    assert_eq!(rows[0].latest.id, m4);
    assert_eq!(rows[0].latest.folder, mb.sent);
    assert_eq!(rows[0].message_count, 4);
    assert_eq!(ids(&mb.conv_of(m1)), vec![m1, m2, m3, m4]);

    // Paging with the row cursor gives the same rows as one big page.
    let first = mb.r.conversations(mb.inbox, None, 1).unwrap();
    let second =
        mb.r.conversations(mb.inbox, Some(first[0].cursor()), 1)
            .unwrap();
    assert_eq!(vec![first[0].clone(), second[0].clone()], rows);
    assert!(
        mb.r.conversations(mb.inbox, Some(second[0].cursor()), 1)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn moves_keep_the_conversation_together() {
    let mut mb = mailbox();
    let [m1, m2, m3] = mb.practice();
    let before = mb.conv_of(m1);

    // Filing the whole Inbox side into a custom folder.
    mb.w.write(|tx| {
        tx.move_message(m1, mb.archive)?;
        tx.move_message(m3, mb.archive)
    })
    .unwrap();
    let c = mb.conv_of(m2);
    assert_eq!(c.thread, before.thread);
    assert_eq!(ids(&c), vec![m1, m2, m3]);
    assert_eq!(c.messages[0].folder_name, "Cricket Club");
    assert_eq!(
        c.folders.iter().map(|f| f.folder).collect::<Vec<_>>(),
        vec![mb.sent, mb.archive]
    );
    assert!(mb.r.conversations(mb.inbox, None, 10).unwrap().is_empty());
    assert_eq!(mb.r.conversations(mb.archive, None, 10).unwrap().len(), 1);

    // A move reported by delta arrives as an upsert with the new folder.
    let mut m = msg(mb.acc, mb.deleted, "m3", "conv-practice", 3_000, false);
    m.subject = "RE: Practice".into();
    mb.w.write(|tx| tx.upsert_message(&m)).unwrap();
    let c = mb.conv_of(m1);
    assert_eq!(ids(&c), vec![m1, m2, m3]);
    assert!(c.messages[2].in_trash);
    assert_eq!(
        c.without_trash().map(|m| m.message.id).collect::<Vec<_>>(),
        vec![m1, m2]
    );
}

#[test]
fn a_message_in_inbox_and_sent_items_is_shown_once() {
    let mut mb = mailbox();
    // Mail to yourself: one copy in Sent Items, one in the Inbox, same
    // Message-ID, different immutable IDs.
    let mut sent = msg(mb.acc, mb.sent, "self-sent", "conv-self", 100, true);
    sent.internet_message_id = Some("<note@example.com>".into());
    sent.from = me();
    let mut got = sent.clone();
    got.remote_id = "self-inbox".into();
    got.folder = mb.inbox;
    got.received_at = 101;
    got.is_read = false;
    got.has_attachments = true;
    let (s, g) =
        mb.w.write(|tx| Ok((tx.upsert_message(&sent)?, tx.upsert_message(&got)?)))
            .unwrap();

    let c = mb.conv_of(g);
    assert_eq!(c.message_count, 1);
    assert_eq!(c.unread_count, 1, "unread while any copy is unread");
    let only = &c.messages[0];
    assert_eq!(only.message.id, s, "the oldest row is shown");
    assert!(only.from_me && only.is_sent());
    assert!(!only.message.is_read);
    assert!(only.message.has_attachments);
    assert_eq!(only.copies.len(), 1);
    assert_eq!(only.copies[0].id, g);
    assert_eq!(only.copies[0].well_known.as_deref(), Some("inbox"));
    assert_eq!(c.folders.iter().map(|f| f.count).sum::<usize>(), 2);

    // A copy with a body beats one without; a copy in the trash loses.
    let body = mb.w.write(|tx| tx.put_body(b"hello")).unwrap();
    mb.w.write(|tx| {
        tx.set_body(
            g,
            Some(BodyRef {
                hash: body,
                is_html: false,
            }),
        )
    })
    .unwrap();
    assert_eq!(mb.conv_of(g).messages[0].message.id, g);
    mb.w.write(|tx| tx.move_message(g, mb.deleted)).unwrap();
    let c = mb.conv_of(g);
    assert_eq!(c.messages[0].message.id, s);
    assert!(!c.messages[0].in_trash);

    // Messages with no Message-ID are never merged.
    let mut a = msg(mb.acc, mb.inbox, "no-id-1", "conv-noid", 10, true);
    a.internet_message_id = None;
    let mut b = a.clone();
    b.remote_id = "no-id-2".into();
    let a = mb.w.write(|tx| tx.upsert_message(&a)).unwrap();
    mb.w.write(|tx| tx.upsert_message(&b)).unwrap();
    assert_eq!(mb.conv_of(a).message_count, 2);
}

#[test]
fn conversations_are_per_account_and_go_when_emptied() {
    let mut mb = mailbox();
    let [m1, m2, m3] = mb.practice();
    // Another account with the same conversation ID (they are only unique
    // per mailbox) stays separate.
    let other = account(&mut mb.w, "parklands");
    let other_inbox = folder(&mut mb.w, other, "Inbox");
    let x =
        mb.w.write(|tx| {
            tx.upsert_message(&msg(
                other,
                other_inbox,
                "m1",
                "conv-practice",
                1_500,
                false,
            ))
        })
        .unwrap();
    let theirs = mb.conv_of(x);
    assert_eq!(ids(&theirs), vec![x]);
    assert_eq!(theirs.account, other);
    assert_eq!(ids(&mb.conv_of(m1)), vec![m1, m2, m3]);

    let thread = mb.conv_of(m1).thread;
    mb.w.write(|tx| {
        tx.delete_message(m1)?;
        tx.delete_message(m3)
    })
    .unwrap();
    assert_eq!(ids(&mb.conv_of(m2)), vec![m2]);
    mb.w.write(|tx| tx.delete_message(m2)).unwrap();
    assert_eq!(mb.r.conversation(thread).unwrap(), None);
    assert_eq!(mb.r.conversation_of(m2).unwrap(), None);
    assert_eq!(
        mb.r.thread_by_conversation(mb.acc, "conv-practice")
            .unwrap(),
        None
    );
}

/// Random mail across folders and conversations, with moves, deletes,
/// copies and read flips: every conversation holds exactly the messages
/// sharing its conversation ID, and the threaded list of each folder agrees.
#[test]
fn random_mailbox_matches_a_model() {
    let mut mb = mailbox();
    let folders = [mb.inbox, mb.sent, mb.archive, mb.deleted];
    let mut rng = Lcg(18);
    let mut model: Model = BTreeMap::new();
    for step in 0..3_000u64 {
        let roll = rng.below(10);
        if roll < 6 || model.is_empty() {
            let remote = format!("r{step}");
            let conv = format!("c{}", rng.below(60));
            let f = folders[rng.below(4) as usize];
            // One in ten is a copy of an existing message in the same conversation.
            let imid = model
                .values()
                .find(|v| v.0 == conv && rng.below(10) == 0)
                .map(|v| v.4.clone())
                .unwrap_or_else(|| format!("<{remote}@example.com>"));
            let at = rng.below(1_000_000) as i64;
            let read = rng.below(2) == 0;
            let mut m = msg(mb.acc, f, &remote, &conv, at, read);
            m.internet_message_id = Some(imid.clone());
            mb.w.write(|tx| tx.upsert_message(&m)).unwrap();
            model.insert(remote, (conv, f, at, read, imid));
        } else {
            let pick = model
                .keys()
                .nth(rng.below(model.len() as u64) as usize)
                .unwrap()
                .clone();
            let id = mb.r.message_by_remote(mb.acc, &pick).unwrap().unwrap().id;
            let entry = model.get_mut(&pick).unwrap();
            match roll {
                6 | 7 => {
                    let f = folders[rng.below(4) as usize];
                    mb.w.write(|tx| tx.move_message(id, f)).unwrap();
                    entry.1 = f;
                }
                8 => {
                    entry.3 = !entry.3;
                    let read = entry.3;
                    mb.w.write(|tx| tx.set_read(id, read)).unwrap();
                }
                _ => {
                    mb.w.write(|tx| tx.delete_message(id)).unwrap();
                    model.remove(&pick);
                }
            }
        }
    }
    assert_eq!(mb.w.repair_derived().unwrap(), 0, "derived rows drift-free");

    let mut convs: BTreeMap<&str, Vec<(&String, &Entry)>> = BTreeMap::new();
    for (remote, v) in &model {
        convs.entry(v.0.as_str()).or_default().push((remote, v));
    }
    for (conv, members) in &convs {
        let thread = mb.r.thread_by_conversation(mb.acc, conv).unwrap().unwrap();
        let c = mb.r.conversation(thread).unwrap().unwrap();
        // Shown messages plus copies are exactly the members.
        let mut got: Vec<String> = Vec::new();
        for m in &c.messages {
            got.push(m.message.remote_id.clone());
            for copy in &m.copies {
                got.push(mb.r.message(copy.id).unwrap().unwrap().remote_id);
            }
        }
        got.sort();
        let want: Vec<String> = members.iter().map(|(r, _)| (*r).clone()).collect();
        assert_eq!(got, want, "conversation {conv}");
        // One shown message per Message-ID, unread if any copy is.
        let imids: BTreeSet<&str> = members.iter().map(|(_, v)| v.4.as_str()).collect();
        assert_eq!(c.message_count, imids.len());
        let unread: BTreeSet<&str> = members
            .iter()
            .filter(|(_, v)| !v.3)
            .map(|(_, v)| v.4.as_str())
            .collect();
        assert_eq!(c.unread_count, unread.len());
        // Oldest first.
        assert!(
            c.messages
                .windows(2)
                .all(|w| (w[0].message.received_at, w[0].message.id)
                    <= (w[1].message.received_at, w[1].message.id))
        );
        // Shown copy is outside the trash whenever one exists.
        for m in &c.messages {
            if m.in_trash {
                assert!(m.copies.iter().all(|c| c.folder == mb.deleted), "{conv}");
            }
        }
        // Folder counts.
        let mut per: BTreeMap<FolderId, usize> = BTreeMap::new();
        for (_, v) in members {
            *per.entry(v.1).or_default() += 1;
        }
        let got: BTreeMap<FolderId, usize> =
            c.folders.iter().map(|f| (f.folder, f.count)).collect();
        assert_eq!(got, per, "conversation {conv}");
    }

    // Each folder's threaded list has one row per conversation present in it.
    for f in folders {
        let mut rows = Vec::new();
        let mut after = None;
        loop {
            let page = mb.r.conversations(f, after, 7).unwrap();
            let Some(last) = page.last() else { break };
            after = Some(last.cursor());
            rows.extend(page);
        }
        let want: BTreeSet<&str> = model
            .values()
            .filter(|v| v.1 == f)
            .map(|v| v.0.as_str())
            .collect();
        assert_eq!(rows.len(), want.len());
        for row in &rows {
            let c = mb.r.conversation(row.thread).unwrap().unwrap();
            assert!(want.contains(c.conversation_id.as_str()));
            let in_folder: Vec<_> = model
                .values()
                .filter(|v| v.0 == c.conversation_id && v.1 == f)
                .collect();
            assert_eq!(row.folder_message_count as usize, in_folder.len());
            assert_eq!(
                row.last_received_at,
                in_folder.iter().map(|v| v.2).max().unwrap()
            );
            assert_eq!(
                row.message_count as usize,
                convs[c.conversation_id.as_str()].len()
            );
            assert_eq!(row.latest.received_at, c.last_received_at);
            assert_eq!(
                row.has_sent,
                convs[c.conversation_id.as_str()]
                    .iter()
                    .any(|(_, v)| v.1 == mb.sent)
            );
        }
        assert!(
            rows.windows(2).all(
                |w| (w[0].last_received_at, w[0].thread) > (w[1].last_received_at, w[1].thread)
            )
        );
    }
}
