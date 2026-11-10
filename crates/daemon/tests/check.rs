// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 1.12 against the mock: `sync.check` is clean after a full sync and
//! while the mailbox changes, finds drift injected into the store, and
//! changes nothing.

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::Duration as Span;
use common::{Env, KEY, fast_policy};
use familiar_core::commands::sync::render_check;
use familiar_core::daemon::{
    CHECK_TIMEOUT, CheckParams, CheckReport, Client, DriftKind, METHOD_CHECK,
};
use familiar_daemon::policy::SyncPolicy;
use familiar_mock_graph::{EmailAddress, Message};
use familiar_provider_graph::GraphMessage;
use familiar_store::{BodyRef, Cursor, NewFolder};

const LIMIT: Duration = Duration::from_secs(60);

async fn check(env: &Env, p: CheckParams) -> CheckReport {
    env.daemon().service().check(p).await.unwrap()
}

fn kinds(r: &CheckReport) -> BTreeMap<DriftKind, usize> {
    r.accounts[0]
        .by_kind
        .iter()
        .map(|k| (k.kind, k.count))
        .collect()
}

async fn synced(messages: usize) -> Env {
    let mut env = Env::new(messages).await;
    // Ageing only at start: the drift test plants an old body that the
    // hourly ageing would otherwise tidy away before the check sees it.
    env.start(SyncPolicy {
        ageing_interval: Duration::from_secs(3600),
        ..fast_policy()
    });
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    env
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clean_after_a_full_sync() {
    let mut env = synced(3000).await;
    let r = check(&env, CheckParams::default()).await;
    let a = &r.accounts[0];
    assert!(r.clean, "{}", render_check(&r));
    assert!(a.first_sync_done && a.settled);
    assert_eq!(a.server_messages, 3000);
    assert_eq!(a.local_messages, 3000);
    assert_eq!(
        a.folders,
        env.reader().folders(env.account()).unwrap().len()
    );
    assert_eq!(a.drift_total, 0);

    // Over the socket, as `fam sync check` calls it.
    let mut c = Client::connect_with_timeout(&env.paths.socket, CHECK_TIMEOUT).unwrap();
    let r: CheckReport = c
        .call(
            METHOD_CHECK,
            &CheckParams {
                account: Some(KEY.into()),
                ..CheckParams::default()
            },
        )
        .unwrap();
    assert!(r.clean);
    let e = c
        .call::<_, CheckReport>(
            METHOD_CHECK,
            &CheckParams {
                account: Some("nobody".into()),
                ..CheckParams::default()
            },
        )
        .unwrap_err();
    assert_eq!(e.kind, familiar_core::ErrorKind::NotFound);
    env.stop().await;
}

/// Changes made on the server just before the check are still syncing when
/// it starts. Settling first and re-checking keep them out of the report.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changes_in_flight_are_not_drift() {
    let mut env = synced(2000).await;
    let mbx = env.mock.mailbox(KEY);
    let inbox = mbx.message_ids("inbox");
    let change = |round: i64| {
        for i in 0..5 {
            mbx.add_message(Message::new(
                "inbox",
                format!("Late {round}-{i}"),
                EmailAddress::new("Ann", "ann@example.com"),
                env.spec.newest + Span::seconds(round * 10 + i + 1),
            ))
            .unwrap();
        }
        let k = round as usize * 6;
        mbx.move_message(&inbox[k], "archive").unwrap();
        mbx.delete_message(&inbox[k + 1]).unwrap();
        mbx.update_message(&inbox[k + 2], |m| m.is_read = !m.is_read)
            .unwrap();
        mbx.update_message(&inbox[k + 3], |m| m.flagged = !m.flagged)
            .unwrap();
    };

    change(0);
    let r = check(&env, CheckParams::default()).await;
    assert!(r.clean, "{}", render_check(&r));

    // And again, with the engine mid-poll.
    change(1);
    let r = check(&env, CheckParams::default()).await;
    assert!(r.clean, "{}", render_check(&r));
    env.stop().await;
}

