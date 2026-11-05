// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 1.9 end to end: famd indexes what it syncs, `fam mail search` finds it
//! locally, and `--scope all` merges in server results for older message
//! text, labelled online, or says it couldn't.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::Duration as Span;
use common::{Env, KEY, fast_policy};
use familiar_core::commands::search::{
    HitSource, MailSearchInput, MailSearchOutput, OnlineStatus, SearchScope, search_at,
};
use familiar_core::daemon::DaemonPaths;
use familiar_daemon::{AccountSpec, Daemon, DaemonOptions};
use familiar_mock_graph::{EmailAddress, Message};
use familiar_provider_graph::{ClientOptions, GraphClient, StaticToken};

const LIMIT: Duration = Duration::from_secs(60);

fn input(q: &str, scope: SearchScope) -> MailSearchInput {
    MailSearchInput {
        query: q.into(),
        scope,
        limit: None,
        after: None,
    }
}

async fn search(paths: &DaemonPaths, now: i64, q: &str, scope: SearchScope) -> MailSearchOutput {
    let paths = paths.clone();
    let i = input(q, scope);
    tokio::task::spawn_blocking(move || search_at(&paths, &i, "30d", now).unwrap())
        .await
        .unwrap()
}

fn indexed(env: &Env) -> bool {
    env.paths.index.join("familiar-index-version").exists()
        && familiar_store::Reader::open(&env.paths.store)
            .is_ok_and(|r| r.search_queue_len().unwrap() == 0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_and_online_results() {
    let mut env = Env::new(300).await;
    let newest = env.spec.newest;
    let mut recent = Message::new(
        "inbox",
        "Quarterly figures",
        EmailAddress::new("Ann Smith", "ann@example.com"),
        newest - Span::days(2),
    );
    recent.body_html = Some("<p>The zanzibarwidget numbers are in.</p>".into());
    env.mock.mailbox(KEY).add_message(recent).unwrap();
    let mut old = Message::new(
        "inbox",
        "Old report",
        EmailAddress::new("Bob Jones", "bob@example.com"),
        newest - Span::days(400),
    );
    old.body_html = Some("<p>Archived zanzibarwidget totals.</p>".into());
    old.has_attachments = true;
    env.mock.mailbox(KEY).add_message(old).unwrap();

    env.start(fast_policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    env.until("index caught up", LIMIT, indexed).await;
    let now = env.now.load(Ordering::SeqCst);

    // Local only: the old message's body is outside the 30-day window.
    let local = search(&env.paths, now, "zanzibarwidget", SearchScope::Local).await;
    assert_eq!(local.hits.len(), 1, "{local:#?}");
    assert_eq!(local.hits[0].subject, "Quarterly figures");
    assert_eq!(local.hits[0].source, HitSource::Local);
    assert!(local.hits[0].body_local);
    assert_eq!(local.online, OnlineStatus::NotRequested);
    assert!(
        local.notices.iter().any(|n| n.contains("--scope all")),
        "{:?}",
        local.notices
    );

    // With the server: the old one is merged in, labelled online.
    let all = search(&env.paths, now, "zanzibarwidget", SearchScope::All).await;
    assert_eq!(all.online, OnlineStatus::Searched, "{:?}", all.notices);
    assert_eq!(all.hits.len(), 2, "{all:#?}");
    assert_eq!(all.online_hits, 1);
    assert_eq!(
        (all.hits[0].subject.as_str(), all.hits[0].source),
        ("Quarterly figures", HitSource::Local)
    );
    let o = &all.hits[1];
    assert_eq!(
        (o.subject.as_str(), o.source),
        ("Old report", HitSource::Online)
    );
    assert!(o.id.is_some(), "its header is local");
    assert!(!o.body_local);
    assert_eq!(o.folder.as_deref(), Some("Inbox"));
    assert!(o.has_attachments);

    // Local filters apply to server results too.
    for q in [
        "zanzibarwidget folder:sent",
        "zanzibarwidget -has:attachment",
        "zanzibarwidget after:60d",
        "zanzibarwidget account:nobody",
    ] {
        let r = search(&env.paths, now, q, SearchScope::All).await;
        assert_eq!(r.online_hits, 0, "{q}: {r:#?}");
    }
    let r = search(
        &env.paths,
        now,
        "zanzibarwidget before:60d has:attachment",
        SearchScope::All,
    )
    .await;
    assert_eq!(r.hits.len(), 1);
    assert_eq!(r.online_hits, 1);

    // A match the local index already has (by subject) isn't repeated as online.
    let r = search(&env.paths, now, "subject:\"old report\"", SearchScope::All).await;
    assert_eq!(r.hits.len(), 1, "{r:#?}");
    assert_eq!(r.hits[0].source, HitSource::Local);
    assert_eq!(r.online_hits, 0);

    // Filters only: the server has nothing to add.
    let r = search(&env.paths, now, "folder:inbox is:unread", SearchScope::All).await;
    assert_eq!(r.online, OnlineStatus::NotNeeded);

    // New mail is searchable a moment after it syncs.
    let mut fresh = Message::new(
        "inbox",
        "Fresh news",
        EmailAddress::new("Cat", "cat@example.com"),
        newest + Span::hours(1),
    );
    fresh.body_html = Some("quokkaflux".into());
    env.mock.mailbox(KEY).add_message(fresh).unwrap();
    let paths = env.paths.clone();
    env.until("new mail indexed", LIMIT, |_| {
        let i = input("quokkaflux", SearchScope::Local);
        search_at(&paths, &i, "30d", now).unwrap().hits.len() == 1
    })
    .await;

    // Without famd, local search still works and says the server wasn't searched.
    env.stop().await;
    let r = search(&env.paths, now, "zanzibarwidget", SearchScope::All).await;
    assert_eq!(r.online, OnlineStatus::DaemonUnavailable);
    assert_eq!(r.hits.len(), 1);
    assert!(
        r.notices.iter().any(|n| n.contains("wasn't searched")),
        "{:?}",
        r.notices
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_notice() {
    let env = Env::new(10).await;
    // A Graph that can't be reached: nothing listens on port 9.
    let client = GraphClient::new(
        ClientOptions::with_base("http://127.0.0.1:9/v1.0"),
        Arc::new(StaticToken("t".into())),
    )
    .unwrap();
    let mut o = DaemonOptions::new(
        env.paths.clone(),
        vec![AccountSpec {
            key: KEY.into(),
            email: "pat@example.com".into(),
            display_name: "Pat".into(),
            tenant_id: None,
            client,
        }],
    );
    o.policy = fast_policy();
    let d = Daemon::start(o).unwrap();
    env.until("index created", LIMIT, indexed).await;
    let now = env.now.load(Ordering::SeqCst);
    let paths = env.paths.clone();
    let r = tokio::task::spawn_blocking(move || {
        // The socket may take a moment to appear.
        for _ in 0..100 {
            let r = search_at(&paths, &input("anything", SearchScope::All), "30d", now).unwrap();
            if r.online != OnlineStatus::DaemonUnavailable {
                return r;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("famd never answered");
    })
    .await
    .unwrap();
    assert_eq!(r.online, OnlineStatus::Offline, "{r:#?}");
    assert!(
        r.notices
            .iter()
            .any(|n| n.starts_with("Offline:") && n.contains("wasn't searched")),
        "{:?}",
        r.notices
    );
    d.stop().await;
}
