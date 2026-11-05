// SPDX-License-Identifier: MIT OR Apache-2.0
//! The index follows the store, and every operator finds what it should.

use familiar_search::{Error, Indexer, Opened, Query, Searcher};
use familiar_store::*;
use tempfile::TempDir;

const DAY: i64 = 86_400;
/// 2026-01-01.
const T0: i64 = 1_767_225_600;

struct Env {
    _dir: TempDir,
    w: Writer,
    ix: Indexer,
    index_dir: std::path::PathBuf,
    home: AccountId,
    club: AccountId,
    inbox: FolderId,
    sent: FolderId,
    committee: FolderId,
    club_inbox: FolderId,
}

fn folder(
    w: &mut Writer,
    account: AccountId,
    name: &str,
    wk: Option<&str>,
    parent: Option<FolderId>,
) -> FolderId {
    w.write(|tx| {
        tx.upsert_folder(&NewFolder {
            account,
            remote_id: format!("{}-{name}", account.0),
            parent,
            name: name.into(),
            well_known: wk.map(str::to_string),
        })
    })
    .unwrap()
}

fn addr(name: &str, a: &str) -> Address {
    Address {
        name: name.into(),
        addr: a.into(),
    }
}

impl Env {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let mut w = Writer::open(&dir.path().join("store")).unwrap();
        let acc = |w: &mut Writer, key: &str, email: &str| {
            w.write(|tx| {
                tx.upsert_account(&NewAccount {
                    key: key.into(),
                    email: email.into(),
                    ..Default::default()
                })
            })
            .unwrap()
        };
        let home = acc(&mut w, "hotmail", "marc@hotmail.example");
        let club = acc(&mut w, "parklands", "marc@parklands.example");
        let inbox = folder(&mut w, home, "Inbox", Some("inbox"), None);
        let sent = folder(&mut w, home, "Sent Items", Some("sentitems"), None);
        let club_inbox = folder(&mut w, club, "Inbox", Some("inbox"), None);
        let club_parent = folder(&mut w, club, "Club", None, None);
        let committee = folder(&mut w, club, "Committee", None, Some(club_parent));
        let index_dir = dir.path().join("search");
        let (ix, opened) = Indexer::open_for(&index_dir, &mut w).unwrap();
        assert_eq!(opened, Opened::Rebuilt);
        Env {
            _dir: dir,
            w,
            ix,
            index_dir,
            home,
            club,
            inbox,
            sent,
            committee,
            club_inbox,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        account: AccountId,
        folder: FolderId,
        remote: &str,
        subject: &str,
        from: Address,
        at: i64,
        body: Option<&str>,
        attach: bool,
    ) -> MessageId {
        self.w
            .write(|tx| {
                let body = body
                    .map(|b| -> Result<BodyRef> {
                        Ok(BodyRef {
                            hash: tx.put_body(b.as_bytes())?,
                            is_html: b.starts_with('<'),
                        })
                    })
                    .transpose()?;
                tx.upsert_message(&NewMessage {
                    account,
                    folder,
                    remote_id: remote.into(),
                    conversation_id: format!("conv-{remote}"),
                    subject: subject.into(),
                    from,
                    to: vec![addr("Marc Stevenson", "marc@hotmail.example")],
                    cc: vec![addr("Bea Treasurer", "bea@parklands.example")],
                    received_at: at,
                    has_attachments: attach,
                    importance: 1,
                    body,
                    ..Default::default()
                })
            })
            .unwrap()
    }

    fn index(&mut self) -> usize {
        self.ix.drain(&mut self.w).unwrap()
    }

    fn ids(&self, q: &str) -> Vec<String> {
        let s = Searcher::open(&self.index_dir).unwrap();
        let r = Reader::open(&self.w.paths().root).unwrap();
        let q = Query::parse(q, T0 + 400 * DAY).unwrap();
        s.search(&r, &q, None, 50)
            .unwrap()
            .hits
            .into_iter()
            .map(|m| m.remote_id)
            .collect()
    }

    fn mailbox(&mut self) {
        let club = addr("Parklands Cricket Club", "secretary@parklands.example");
        let ann = addr("Ann Smith", "ann.smith@example.com");
        let (home, clubacc) = (self.home, self.club);
        let (inbox, sent, committee, club_inbox) =
            (self.inbox, self.sent, self.committee, self.club_inbox);
        self.add(
            clubacc,
            club_inbox,
            "agm",
            "AGM notice",
            club.clone(),
            T0 + 10 * DAY,
            Some("<p>The <b>annual general meeting</b> is on Tuesday. Agenda attached.</p>"),
            true,
        );
        self.add(
            clubacc,
            committee,
            "minutes",
            "Committee minutes",
            club.clone(),
            T0 + 20 * DAY,
            Some("Minutes from the meeting. Café budget approved."),
            false,
        );
        self.add(
            home,
            inbox,
            "invoice-old",
            "Invoice 2014",
            ann.clone(),
            T0 - 12 * 365 * DAY,
            None,
            true,
        );
        self.add(
            home,
            inbox,
            "invoice-new",
            "Your invoice",
            ann.clone(),
            T0 + 30 * DAY,
            Some("Please pay the invoice by Friday. General enquiries welcome."),
            false,
        );
        self.add(
            home,
            sent,
            "reply",
            "Re: Your invoice",
            addr("Marc Stevenson", "marc@hotmail.example"),
            T0 + 31 * DAY,
            Some("Paid, thanks Ann."),
            false,
        );
        assert_eq!(self.index(), 5);
    }
}