/// Drift the sync engine can't see (the store changed behind its back) is
/// found, by kind, and left as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finds_injected_drift_and_fixes_nothing() {
    let mut env = synced(2000).await;
    let acc = env.account();
    let r = env.reader();
    let folders = r.folders(acc).unwrap();
    let inbox = folders
        .iter()
        .find(|f| f.well_known.as_deref() == Some("inbox"))
        .unwrap()
        .clone();
    let archive = folders
        .iter()
        .find(|f| f.well_known.as_deref() == Some("archive"))
        .unwrap()
        .clone();
    let msgs = r.messages(inbox.id, None::<Cursor>, 5000).unwrap();
    // Newest first: the first few are inside the 30-day window.
    let (newest, oldest) = (&msgs[..4], &msgs[msgs.len() - 2..]);
    assert!(newest.iter().all(|m| m.body.is_some()));
    assert!(oldest.iter().all(|m| m.body.is_none()));

    env.daemon()
        .writer()
        .write(|tx| {
            tx.set_read(newest[0].id, !newest[0].is_read)?;
            tx.set_flagged(newest[1].id, !newest[1].is_flagged)?;
            tx.move_message(newest[2].id, archive.id)?;
            tx.set_body(newest[3].id, None)?;
            tx.delete_message(msgs[10].id)?;
            let hash = tx.put_body(b"<p>old</p>")?;
            tx.set_body(
                oldest[0].id,
                Some(BodyRef {
                    hash,
                    is_html: true,
                }),
            )?;
            let mut ghost = GraphMessage::default();
            ghost.id = "AAMk-ghost".into();
            ghost.received_date_time = Some("2001-01-01T00:00:00Z".into());
            tx.upsert_message(&ghost.to_new_message(acc, inbox.id))?;
            tx.upsert_folder(&NewFolder {
                account: acc,
                remote_id: "AAMk-ghost-folder".into(),
                parent: None,
                name: "Ghost".into(),
                well_known: None,
            })?;
            Ok(())
        })
        .unwrap();

    let mut before = env.differences(30);
    before.sort();
    let report = check(&env, CheckParams::default()).await;
    let a = &report.accounts[0];
    assert!(!report.clean);
    let want: BTreeMap<DriftKind, usize> = [
        (DriftKind::ReadState, 1),
        (DriftKind::Flag, 1),
        (DriftKind::WrongFolder, 1),
        (DriftKind::BodyMissing, 1),
        (DriftKind::MessageMissing, 1),
        (DriftKind::BodyOutsideWindow, 1),
        (DriftKind::MessageExtra, 1),
        (DriftKind::FolderExtra, 1),
    ]
    .into();
    assert_eq!(kinds(&report), want, "{}", render_check(&report));
    assert_eq!(a.drift_total, 8);
    let wrong = a
        .drift
        .iter()
        .find(|d| d.kind == DriftKind::WrongFolder)
        .unwrap();
    assert_eq!(wrong.message, Some(newest[2].id.0));
    assert_eq!(wrong.folder.as_deref(), Some(archive.name.as_str()));
    let missing = a
        .drift
        .iter()
        .find(|d| d.kind == DriftKind::MessageMissing)
        .unwrap();
    assert_eq!(
        missing.remote_id.as_deref(),
        Some(msgs[10].remote_id.as_str())
    );
    let text = render_check(&report);
    assert!(text.contains("DRIFT: 8 differences"), "{text}");

    // The limit trims the list, not the counts.
    let short = check(
        &env,
        CheckParams {
            limit: Some(3),
            ..CheckParams::default()
        },
    )
    .await;
    assert_eq!(short.accounts[0].drift.len(), 3);
    assert_eq!(short.accounts[0].drift_total, 8);

    // Nothing was repaired.
    let r = env.reader();
    assert_eq!(
        r.message(newest[0].id).unwrap().unwrap().is_read,
        !newest[0].is_read
    );
    assert!(r.message(msgs[10].id).unwrap().is_none());
    let mut after = env.differences(30);
    after.sort();
    assert_eq!(after, before);
    env.stop().await;
}

/// A folder counter out of step with its messages is store drift too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finds_counter_drift() {
    let mut env = synced(500).await;
    let db = env.reader().paths().db.clone();
    {
        // Behind the store's back, as a bug in a trigger would.
        let c = rusqlite::Connection::open(&db).unwrap();
        c.busy_timeout(Duration::from_secs(5)).unwrap();
        c.execute(
            "UPDATE folders SET total = total + 3 WHERE well_known = 'inbox'",
            [],
        )
        .unwrap();
    }
    let r = check(&env, CheckParams::default()).await;
    assert_eq!(
        kinds(&r),
        [(DriftKind::Counter, 1)].into(),
        "{}",
        render_check(&r)
    );
    env.stop().await;
}
