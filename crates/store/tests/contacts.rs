// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contacts rounds, correspondents kept by triggers (and the migration's
//! back-fill agreeing with them), and people search.

mod common;

use common::*;
use familiar_store::*;

fn person(addr: &str) -> Address {
    Address {
        name: String::new(),
        addr: addr.into(),
    }
}

#[test]
fn correspondents_contacts_and_search() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let inbox = folder(&mut w, a, "Inbox");
    let sent = folder(&mut w, a, "SentItems");
    let drafts = folder(&mut w, a, "Drafts");

    w.write(|tx| {
        // Two from Ann, one from me (not counted), one draft (not counted).
        tx.upsert_message(&msg(a, inbox, "m1", "c1", 100, false))?;
        tx.upsert_message(&msg(a, inbox, "m2", "c2", 300, false))?;
        let mut mine = msg(a, inbox, "m3", "c3", 200, false);
        mine.from = person("hotmail@example.com");
        tx.upsert_message(&mine)?;
        tx.upsert_message(&msg(a, drafts, "m4", "c4", 400, true))?;
        // Sent to Bob twice (To and Cc), to Cy once.
        let mut s1 = msg(a, sent, "s1", "c5", 500, true);
        s1.from = person("hotmail@example.com");
        s1.to = vec![Address {
            name: "Bob Builder".into(),
            addr: "Bob@Example.com".into(),
        }];
        s1.cc = vec![person("cy@example.org")];
        tx.upsert_message(&s1)?;
        let mut s2 = msg(a, sent, "s2", "c6", 600, true);
        s2.from = person("hotmail@example.com");
        s2.to = vec![person("bob@example.com")];
        tx.upsert_message(&s2)?;
        // Updating a message doesn't count it again.
        tx.upsert_message(&msg(a, inbox, "m1", "c1", 100, true))?;
        Ok(())
    })
    .unwrap();

    let r = Reader::open(dir.path()).unwrap();
    let all = r.people(&[a], "", 10).unwrap();
    let get = |addr: &str| all.iter().find(|p| p.addr == addr).cloned();
    let bob = get("bob@example.com").unwrap();
    assert_eq!(
        (bob.sent, bob.received, bob.name.as_str()),
        (2, 0, "Bob Builder")
    );
    let ann = get("ann@example.com").unwrap();
    assert_eq!((ann.sent, ann.received, ann.last_at), (0, 2, 300));
    assert_eq!(get("cy@example.org").unwrap().sent, 1);
    assert!(get("hotmail@example.com").is_none());
    assert_eq!(all.len(), 3);
    // Written to most first.
    assert_eq!(all[0].addr, "bob@example.com");

    // The migration's back-fill gives the same rows as the triggers.
    let before = rows(dir.path());
    drop(r);
    drop(w);
    let sql = include_str!("../migrations/0006_contacts.sql");
    let backfill = &sql[sql.find("-- Mail already stored.").unwrap()..];
    let c = rusqlite::Connection::open(dir.path().join("store.sqlite")).unwrap();
    c.execute_batch("DELETE FROM correspondents;").unwrap();
    c.execute_batch(backfill).unwrap();
    drop(c);
    let after = rows(dir.path());
    // The back-fill takes any one name; compare the rest.
    let strip = |v: Vec<(String, String, i64, i64, i64)>| {
        v.into_iter()
            .map(|(a, _, r, s, l)| (a, r, s, l))
            .collect::<Vec<_>>()
    };
    assert_eq!(strip(before), strip(after));

    // Contacts: a round replaces the account's contacts.
    let mut w = Writer::open(dir.path()).unwrap();
    let contact = |id: &str, name: &str, addrs: &[&str]| NewContact {
        account: a,
        remote_id: id.into(),
        display_name: name.into(),
        company: "Acme".into(),
        emails: addrs
            .iter()
            .map(|x| Address {
                name: name.into(),
                addr: (*x).into(),
            })
            .collect(),
        phones: vec!["021 555".into()],
    };
    w.write(|tx| {
        let round = tx.begin_contact_round(a)?;
        tx.upsert_contact(&contact("k1", "Bob Builder", &["bob@example.com"]), round)?;
        tx.upsert_contact(&contact("k2", "Dee Old", &["dee@example.com"]), round)?;
        tx.finish_contact_round(a, round)?;
        let round = tx.begin_contact_round(a)?;
        tx.upsert_contact(
            &contact(
                "k1",
                "Bob Builder",
                &["bob@example.com", "bob@home.example"],
            ),
            round,
        )?;
        assert_eq!(tx.finish_contact_round(a, round)?, 1);
        Ok(())
    })
    .unwrap();
    let r = Reader::open(dir.path()).unwrap();
    let cs = r.contacts(a).unwrap();
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].emails.len(), 2);
    let bobs = r.people(&[a], "bob", 10).unwrap();
    assert_eq!(bobs.len(), 2);
    assert!(bobs.iter().all(|p| p.contact.is_some()));
    // A contact who is also a correspondent keeps the counts.
    assert_eq!(bobs[0].addr, "bob@example.com");
    assert_eq!(bobs[0].sent, 2);
    assert_eq!(r.people(&[a], "acme", 10).unwrap().len(), 2);
    assert_eq!(r.people(&[a], "build", 10).unwrap().len(), 2);
    assert!(r.people(&[a], "dee", 10).unwrap().is_empty());
    assert_eq!(r.people(&[a], "", 1).unwrap().len(), 1);
}

fn rows(root: &std::path::Path) -> Vec<(String, String, i64, i64, i64)> {
    let c = rusqlite::Connection::open(root.join("store.sqlite")).unwrap();
    let mut s = c
        .prepare("SELECT addr, name, received, sent, last_at FROM correspondents ORDER BY addr")
        .unwrap();
    s.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}
