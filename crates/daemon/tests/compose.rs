// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 2.3: drafts, replies, attachments, signatures, undo send and the
//! offline outbox, against the mock.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{Env, KEY, fast_policy};
use familiar_core::CommandError;
use familiar_core::commands::draft::{ComposeArgs, ComposeSettings, compose_at, send_at};
use familiar_core::commands::write::{WriteFlags, submit_at};
use familiar_core::compose::{Recipient, ResponseKind};
use familiar_core::ops::{WriteOutput, WriteRequest};
use familiar_daemon::policy::SyncPolicy;
use familiar_store::{MessageId, OpId, OpState};

const LIMIT: Duration = Duration::from_secs(60);
const ME: &str = "test.user@example.com";

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

fn settings() -> ComposeSettings {
    ComposeSettings {
        new_signatures: vec![(KEY.into(), "<p>-- Pat (new)</p>".into())],
        reply_signatures: vec![(KEY.into(), "<p>-- Pat (reply)</p>".into())],
        send_delay_secs: 30,
        aliases: vec![(KEY.into(), vec!["pat.alias@example.com".into()])],
    }
}

fn waiting() -> WriteFlags {
    WriteFlags {
        wait: true,
        wait_secs: Some(30),
        ..WriteFlags::default()
    }
}

async fn compose(
    env: &Env,
    args: ComposeArgs,
    send: Option<(bool, bool)>,
    flags: WriteFlags,
) -> Result<WriteOutput, CommandError> {
    let p = env.paths.clone();
    blocking(move || compose_at(&p, &args, &settings(), send, &flags)).await
}

