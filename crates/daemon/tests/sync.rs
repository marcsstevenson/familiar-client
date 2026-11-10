// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 1.7 against the mock: first sync, polling, window ageing, pinning,
//! the body cache, the socket and `fam sync status`.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::Duration as Days;
use common::{Env, KEY, fast_policy};
use familiar_core::ErrorKind;
use familiar_core::daemon::{
    Client, FetchBodyParams, METHOD_FETCH_BODY, METHOD_STATUS, METHOD_STOP, METHOD_SYNC_NOW,
    PinParams, SyncPhase,
};
use familiar_daemon::policy::DAY;
use familiar_daemon::{Daemon, StartError};
use familiar_mock_graph::{EmailAddress, Message};
use familiar_store::Cursor;
use serde_json::{Value, json};

const LIMIT: Duration = Duration::from_secs(60);

async fn sync_now(env: &Env) {
    env.daemon()
        .service()
        .handle(METHOD_SYNC_NOW, json!({}))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_sync_then_polling_stays_consistent() {
    let mut env = Env::new(3000).await;
    env.start(fast_policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    assert_eq!(env.differences(30), Vec::<String>::new());

    // Bodies came newest first: everything in the window, nothing older.
    let r = env.reader();
    let stats = r.body_stats(env.account(), env.window_start(30)).unwrap();
    assert!(stats.with_body > 0 && stats.window_missing == 0);
    assert!(stats.with_body < stats.messages / 10, "{stats:?}");

    // Change the mailbox: new mail, moves, deletes, read states, an old
    // message filed into a user folder.
    let mbx = env.mock.mailbox(KEY);
    let inbox = mbx.message_ids("inbox");
    let newest = env.spec.newest;
    for i in 0..10 {
        mbx.add_message(Message::new(
            "inbox",
            format!("New {i}"),
            EmailAddress::new("Ann", "ann@example.com"),
            newest + Days::minutes(i),
        ))
        .unwrap();
    }
    for id in &inbox[..5] {
        mbx.move_message(id, "archive").unwrap();
    }
    for id in &inbox[5..10] {
        mbx.delete_message(id).unwrap();
    }
    let old = &inbox[inbox.len() - 5..];
    for id in old {
        mbx.update_message(id, |m| m.is_read = !m.is_read).unwrap();
    }
    mbx.move_message(&inbox[inbox.len() - 1], "fld-user-001")
        .unwrap();

    // The Inbox polls every 200 ms in this policy; the rest every 400 ms.
    env.until("changes to arrive", LIMIT, |e| {
        e.settled(30) && e.differences(30).is_empty()
    })
    .await;
    assert_eq!(env.daemon().writer().lock().repair_derived().unwrap(), 0);
    let live = env.daemon().service().status().accounts[0].clone();
    assert!(live.headers_synced >= 3000 && live.bodies_fetched > 0);
    assert_eq!(live.failures, 0);
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn window_ageing_pinning_and_the_cache() {
    let mut env = Env::new(2000).await;
    let mut policy = fast_policy();
    policy.cache_limit = 1; // Room for nothing: every cached body is evicted.
    env.start(policy.clone());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let acc = env.account();
    let svc = env.daemon().service().clone();

    // Pin a message from two years ago: its body is downloaded.
    let r = env.reader();
    let inbox = r
        .folders(acc)
        .unwrap()
        .into_iter()
        .find(|f| f.well_known.as_deref() == Some("inbox"))
        .unwrap();
    let cutoff = env.window_start(30);
    let old: Vec<_> = r
        .messages(
            inbox.id,
            Some(Cursor {
                received_at: cutoff - 365 * DAY,
                id: i64::MAX,
            }),
            3,
        )
        .unwrap();
    assert!(old.iter().all(|m| m.body.is_none()));
    let pin: Value = svc
        .handle(
            "mail.pin",
            serde_json::to_value(PinParams {
                message: old[0].id.0,
                pinned: true,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pin["fetched"], true);
    assert_eq!(pin["has_body"], true);

    // Open another old one: it goes in the cache, which holds nothing, so
    // it is evicted again straight away (the limit always holds).
    let f: Value = svc
        .handle(METHOD_FETCH_BODY, json!({ "message": old[1].id.0 }))
        .await
        .unwrap();
    assert_eq!(
        (f["fetched"].clone(), f["cached"].clone()),
        (json!(true), json!(true))
    );
    let m1 = r.message(old[1].id).unwrap().unwrap();
    assert!(m1.body.is_none(), "evicted at once with a 1-byte cache");

    // Move the clock 10 days on: bodies 30 to 40 days old leave the window
    // and are dropped at the next ageing; the pinned body stays.
    let in_window_before = r.body_stats(acc, env.window_start(30)).unwrap().with_body;
    env.now.fetch_add(10 * DAY, Ordering::SeqCst);
    env.until("ageing", LIMIT, |e| e.differences(30).is_empty())
        .await;
    let after = r.body_stats(acc, env.window_start(30)).unwrap();
    assert!(after.with_body < in_window_before, "{after:?}");
    assert_eq!(after.pinned, 1);
    assert!(
        r.message(old[0].id).unwrap().unwrap().body.is_some(),
        "pinned body kept"
    );

    // Unpin with room in the cache: the body stays, now as a cached body.
    drop(svc);
    env.stop().await;
    policy.cache_limit = 1 << 30;
    env.start(policy);
    let svc = env.daemon().service().clone();
    let unpin: Value = svc
        .handle(
            "mail.pin",
            serde_json::to_value(PinParams {
                message: old[0].id.0,
                pinned: false,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unpin["pinned"], false);
    let m0 = r.message(old[0].id).unwrap().unwrap();
    assert!(m0.body.is_some() && !m0.body_pinned && m0.body_cached_at.is_some());
    let missing = svc
        .handle(
            "mail.pin",
            json!({ "message": 999_999_999, "pinned": true }),
        )
        .await
        .unwrap_err();
    assert_eq!(missing.kind, ErrorKind::NotFound);
    drop(svc);
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn socket_status_and_sync_status_command() {
    let mut env = Env::new(500).await;
    env.start(fast_policy());

    // A second daemon on the same store is refused.
    match Daemon::start(env.options(fast_policy())) {
        Err(StartError::AlreadyRunning(_)) => {}
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("a second famd started"),
    }

    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let socket = env.paths.socket.clone();
    let paths = env.paths.clone();
    let now = env.now.load(Ordering::SeqCst);
    let (live, status) = tokio::task::spawn_blocking(move || {
        // The socket may take a moment to appear after start.
        let mut tries = 0;
        let mut c = loop {
            match Client::connect(&socket) {
                Ok(c) => break c,
                Err(_) if tries < 50 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => panic!("{e}"),
            }
        };
        let live: familiar_core::daemon::DaemonStatus = c.call(METHOD_STATUS, &json!({})).unwrap();
        // Two requests on one connection.
        let again: familiar_core::daemon::DaemonStatus = c.call(METHOD_STATUS, &json!({})).unwrap();
        assert_eq!(again.pid, live.pid);
        let bad = c.call::<_, Value>("nope", &json!({})).unwrap_err();
        assert_eq!(bad.kind, ErrorKind::Usage);
        let nf = c
            .call::<_, Value>(METHOD_FETCH_BODY, &FetchBodyParams { message: -1 })
            .unwrap_err();
        assert_eq!(nf.kind, ErrorKind::NotFound);
        let status = familiar_core::commands::sync::status_at(&paths, "30d", None, now).unwrap();
        (live, status)
    })
    .await
    .unwrap();
    assert_eq!(live.accounts.len(), 1);
    assert_eq!(live.accounts[0].phase, SyncPhase::Idle);
    assert!(status.daemon.running);
    let a = &status.accounts[0];
    assert_eq!(a.account, KEY);
    assert_eq!(a.phase, SyncPhase::Idle);
    assert!(a.first_sync_done);
    assert_eq!(a.messages, 500);
    assert_eq!(a.bodies_missing, 0);
    assert!(a.bodies > 0 && a.last_synced_at.is_some() && a.next_poll_at.is_some());

    // Stop through the socket; status then reads the store alone.
    let socket = env.paths.socket.clone();
    tokio::task::spawn_blocking(move || {
        let _: Value = Client::connect(&socket)
            .unwrap()
            .call(METHOD_STOP, &json!({}))
            .unwrap();
    })
    .await
    .unwrap();
    tokio::time::timeout(LIMIT, env.daemon().stopped())
        .await
        .unwrap();
    env.stop().await;
    let paths = env.paths.clone();
    let status = tokio::task::spawn_blocking(move || {
        familiar_core::commands::sync::status_at(&paths, "30d", Some(KEY), now).unwrap()
    })
    .await
    .unwrap();
    assert!(!status.daemon.running);
    assert_eq!(status.accounts[0].phase, SyncPhase::Stopped);
    assert_eq!(status.accounts[0].messages, 500);
    let paths = env.paths.clone();
    let err = tokio::task::spawn_blocking(move || {
        familiar_core::commands::sync::status_at(&paths, "30d", Some("nobody"), now).unwrap_err()
    })
    .await
    .unwrap();
    assert_eq!(err.kind, ErrorKind::NotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_resumes_incrementally() {
    let mut env = Env::new(1500).await;
    env.start(fast_policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    env.stop().await;
    env.mock.clear_requests();

    let mbx = env.mock.mailbox(KEY);
    let id = mbx.message_ids("inbox")[0].clone();
    mbx.move_message(&id, "archive").unwrap();
    env.start(fast_policy());
    env.until("the move", LIMIT, |e| {
        e.settled(30) && e.differences(30).is_empty()
    })
    .await;
    // No full round: every delta request carried a saved token.
    let full: Vec<_> = env
        .mock
        .requests()
        .into_iter()
        .filter(|r| r.path.ends_with("/messages/delta") && !r.query.contains("token"))
        .collect();
    assert!(full.is_empty(), "{} full rounds after restart", full.len());
    sync_now(&env).await;
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn calendars_sync_for_the_window_around_today() {
    let mut env = Env::new(200).await;
    let mut policy = fast_policy();
    policy.calendar_interval = Duration::from_millis(300);
    let now = env.now.load(std::sync::atomic::Ordering::SeqCst);
    let window = policy.calendar_window(now);
    let at = |days: i64| chrono::DateTime::from_timestamp(now + days * 86_400, 0).unwrap();
    env.mock
        .mailbox(KEY)
        .add_event(familiar_mock_graph::Event::new(
            "Tomorrow",
            at(1),
            at(1) + chrono::Duration::hours(1),
        ))
        .unwrap();
    // Two years ahead: outside the default one-year future.
    env.mock
        .mailbox(KEY)
        .add_event(familiar_mock_graph::Event::new(
            "Far off",
            at(730),
            at(730) + chrono::Duration::hours(1),
        ))
        .unwrap();
    env.start(policy);

    let subjects = |e: &Env| -> Vec<String> {
        let r = e.reader();
        let Some(cal) = r.calendars(e.account()).unwrap().into_iter().next() else {
            return Vec::new();
        };
        r.events_in(&[cal.id], familiar_store::EventRange::utc(0, i64::MAX / 2))
            .unwrap()
            .into_iter()
            .map(|ev| ev.data.subject)
            .collect()
    };
    env.until("calendar events", LIMIT, |e| {
        subjects(e).contains(&"Tomorrow".to_string())
    })
    .await;
    assert!(!subjects(&env).contains(&"Far off".to_string()));
    let cal = env.reader().calendars(env.account()).unwrap().remove(0);
    assert!(cal.is_default);
    assert_eq!(
        (cal.window_start, cal.window_end),
        (Some(window.0), Some(window.1))
    );

    // A new event shows up by delta on the next calendar poll.
    env.mock
        .mailbox(KEY)
        .add_event(familiar_mock_graph::Event::new(
            "Next week",
            at(7),
            at(7) + chrono::Duration::hours(2),
        ))
        .unwrap();
    sync_now(&env).await;
    env.until("the new event", LIMIT, |e| {
        subjects(e).contains(&"Next week".to_string())
    })
    .await;
    // Everything stored overlaps the window.
    let r = env.reader();
    let cal = r.calendars(env.account()).unwrap().remove(0);
    for ev in r
        .events_in(&[cal.id], familiar_store::EventRange::utc(0, i64::MAX / 2))
        .unwrap()
    {
        assert!(ev.data.start_at < window.1 && ev.data.end_at > window.0 - DAY);
    }
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_refusing_folder_does_not_hold_up_the_rest() {
    use familiar_mock_graph::Fault;
    let mut env = Env::new(1000).await;
    // One user folder refuses delta for good, as a folder Graph won't sync might.
    env.mock
        .push_fault(Fault::access_denied().on_path("fld-user-001/messages/delta"));
    env.start(fast_policy());
    env.until("first sync past the refusing folder", LIMIT, |e| {
        e.settled(30)
    })
    .await;

    // New Inbox mail still arrives on the next polls.
    let mbx = env.mock.mailbox(KEY);
    mbx.add_message(Message::new(
        "inbox",
        "After the refusal",
        EmailAddress::new("Ann", "ann@example.com"),
        env.spec.newest + Days::minutes(5),
    ))
    .unwrap();
    env.until("new mail in the Inbox", LIMIT, |e| {
        let r = e.reader();
        let inbox = r
            .folders(e.account())
            .unwrap()
            .into_iter()
            .find(|f| f.well_known.as_deref() == Some("inbox"))
            .unwrap();
        r.messages(inbox.id, None, 5)
            .unwrap()
            .iter()
            .any(|m| m.subject == "After the refusal")
    })
    .await;
    // The refusal is on record for that folder only.
    let r = env.reader();
    let st = r
        .sync_state(env.account(), "messages:fld-user-001")
        .unwrap()
        .unwrap();
    assert!(st.last_error.is_some_and(|e| e.contains("Access")));
    env.stop().await;
}
