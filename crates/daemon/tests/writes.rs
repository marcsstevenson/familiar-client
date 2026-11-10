// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 2.1 and 2.2: local-first writes through the durable op queue.
//!
//! Writes change the store at once, reach the mock through famd's op
//! runner, survive famd stopping (and ops left in flight by a kill), are
//! retried through passing failures and undone when the server refuses
//! them for good. Afterwards the store and the mock agree.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{Env, KEY, fast_policy};
use familiar_core::ErrorKind;
use familiar_core::commands::write::{OpsListInput, WriteFlags, ops_list_at, submit_at};
use familiar_core::ops::{WriteOutput, WriteRequest};
use familiar_daemon::policy::SyncPolicy;
use familiar_mock_graph::Fault;
use familiar_store::{LOCAL_REMOTE_PREFIX, Message, OpState, Writer};

const LIMIT: Duration = Duration::from_secs(60);

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

fn policy() -> SyncPolicy {
    SyncPolicy {
        op_retry_min: Duration::from_millis(100),
        op_retry_max: Duration::from_millis(500),
        ..fast_policy()
    }
}

fn waiting() -> WriteFlags {
    WriteFlags {
        wait: true,
        wait_secs: Some(30),
        ..WriteFlags::default()
    }
}

async fn submit(
    env: &Env,
    req: WriteRequest,
    flags: WriteFlags,
) -> Result<WriteOutput, familiar_core::CommandError> {
    let p = env.paths.clone();
    blocking(move || submit_at(&p, req, &flags)).await
}

fn inbox(env: &Env, n: usize) -> Vec<Message> {
    let r = env.reader();
    let acc = env.account();
    let f = r
        .folders(acc)
        .unwrap()
        .into_iter()
        .find(|f| f.well_known.as_deref() == Some("inbox"))
        .unwrap();
    r.messages(f.id, None, n).unwrap()
}