fn draft_id(out: &WriteOutput) -> i64 {
    out.ops
        .iter()
        .find(|o| o.kind == "create_draft")
        .and_then(|o| o.item)
        .expect("a draft")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drafts_replies_and_sending() {
    let mut env = Env::new(150).await;
    env.start(policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let mbox = || env.mock.mailbox(KEY);

    // A new draft with an attachment and the new-message signature.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, b"club notes").unwrap();
    let out = compose(
        &env,
        ComposeArgs {
            to: vec![format!("Me <{ME}>")],
            subject: Some("Familiar Test draft".into()),
            body: Some("Hello <there>\nsecond line".into()),
            attach: vec![file.to_string_lossy().into_owned()],
            ..ComposeArgs::default()
        },
        None,
        waiting(),
    )
    .await
    .unwrap();
    let draft = draft_id(&out);
    let local = env.reader().message(MessageId(draft)).unwrap().unwrap();
    assert!(!local.remote_id.starts_with("local:"), "{local:?}");
    let server = mbox().message(&local.remote_id).unwrap();
    assert!(server.is_draft);
    assert_eq!(server.subject, "Familiar Test draft");
    assert_eq!(server.to[0].address, ME);
    let html = server.body_html.clone().unwrap();
    assert!(html.contains("Hello &lt;there&gt;<br>"), "{html}");
    assert!(html.contains("-- Pat (new)"), "{html}");
    assert_eq!(server.attachments.len(), 1);
    assert_eq!(server.attachments[0].name, "notes.txt");
    assert_eq!(server.attachments[0].content_bytes, "Y2x1YiBub3Rlcw==");

    // Update the subject.
    let p = env.paths.clone();
    blocking(move || {
        submit_at(
            &p,
            WriteRequest::DraftUpdate {
                message: draft,
                from: None,
                subject: Some("Familiar Test draft v2".into()),
                html: None,
                to: None,
                cc: None,
                bcc: None,
                attachments: vec![],
                remove_attachments: vec![],
            },
            &waiting(),
        )
    })
    .await
    .unwrap();
    assert_eq!(
        mbox().message(&local.remote_id).unwrap().subject,
        "Familiar Test draft v2"
    );

    // A body with an image, saved twice as the app does: the image is
    // attached once. Then a body without it, removing notes.txt by name:
    // both go.
    let body_update = |html: &str, remove: Vec<String>| {
        let (html, images) = familiar_core::compose::embedded_images(html).unwrap();
        WriteRequest::DraftUpdate {
            message: draft,
            from: None,
            subject: None,
            html: Some(html),
            to: None,
            cc: None,
            bcc: None,
            attachments: images,
            remove_attachments: remove,
        }
    };
    let pic = "<p>See</p><img src=\"data:image/png;base64,iVBORw0KGgo=\">";
    for _ in 0..2 {
        let (p, req) = (env.paths.clone(), body_update(pic, vec![]));
        blocking(move || submit_at(&p, req, &waiting()))
            .await
            .unwrap();
        let atts = mbox().message(&local.remote_id).unwrap().attachments;
        let names: Vec<&str> = atts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(atts.len(), 2, "{names:?}");
        assert!(atts.iter().any(|a| a.is_inline), "{names:?}");
    }
    let (p, req) = (
        env.paths.clone(),
        body_update("<p>No picture</p>", vec!["notes.txt".into()]),
    );
    blocking(move || submit_at(&p, req, &waiting()))
        .await
        .unwrap();
    assert!(
        mbox()
            .message(&local.remote_id)
            .unwrap()
            .attachments
            .is_empty()
    );

    // Send it now: it moves to Sent Items and, being to me, arrives in the Inbox.
    let p = env.paths.clone();
    let sent = blocking(move || send_at(&p, draft, true, false, &settings(), &waiting()))
        .await
        .unwrap();
    assert_eq!(sent.ops[0].state, "done", "{sent:?}");
    let after = mbox().message(&local.remote_id).unwrap();
    assert!(!after.is_draft);
    assert_eq!(after.folder_id, mbox().folder_id("sentitems").unwrap());
    let inbox = mbox().folder_id("inbox").unwrap();
    assert!(
        mbox()
            .message_ids(&inbox)
            .iter()
            .any(|id| mbox().message(id).unwrap().subject == "Familiar Test draft v2")
    );

    // Reply to an Inbox message: threaded, quoted, reply signature.
    let orig = {
        let r = env.reader();
        let f = r
            .folders(env.account())
            .unwrap()
            .into_iter()
            .find(|f| f.well_known.as_deref() == Some("inbox"))
            .unwrap();
        // Not the test mail sent to ourselves above, which quotes the new signature.
        r.messages(f.id, None, 10)
            .unwrap()
            .into_iter()
            .find(|m| !m.subject.starts_with("Familiar Test"))
            .unwrap()
    };
    let out = compose(
        &env,
        ComposeArgs {
            respond_to: Some(orig.id.0),
            response: Some(ResponseKind::Reply),
            body: Some("Thanks!".into()),
            ..ComposeArgs::default()
        },
        None,
        waiting(),
    )
    .await
    .unwrap();
    let reply = env
        .reader()
        .message(MessageId(draft_id(&out)))
        .unwrap()
        .unwrap();
    let server = mbox().message(&reply.remote_id).unwrap();
    let o = mbox().message(&orig.remote_id).unwrap();
    assert_eq!(server.conversation_id, o.conversation_id);
    assert!(server.subject.starts_with("RE: "), "{}", server.subject);
    assert_eq!(server.to[0].address, o.from.address);
    let html = server.body_html.unwrap();
    assert!(
        html.contains("Thanks!") && html.contains("-- Pat (reply)"),
        "{html}"
    );
    assert!(html.contains("divRplyFwdMsg"), "{html}");
    assert!(!html.contains("-- Pat (new)"));

    // From an alias, then back to the account's own address.
    const ALIAS: &str = "pat.alias@example.com";
    let out = compose(
        &env,
        ComposeArgs {
            from: Some(ALIAS.into()),
            to: vec![ME.into()],
            subject: Some("Familiar Test alias".into()),
            ..ComposeArgs::default()
        },
        None,
        waiting(),
    )
    .await
    .unwrap();
    let aliased = draft_id(&out);
    let local = env.reader().message(MessageId(aliased)).unwrap().unwrap();
    assert_eq!(local.from.addr, ALIAS);
    assert_eq!(
        mbox().message(&local.remote_id).unwrap().from.address,
        ALIAS
    );
    let p = env.paths.clone();
    blocking(move || {
        submit_at(
            &p,
            WriteRequest::DraftUpdate {
                message: aliased,
                from: Some(Recipient {
                    name: String::new(),
                    address: ME.into(),
                }),
                subject: None,
                html: None,
                to: None,
                cc: None,
                bcc: None,
                attachments: vec![],
                remove_attachments: vec![],
            },
            &waiting(),
        )
    })
    .await
    .unwrap();
    assert_eq!(mbox().message(&local.remote_id).unwrap().from.address, ME);
    // An address that isn't the account's is refused.
    let err = compose(
        &env,
        ComposeArgs {
            from: Some("someone.else@example.com".into()),
            to: vec![ME.into()],
            ..ComposeArgs::default()
        },
        None,
        waiting(),
    )
    .await
    .unwrap_err();
    assert!(err.message.contains("aliases"), "{err:?}");

    // After a sync round, the store matches the mock (no duplicate drafts).
    env.daemon()
        .service()
        .handle("sync.now", serde_json::json!({}))
        .await
        .unwrap();
    env.until("store matches the mock", LIMIT, |e| e.settled(30))
        .await;
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn undo_send_and_offline_outbox() {
    let mut env = Env::new(80).await;
    env.start(policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let mbox = || env.mock.mailbox(KEY);
    let args = |subject: &str| ComposeArgs {
        to: vec![ME.into()],
        subject: Some(subject.into()),
        body: Some("hi".into()),
        ..ComposeArgs::default()
    };

    // famd's clock reads the real time here, so the delay counts.
    let real_now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    };
    env.now.store(real_now(), Ordering::SeqCst);

    // Send with the 30 s undo delay, then cancel: the draft stays a draft.
    let out = compose(
        &env,
        args("Familiar Test undo"),
        Some((false, false)),
        WriteFlags::default(),
    )
    .await
    .unwrap();
    let send = out.ops.iter().find(|o| o.kind == "send_draft").unwrap();
    assert!(send.not_before.is_some());
    let send_id = send.id.unwrap();
    let draft = draft_id(&out);
    env.until("draft saved", LIMIT, |e| {
        !e.reader()
            .message(MessageId(draft))
            .unwrap()
            .unwrap()
            .remote_id
            .starts_with("local:")
    })
    .await;
    let cancelled = env
        .daemon()
        .service()
        .handle("ops.cancel", serde_json::json!({"id": send_id}))
        .await
        .unwrap();
    assert_eq!(cancelled, serde_json::json!(true));
    let remote = env
        .reader()
        .message(MessageId(draft))
        .unwrap()
        .unwrap()
        .remote_id;
    assert!(mbox().message(&remote).unwrap().is_draft);

    // Not cancelled: it goes once the delay has passed (by famd's clock).
    let out = compose(
        &env,
        args("Familiar Test delayed"),
        Some((false, false)),
        WriteFlags::default(),
    )
    .await
    .unwrap();
    let send_id = out
        .ops
        .iter()
        .find(|o| o.kind == "send_draft")
        .unwrap()
        .id
        .unwrap();
    env.now.store(real_now() + 31, Ordering::SeqCst);
    env.until("sent after the delay", LIMIT, |e| {
        e.reader()
            .op(OpId(send_id))
            .unwrap()
            .is_some_and(|o| o.state == OpState::Done)
    })
    .await;

    // Offline (famd stopped): mail send queues; famd sends it on start.
    env.stop().await;
    let out = compose(
        &env,
        args("Familiar Test outbox"),
        Some((true, false)),
        WriteFlags::default(),
    )
    .await
    .unwrap();
    assert_eq!(out.ops.len(), 2);
    env.start(policy());
    env.until("outbox sent", LIMIT, |e| {
        let sent = e.mock.mailbox(KEY).folder_id("sentitems").unwrap();
        e.mock
            .mailbox(KEY)
            .message_ids(&sent)
            .iter()
            .any(|id| e.mock.mailbox(KEY).message(id).unwrap().subject == "Familiar Test outbox")
    })
    .await;

    // A send needs a recipient.
    let err = compose(
        &env,
        ComposeArgs {
            subject: Some("nobody".into()),
            ..ComposeArgs::default()
        },
        Some((true, false)),
        WriteFlags::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, familiar_core::ErrorKind::Usage);
    env.stop().await;
}