#[test]
fn every_operator() {
    let mut e = Env::new();
    e.mailbox();

    assert_eq!(e.ids("invoice"), ["reply", "invoice-new", "invoice-old"]);
    // Body text, inside HTML.
    assert_eq!(e.ids("agenda"), ["agm"]);
    assert_eq!(e.ids("\"general meeting\""), ["agm"]);
    assert_eq!(e.ids("\"meeting general\""), Vec::<String>::new());
    // Accents fold both ways.
    assert_eq!(e.ids("cafe"), ["minutes"]);
    assert_eq!(e.ids("CAFÉ"), ["minutes"]);

    assert_eq!(e.ids("from:ann"), ["invoice-new", "invoice-old"]);
    assert_eq!(
        e.ids("from:ann.smith@example.com"),
        ["invoice-new", "invoice-old"]
    );
    assert_eq!(e.ids("from:\"Cricket Club\""), ["minutes", "agm"]);
    assert_eq!(e.ids("to:bea").len(), 5);
    assert_eq!(e.ids("to:nobody"), Vec::<String>::new());
    assert_eq!(
        e.ids("subject:invoice"),
        ["reply", "invoice-new", "invoice-old"]
    );
    assert_eq!(e.ids("subject:minutes"), ["minutes"]);
    assert_eq!(e.ids("body:invoice"), ["invoice-new"]);
    assert_eq!(e.ids("has:attachment"), ["agm", "invoice-old"]);
    assert_eq!(e.ids("-has:attachment invoice"), ["reply", "invoice-new"]);
    assert_eq!(e.ids("invoice before:2015-01-01"), ["invoice-old"]);
    assert_eq!(e.ids("invoice after:2026-01-31"), ["reply", "invoice-new"]);
    assert_eq!(
        e.ids("after:2026-01-12 before:2026-01-31"),
        ["minutes"],
        "after is inclusive of the day, before is not"
    );
    assert_eq!(
        e.ids("folder:inbox invoice"),
        ["invoice-new", "invoice-old"]
    );
    assert_eq!(e.ids("folder:sent"), ["reply"]);
    assert_eq!(e.ids("folder:\"Club/Committee\""), ["minutes"]);
    assert_eq!(e.ids("folder:committee"), ["minutes"]);
    assert_eq!(e.ids("folder:inbox account:parklands"), ["agm"]);
    assert_eq!(e.ids("account:marc@parklands.example"), ["minutes", "agm"]);
    assert_eq!(
        e.ids("-account:parklands -folder:sent"),
        ["invoice-new", "invoice-old"]
    );
    assert_eq!(e.ids("folder:nowhere"), Vec::<String>::new());
    assert_eq!(e.ids("invoice -paid"), ["invoice-new", "invoice-old"]);
    assert_eq!(e.ids("general -\"general meeting\""), ["invoice-new"]);
    // The design doc's example.
    assert_eq!(
        e.ids(r#"from:club subject:"AGM" has:attachment after:2026-01-01"#),
        ["agm"]
    );
}

#[test]
fn read_and_flag_state_come_from_the_store() {
    let mut e = Env::new();
    e.mailbox();
    assert_eq!(e.ids("is:unread").len(), 5);
    assert_eq!(e.ids("is:read"), Vec::<String>::new());
    let r = Reader::open(&e.w.paths().root).unwrap();
    let agm = r.message_by_remote(e.club, "agm").unwrap().unwrap().id;
    let reply = r.message_by_remote(e.home, "reply").unwrap().unwrap().id;
    e.w.write(|tx| {
        tx.set_read(agm, true)?;
        tx.set_flagged(reply, true)
    })
    .unwrap();
    // No re-index needed: read and flag state are not in the index.
    assert_eq!(r.search_queue_len().unwrap(), 0);
    assert_eq!(e.ids("is:read"), ["agm"]);
    assert_eq!(e.ids("is:unread folder:inbox").len(), 2);
    assert_eq!(e.ids("is:flagged"), ["reply"]);
    assert_eq!(e.ids("-is:flagged invoice"), ["invoice-new", "invoice-old"]);
}

#[test]
fn index_follows_moves_edits_ageing_and_deletes() {
    let mut e = Env::new();
    e.mailbox();
    let r = Reader::open(&e.w.paths().root).unwrap();
    let new = r.message_by_remote(e.home, "invoice-new").unwrap().unwrap();
    let (sent, inbox) = (e.sent, e.inbox);

    e.w.write(|tx| tx.move_message(new.id, sent)).unwrap();
    assert_eq!(e.ids("folder:sent"), ["reply"], "not yet indexed");
    assert_eq!(e.index(), 1);
    assert_eq!(e.ids("folder:sent"), ["reply", "invoice-new"]);
    assert_eq!(e.ids("folder:inbox account:hotmail"), ["invoice-old"]);

    // The body ages out of the window: header search still finds it, body
    // text no longer does.
    e.w.write(|tx| tx.set_body(new.id, None)).unwrap();
    e.index();
    assert_eq!(e.ids("friday"), Vec::<String>::new());
    assert_eq!(
        e.ids("subject:invoice folder:sent"),
        ["reply", "invoice-new"]
    );

    // A re-sync of an unchanged header doesn't queue anything.
    let mut m = familiar_store::NewMessage {
        account: e.home,
        folder: inbox,
        remote_id: "invoice-old".into(),
        conversation_id: "conv-invoice-old".into(),
        subject: "Invoice 2014".into(),
        from: addr("Ann Smith", "ann.smith@example.com"),
        to: vec![addr("Marc Stevenson", "marc@hotmail.example")],
        cc: vec![addr("Bea Treasurer", "bea@parklands.example")],
        received_at: T0 - 12 * 365 * DAY,
        has_attachments: true,
        importance: 1,
        ..Default::default()
    };
    e.w.write(|tx| tx.upsert_message(&m)).unwrap();
    assert_eq!(r.search_queue_len().unwrap(), 0);
    m.subject = "Invoice 2014 (paid)".into();
    e.w.write(|tx| tx.upsert_message(&m)).unwrap();
    assert_eq!(r.search_queue_len().unwrap(), 1);
    e.index();
    assert_eq!(e.ids("paid"), ["reply", "invoice-old"]);

    let old = r.message_by_remote(e.home, "invoice-old").unwrap().unwrap();
    e.w.write(|tx| tx.delete_message(old.id)).unwrap();
    e.index();
    assert_eq!(e.ids("invoice"), ["reply", "invoice-new"]);
}

#[test]
fn paging_is_exact_with_equal_times() {
    let mut e = Env::new();
    let (home, inbox) = (e.home, e.inbox);
    for i in 0..23 {
        // Groups of three share a second.
        e.add(
            home,
            inbox,
            &format!("m{i:02}"),
            "Weekly newsletter",
            addr("News", "news@example.com"),
            T0 + (i / 3) * 60,
            Some("weekly news"),
            false,
        );
    }
    e.index();
    let s = Searcher::open(&e.index_dir).unwrap();
    let r = Reader::open(&e.w.paths().root).unwrap();
    let q = Query::parse("newsletter", T0).unwrap();
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = s.search(&r, &q, cursor, 5).unwrap();
        assert!(page.hits.len() <= 5);
        seen.extend(page.hits.iter().map(|m| (m.received_at, m.id)));
        match page.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen.len(), 23);
    let mut sorted = seen.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    sorted.dedup();
    assert_eq!(seen, sorted, "newest first, no repeats");
}