fn folder_named(env: &Env, name: &str) -> Option<familiar_store::Folder> {
    let r = env.reader();
    r.folders(env.account())
        .unwrap()
        .into_iter()
        .find(|f| f.name == name)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writes_reach_the_server_and_settle() {
    let mut env = Env::new(200).await;
    env.start(policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let msgs = inbox(&env, 4);
    let ids: Vec<i64> = msgs.iter().map(|m| m.id.0).collect();
    let mbox = || env.mock.mailbox(KEY);

    // Dry run: nothing changes.
    let dry = submit(
        &env,
        WriteRequest::MarkRead {
            messages: vec![ids[0]],
            read: !msgs[0].is_read,
        },
        WriteFlags {
            dry_run: true,
            ..WriteFlags::default()
        },
    )
    .await
    .unwrap();
    assert!(dry.dry_run);
    assert_eq!(dry.ops.len(), 1);
    assert_eq!(dry.ops[0].state, "planned");
    assert_eq!(dry.ops[0].id, None);
    assert_eq!(
        env.reader().message(msgs[0].id).unwrap().unwrap().is_read,
        msgs[0].is_read
    );

    // Read state and flag, waiting for the server.
    let out = submit(
        &env,
        WriteRequest::MarkRead {
            messages: vec![ids[0]],
            read: !msgs[0].is_read,
        },
        waiting(),
    )
    .await
    .unwrap();
    assert!(out.waited);
    assert_eq!(out.ops[0].state, "done", "{out:?}");
    assert_eq!(
        mbox().message(&msgs[0].remote_id).unwrap().is_read,
        !msgs[0].is_read
    );

    submit(
        &env,
        WriteRequest::Flag {
            messages: vec![ids[1]],
            flagged: true,
        },
        waiting(),
    )
    .await
    .unwrap();
    assert!(mbox().message(&msgs[1].remote_id).unwrap().flagged);

    // Create a folder and move a message into it straight away, before the
    // server has the folder: both queue, and run in order.
    let created = submit(
        &env,
        WriteRequest::FolderCreate {
            account: None,
            name: "Familiar Test".into(),
            parent: None,
        },
        WriteFlags::default(),
    )
    .await
    .unwrap();
    assert_eq!(created.ops.len(), 1);
    let local = folder_named(&env, "Familiar Test").expect("folder shows at once");
    let moved = submit(
        &env,
        WriteRequest::Move {
            messages: vec![ids[2]],
            to: "Familiar Test".into(),
        },
        waiting(),
    )
    .await
    .unwrap();
    assert_eq!(moved.ops[0].state, "done", "{moved:?}");
    let f = folder_named(&env, "Familiar Test").unwrap();
    assert_eq!(f.id, local.id);
    assert!(!f.remote_id.starts_with(LOCAL_REMOTE_PREFIX));
    assert_eq!(
        mbox().message(&msgs[2].remote_id).unwrap().folder_id,
        f.remote_id
    );

    // Rename, move under Inbox, delete (into Deleted Items).
    submit(
        &env,
        WriteRequest::FolderRename {
            account: None,
            folder: "Familiar Test".into(),
            name: "Familiar Test 2".into(),
        },
        waiting(),
    )
    .await
    .unwrap();
    submit(
        &env,
        WriteRequest::FolderMove {
            account: None,
            folder: "Familiar Test 2".into(),
            to: Some("inbox".into()),
        },
        waiting(),
    )
    .await
    .unwrap();
    let inbox_remote = mbox().folder_id("inbox").unwrap();
    let deleted_remote = mbox().folder_id("deleteditems").unwrap();
    submit(
        &env,
        WriteRequest::FolderDelete {
            account: None,
            folder: "Inbox/Familiar Test 2".into(),
            permanent: false,
        },
        waiting(),
    )
    .await
    .unwrap();
    let local = folder_named(&env, "Familiar Test 2").unwrap();
    let parent = env.reader().folder(local.parent.unwrap()).unwrap().unwrap();
    assert_eq!(parent.well_known.as_deref(), Some("deleteditems"));
    let _ = inbox_remote;

    // Archive one, delete one (to Deleted Items).
    submit(
        &env,
        WriteRequest::Archive {
            messages: vec![ids[3]],
        },
        waiting(),
    )
    .await
    .unwrap();
    assert_eq!(
        mbox().message(&msgs[3].remote_id).unwrap().folder_id,
        mbox().folder_id("archive").unwrap()
    );
    submit(
        &env,
        WriteRequest::Delete {
            messages: vec![ids[1]],
            permanent: false,
        },
        waiting(),
    )
    .await
    .unwrap();
    assert_eq!(
        mbox().message(&msgs[1].remote_id).unwrap().folder_id,
        deleted_remote
    );

    // Deleting it again needs --permanent, then it is gone for good.
    let again = submit(
        &env,
        WriteRequest::Delete {
            messages: vec![ids[1]],
            permanent: false,
        },
        waiting(),
    )
    .await
    .unwrap_err();
    assert_eq!(again.kind, ErrorKind::Usage);
    submit(
        &env,
        WriteRequest::Delete {
            messages: vec![ids[1]],
            permanent: true,
        },
        waiting(),
    )
    .await
    .unwrap();
    assert!(mbox().message(&msgs[1].remote_id).is_none());
    assert!(env.reader().message(msgs[1].id).unwrap().is_none());

    // Nothing is left open, and after a sync round the store matches.
    let p = env.paths.clone();
    let list = blocking(move || ops_list_at(&p, &OpsListInput::default()).unwrap()).await;
    assert!(list.ops.is_empty(), "{list:?}");
    env.daemon()
        .service()
        .handle("sync.now", serde_json::json!({}))
        .await
        .unwrap();
    env.until("store matches the mock", LIMIT, |e| e.settled(30))
        .await;

    // Every write is in the audit log.
    let audit = env.reader().audit_log(0, 100).unwrap();
    assert!(
        audit.iter().filter(|a| a.outcome == "done").count() >= 9,
        "{audit:?}"
    );
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queue_survives_famd_stopping_and_kills() {
    let mut env = Env::new(120).await;
    env.start(policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let msgs = inbox(&env, 3);
    env.stop().await;

    // famd is down: the write is made locally and queued by fam itself.
    let out = submit(
        &env,
        WriteRequest::MarkRead {
            messages: msgs.iter().map(|m| m.id.0).collect(),
            read: true,
        },
        WriteFlags::default(),
    )
    .await
    .unwrap();
    let queued = out.ops.len();
    assert!(queued > 0);
    for m in &msgs {
        assert!(env.reader().message(m.id).unwrap().unwrap().is_read);
    }
    // --wait refuses without famd, before changing anything.
    let refused = submit(
        &env,
        WriteRequest::Archive {
            messages: vec![msgs[0].id.0],
        },
        waiting(),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.kind, ErrorKind::DaemonUnavailable);
    assert_eq!(
        env.reader().message(msgs[0].id).unwrap().unwrap().folder,
        msgs[0].folder
    );

    // A kill while an op was in flight leaves it claimed in the store.
    {
        let mut w = Writer::open(&env.paths.store).unwrap();
        let acc = env.account();
        let op = w
            .write(|tx| tx.claim_op_for(acc, i64::MAX / 2))
            .unwrap()
            .unwrap();
        assert_eq!(op.state, OpState::InFlight);
    }

    // famd starts again: everything is sent, the claimed op included.
    env.start(policy());
    env.until("queued ops sent", LIMIT, |e| {
        msgs.iter()
            .all(|m| e.mock.mailbox(KEY).message(&m.remote_id).unwrap().is_read)
    })
    .await;
    env.until("queue empty", LIMIT, |e| {
        e.reader().open_ops().unwrap().is_empty()
    })
    .await;
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn passing_failures_retry_and_refusals_undo() {
    let mut env = Env::new(120).await;
    env.start(policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let msgs = inbox(&env, 3);

    // A 500 on the first PATCH: the op waits and goes again.
    env.mock.push_fault(
        Fault::status(500, "InternalServerError", "Try again")
            .on_method("PATCH")
            .times(1),
    );
    let out = submit(
        &env,
        WriteRequest::Flag {
            messages: vec![msgs[0].id.0],
            flagged: true,
        },
        WriteFlags::default(),
    )
    .await
    .unwrap();
    let id = out.ops[0].id.unwrap();
    env.until("first try failed", LIMIT, |e| {
        e.reader()
            .op(familiar_store::OpId(id))
            .unwrap()
            .is_some_and(|o| o.last_error.is_some())
    })
    .await;
    // The retry is due a second later by famd's clock.
    env.now.fetch_add(5, Ordering::SeqCst);
    env.until("retried and landed", LIMIT, |e| {
        e.mock
            .mailbox(KEY)
            .message(&msgs[0].remote_id)
            .unwrap()
            .flagged
    })
    .await;

    // A refusal: the folder can't be created, so the move into it fails
    // too and both are undone locally.
    env.mock.push_fault(
        Fault::access_denied()
            .on_method("POST")
            .on_path("mailFolders")
            .times(1),
    );
    env.stop().await;
    submit(
        &env,
        WriteRequest::FolderCreate {
            account: None,
            name: "Doomed".into(),
            parent: None,
        },
        WriteFlags::default(),
    )
    .await
    .unwrap();
    let moved = submit(
        &env,
        WriteRequest::Move {
            messages: vec![msgs[1].id.0],
            to: "Doomed".into(),
        },
        WriteFlags::default(),
    )
    .await
    .unwrap();
    assert_ne!(
        env.reader().message(msgs[1].id).unwrap().unwrap().folder,
        msgs[1].folder
    );
    env.start(policy());
    let move_id = moved.ops[0].id.unwrap();
    env.until("both failed", LIMIT, |e| {
        e.reader()
            .op(familiar_store::OpId(move_id))
            .unwrap()
            .is_some_and(|o| o.state == OpState::Failed)
    })
    .await;
    assert!(folder_named(&env, "Doomed").is_none());
    assert_eq!(
        env.reader().message(msgs[1].id).unwrap().unwrap().folder,
        msgs[1].folder
    );

    // A message gone from the server: the read change is undone.
    let gone = &msgs[2];
    env.mock
        .mailbox(KEY)
        .delete_message(&gone.remote_id)
        .unwrap();
    let out = submit(
        &env,
        WriteRequest::MarkRead {
            messages: vec![gone.id.0],
            read: !gone.is_read,
        },
        waiting(),
    )
    .await
    .unwrap_err();
    assert_eq!(out.kind, ErrorKind::Server);
    if let Some(m) = env.reader().message(gone.id).unwrap() {
        assert_eq!(m.is_read, gone.is_read);
    }
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn process_creates_the_folder_once_then_moves_into_it() {
    let mut env = Env::new(50).await;
    env.start(policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let msgs = inbox(&env, 2);
    let mbox = || env.mock.mailbox(KEY);
    let process = |ids: Vec<i64>, folder: Option<&str>| WriteRequest::Process {
        messages: ids,
        folders: folder
            .map(|f| (KEY.to_string(), f.to_string()))
            .into_iter()
            .collect(),
    };

    // Not set for the account: refused, nothing changes.
    let err = submit(&env, process(vec![msgs[0].id.0], None), waiting())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage, "{err:?}");
    assert!(folder_named(&env, "Processed").is_none());

    // The first one creates the folder, then moves into it.
    let out = submit(
        &env,
        process(vec![msgs[0].id.0], Some("Processed")),
        waiting(),
    )
    .await
    .unwrap();
    let kinds: Vec<&str> = out.ops.iter().map(|o| o.kind.as_str()).collect();
    assert_eq!(kinds, ["create_folder", "move_message"], "{out:?}");
    assert!(out.ops.iter().all(|o| o.state == "done"), "{out:?}");
    let f = folder_named(&env, "Processed").unwrap();
    assert!(!f.remote_id.starts_with(LOCAL_REMOTE_PREFIX));
    assert_eq!(
        mbox().message(&msgs[0].remote_id).unwrap().folder_id,
        f.remote_id
    );

    // The next one finds it (by name, any case).
    let out = submit(
        &env,
        process(vec![msgs[1].id.0], Some("processed")),
        waiting(),
    )
    .await
    .unwrap();
    assert_eq!(out.ops.len(), 1, "{out:?}");
    assert_eq!(out.ops[0].kind, "move_message");
    assert_eq!(
        mbox().message(&msgs[1].remote_id).unwrap().folder_id,
        f.remote_id
    );
    assert_eq!(
        env.reader().message(msgs[1].id).unwrap().unwrap().folder,
        f.id
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn report_moves_to_junk_and_back_through_beta_report_message() {
    use familiar_core::ops::Report;
    let mut env = Env::new(50).await;
    env.start(policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let msgs = inbox(&env, 2);
    let mbox = || env.mock.mailbox(KEY);
    let junk = env
        .reader()
        .folders(env.account())
        .unwrap()
        .into_iter()
        .find(|f| f.well_known.as_deref() == Some("junkemail"))
        .unwrap();
    let report = |id: i64, report: Report| WriteRequest::Report {
        messages: vec![id],
        report,
    };

    // Not junk only makes sense in Junk Email.
    let err = submit(&env, report(msgs[0].id.0, Report::NotJunk), waiting())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage, "{err:?}");

    // Junk and phishing both land in Junk Email, here and on the server.
    for (m, r, action) in [
        (&msgs[0], Report::Junk, "\"junk\""),
        (&msgs[1], Report::Phishing, "\"phish\""),
    ] {
        let out = submit(&env, report(m.id.0, r), waiting()).await.unwrap();
        assert_eq!(out.ops.len(), 1, "{out:?}");
        assert_eq!(out.ops[0].kind, "report_message");
        assert_eq!(out.ops[0].state, "done", "{out:?}");
        assert_eq!(
            mbox().message(&m.remote_id).unwrap().folder_id,
            junk.remote_id
        );
        assert_eq!(env.reader().message(m.id).unwrap().unwrap().folder, junk.id);
        let sent = env
            .mock
            .requests()
            .into_iter()
            .rfind(|q| q.path.ends_with("/reportMessage"))
            .unwrap();
        assert!(sent.path.starts_with("/beta/"), "{}", sent.path);
        assert!(sent.body.contains(action), "{}", sent.body);
    }

    // Junk again: already there, nothing to send.
    let out = submit(&env, report(msgs[0].id.0, Report::Junk), waiting())
        .await
        .unwrap();
    assert!(out.ops.is_empty(), "{out:?}");

    // Not junk goes back to the Inbox.
    let out = submit(&env, report(msgs[0].id.0, Report::NotJunk), waiting())
        .await
        .unwrap();
    assert_eq!(out.ops[0].state, "done", "{out:?}");
    assert_eq!(
        env.reader().message(msgs[0].id).unwrap().unwrap().folder,
        msgs[0].folder
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snooze_hides_mail_until_famd_brings_it_back() {
    let mut env = Env::new(50).await;
    env.start(SyncPolicy {
        snooze_interval: Duration::from_millis(100),
        ..policy()
    });
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let msgs = inbox(&env, 4);
    let inbox_id = msgs[0].folder;
    let mbox = || env.mock.mailbox(KEY);
    let real_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let until = real_now + 3600;
    let ids: Vec<i64> = msgs.iter().map(|m| m.id.0).collect();

    // A time gone by is refused.
    let err = submit(
        &env,
        WriteRequest::Snooze {
            messages: vec![ids[0]],
            until: real_now - 60,
        },
        waiting(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage, "{err:?}");

    // Snoozing makes the folder, then moves into it, here and on the server.
    let out = submit(
        &env,
        WriteRequest::Snooze {
            messages: ids[..3].to_vec(),
            until,
        },
        waiting(),
    )
    .await
    .unwrap();
    let kinds: Vec<&str> = out.ops.iter().map(|o| o.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "create_folder",
            "move_message",
            "move_message",
            "move_message"
        ],
        "{out:?}"
    );
    assert!(out.ops.iter().all(|o| o.state == "done"), "{out:?}");
    let snoozed = folder_named(&env, "Snoozed").unwrap();
    for m in &msgs[..3] {
        assert_eq!(
            mbox().message(&m.remote_id).unwrap().folder_id,
            snoozed.remote_id
        );
    }
    assert_eq!(env.reader().snoozes().unwrap().len(), 3);

    // Snoozing again only moves the time; it still goes back to the Inbox.
    let out = submit(
        &env,
        WriteRequest::Snooze {
            messages: vec![ids[1]],
            until: until + 3600,
        },
        waiting(),
    )
    .await
    .unwrap();
    assert!(out.ops.is_empty(), "{out:?}");
    let s = env.reader().snoozes().unwrap();
    let s1 = s.iter().find(|s| s.message == msgs[1].id).unwrap();
    assert_eq!((s1.until, s1.return_folder), (until + 3600, Some(inbox_id)));

    // Unsnooze brings it back now, as it was (read state kept).
    let out = submit(
        &env,
        WriteRequest::Unsnooze {
            messages: vec![ids[1]],
            mark_unread: false,
        },
        waiting(),
    )
    .await
    .unwrap();
    assert_eq!(out.ops.len(), 1, "{out:?}");
    let back = env.reader().message(msgs[1].id).unwrap().unwrap();
    assert_eq!(back.folder, inbox_id);
    assert_eq!(back.is_read, msgs[1].is_read);
    assert_eq!(env.reader().snoozes().unwrap().len(), 2);

    // A message moved out of Snoozed by hand stays where it was put.
    submit(
        &env,
        WriteRequest::Archive {
            messages: vec![ids[2]],
        },
        waiting(),
    )
    .await
    .unwrap();
    let archive = env.reader().message(msgs[2].id).unwrap().unwrap().folder;

    // Make sure the one famd brings back is read, to see it come back unread.
    submit(
        &env,
        WriteRequest::MarkRead {
            messages: vec![ids[0]],
            read: true,
        },
        waiting(),
    )
    .await
    .unwrap();

    // Nothing comes back early.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        env.reader().message(msgs[0].id).unwrap().unwrap().folder,
        snoozed.id
    );

    // When the time comes famd moves it back to the Inbox, unread.
    env.now.store(until + 1, Ordering::SeqCst);
    env.until("snooze back", LIMIT, |e| {
        let m = mbox().message(&msgs[0].remote_id).unwrap();
        m.folder_id == mbox().folder_id("inbox").unwrap()
            && !m.is_read
            && e.reader().open_ops().is_ok_and(|o| o.is_empty())
    })
    .await;
    let m0 = env.reader().message(msgs[0].id).unwrap().unwrap();
    assert_eq!((m0.folder, m0.is_read), (inbox_id, false));
    assert_eq!(
        env.reader().message(msgs[2].id).unwrap().unwrap().folder,
        archive
    );
    assert!(env.reader().snoozes().unwrap().is_empty());
    env.stop().await;
}
