// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 2.5: Outlook contacts and past correspondents, searched by
//! `fam contacts search`, against the mock.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use common::{Env, KEY, fast_policy};
use familiar_core::commands::contacts::{ContactsSearchInput, search_at};
use familiar_daemon::policy::SyncPolicy;
use familiar_mock_graph::{Contact, EmailAddress, Message};

const LIMIT: Duration = Duration::from_secs(60);

fn policy() -> SyncPolicy {
    SyncPolicy {
        contacts_interval: Duration::from_millis(300),
        ..fast_policy()
    }
}

fn search(env: &Env, q: &str) -> Vec<familiar_core::commands::contacts::PersonView> {
    search_at(
        &env.paths,
        &ContactsSearchInput {
            query: q.into(),
            ..ContactsSearchInput::default()
        },
    )
    .unwrap()
    .people
}

async fn sync_now(env: &Env) {
    env.daemon()
        .service()
        .handle("sync.now", serde_json::json!({}))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn contacts_and_correspondents() {
    let mut env = Env::new(300).await;
    let mbox = env.mock.mailbox(KEY);
    let mut jane = Contact::new("Jane Coach", "Jane.Coach@club.example");
    jane.company_name = "Parklands Cricket".into();
    jane.email_addresses
        .push(EmailAddress::new("Jane (home)", "jane@home.example"));
    mbox.add_contact(jane);
    let mut gran = Contact::new("Gran", "gran@family.example");
    gran.folder_id = "fld-family".into();
    mbox.add_contact(gran);
    let gone = mbox.add_contact(Contact::new("Old Plumber", "plumber@example.net"));
    drop(mbox);

    env.start(policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    env.until("contacts", LIMIT, |e| {
        e.reader().contacts(e.account()).unwrap().len() == 3
    })
    .await;

    // By the start of a name, a word in it, an address or a company.
    let found = search(&env, "jan");
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.iter().all(|p| p.contact));
    assert!(found.iter().any(|p| p.address == "jane.coach@club.example"
        && p.mailbox == "Jane Coach <jane.coach@club.example>"
        && p.company == "Parklands Cricket"));
    assert_eq!(search(&env, "coach")[0].address, "jane.coach@club.example");
    assert_eq!(search(&env, "parklands").len(), 2);
    assert_eq!(search(&env, "gran@")[0].name, "Gran");
    assert!(search(&env, "zzz").is_empty());
    // LIKE wildcards are only letters here.
    assert!(search(&env, "%").is_empty());

    // Past correspondents, counted from the mailbox.
    let mbox = env.mock.mailbox(KEY);
    let mut received: HashMap<String, i64> = HashMap::new();
    let mut sent: HashMap<String, i64> = HashMap::new();
    let skip = ["sentitems", "drafts", "outbox"].map(|w| mbox.folder_id(w).unwrap());
    let sent_id = mbox.folder_id("sentitems").unwrap();
    let folders: Vec<String> = env
        .reader()
        .folders(env.account())
        .unwrap()
        .into_iter()
        .map(|f| f.remote_id)
        .collect();
    for f in folders {
        for id in mbox.message_ids(&f) {
            let m = mbox.message(&id).unwrap();
            if f == sent_id {
                for a in m.to.iter().chain(&m.cc) {
                    *sent.entry(a.address.to_lowercase()).or_default() += 1;
                }
            } else if !skip.contains(&f) {
                *received.entry(m.from.address.to_lowercase()).or_default() += 1;
            }
        }
    }
    drop(mbox);
    let top = search(&env, "");
    assert!(!top.is_empty());
    for p in top.iter().filter(|p| !p.contact) {
        assert_eq!(p.sent, sent.get(&p.address).copied().unwrap_or(0), "{p:?}");
        assert_eq!(
            p.received,
            received.get(&p.address).copied().unwrap_or(0),
            "{p:?}"
        );
    }
    let most = sent.values().max().copied().unwrap_or(0);
    assert!(most > 0);
    assert_eq!(top.iter().find(|p| !p.contact).unwrap().sent, most);

    // New mail from someone new; a contact deleted in Outlook.
    let mut m = Message::new(
        "inbox",
        "Hello",
        EmailAddress::new("Nora Newbie", "nora@newbie.example"),
        chrono::DateTime::from_timestamp(env.now.load(std::sync::atomic::Ordering::SeqCst) - 60, 0)
            .unwrap(),
    );
    m.body_html = Some("<p>hi</p>".into());
    env.mock.mailbox(KEY).add_message(m).unwrap();
    env.mock.mailbox(KEY).remove_contact(&gone);
    sync_now(&env).await;
    env.until("the newcomer and the removal", LIMIT, |e| {
        let nora = search(e, "nora");
        nora.len() == 1 && nora[0].received == 1 && search(e, "plumber").is_empty()
    })
    .await;
    assert_eq!(search(&env, "newbie")[0].name, "Nora Newbie");
    env.stop().await;
}