#[test]
fn rebuild_on_version_change_and_missing_index() {
    let mut e = Env::new();
    e.mailbox();
    // Reopening a current index keeps it.
    let (_, opened) = Indexer::open_for(&e.index_dir, &mut e.w).unwrap();
    assert_eq!(opened, Opened::Existing);

    std::fs::write(e.index_dir.join("familiar-index-version"), "0").unwrap();
    assert!(matches!(
        Searcher::open(&e.index_dir),
        Err(Error::NotBuilt(_))
    ));
    let (mut ix, opened) = Indexer::open_for(&e.index_dir, &mut e.w).unwrap();
    assert_eq!(opened, Opened::Rebuilt);
    assert_eq!(ix.drain(&mut e.w).unwrap(), 5);
    assert_eq!(e.ids("invoice").len(), 3);

    std::fs::remove_dir_all(&e.index_dir).unwrap();
    assert!(matches!(
        Searcher::open(&e.index_dir),
        Err(Error::NotBuilt(_))
    ));
    let (mut ix, opened) = Indexer::open_for(&e.index_dir, &mut e.w).unwrap();
    assert_eq!(opened, Opened::Rebuilt);
    ix.drain(&mut e.w).unwrap();
    assert_eq!(e.ids("invoice").len(), 3);
}

#[test]
fn matching_tells_local_finds_from_server_only_finds() {
    let mut e = Env::new();
    e.mailbox();
    let s = Searcher::open(&e.index_dir).unwrap();
    let r = Reader::open(&e.w.paths().root).unwrap();
    let id = |remote: &str, acc| r.message_by_remote(acc, remote).unwrap().unwrap().id;
    let all = [
        id("invoice-old", e.home),
        id("invoice-new", e.home),
        id("agm", e.club),
    ];
    let q = Query::parse("invoice", T0).unwrap();
    let m = s.matching(&r, &q, &all).unwrap();
    assert_eq!(m.len(), 2);
    assert!(!m.contains(&all[2]));
}
