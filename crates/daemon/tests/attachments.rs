// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 3.3 through a running famd: `fam mail attachments` lists a message's
//! attachments from Graph, saves them without replacing files, and returns
//! the bytes and Content-ID of inline images for the reading pane.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine as _;
use common::{Env, KEY, fast_policy};
use familiar_core::ErrorKind;
use familiar_core::commands::attachments::{
    AttachmentKind, MailAttachmentsInput, attachments_at, content_at, list_at,
};
use familiar_mock_graph::{Attachment, EmailAddress, Message};

const LIMIT: Duration = Duration::from_secs(60);

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attachments_list_save_and_inline() {
    let mut env = Env::new(50).await;
    let now = env.now.load(Ordering::SeqCst);
    let mut m = Message::new(
        "inbox",
        "Minutes attached",
        EmailAddress::new("Sec", "sec@club.example"),
        chrono::DateTime::from_timestamp(now - 60, 0).unwrap(),
    );
    m.body_html = Some(r#"<p>See attached.</p><img src="cid:logo@club">"#.into());
    m.has_attachments = true;
    m.attachments = vec![
        Attachment {
            id: "att-minutes".into(),
            name: "minutes.txt".into(),
            content_type: "text/plain".into(),
            is_inline: false,
            content_id: None,
            content_bytes: b64(b"Minutes of the AGM"),
        },
        Attachment {
            id: "att-logo".into(),
            name: "logo.png".into(),
            content_type: "image/png".into(),
            is_inline: true,
            content_id: Some("logo@club".into()),
            content_bytes: b64(b"\x89PNG fake"),
        },
    ];
    let remote = env.mock.mailbox(KEY).add_message(m).unwrap();
    env.start(fast_policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let id = env
        .reader()
        .message_by_remote(env.account(), &remote)
        .unwrap()
        .expect("synced")
        .id
        .0;

    // The list has both, without bytes.
    let p = env.paths.clone();
    let all = blocking(move || list_at(&p, id).unwrap()).await;
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[0].name, "minutes.txt");
    assert_eq!(all[0].kind, AttachmentKind::File);
    assert!(all[1].is_inline);

    // The command leaves inline images out unless asked.
    let p = env.paths.clone();
    let out = blocking(move || {
        attachments_at(
            &p,
            &MailAttachmentsInput {
                message: id,
                ..Default::default()
            },
        )
        .unwrap()
    })
    .await;
    assert_eq!(out.attachments.len(), 1);

    // Saving twice keeps both files.
    let dir = tempfile::tempdir().unwrap();
    for want in ["minutes.txt", "minutes (2).txt"] {
        let p = env.paths.clone();
        let d = dir.path().display().to_string();
        let out = blocking(move || {
            attachments_at(
                &p,
                &MailAttachmentsInput {
                    message: id,
                    save: Some(d),
                    name: Some("minutes.txt".into()),
                    inline: false,
                },
            )
            .unwrap()
        })
        .await;
        let saved = out.attachments[0].saved_to.clone().unwrap();
        assert!(saved.ends_with(want), "{saved}");
        assert_eq!(std::fs::read(&saved).unwrap(), b"Minutes of the AGM");
    }

    // An inline image comes with its Content-ID.
    let p = env.paths.clone();
    let logo = blocking(move || content_at(&p, id, "att-logo").unwrap()).await;
    assert_eq!(logo.content_id.as_deref(), Some("logo@club"));
    assert_eq!(logo.bytes, b"\x89PNG fake");

    // An unknown name is not_found.
    let p = env.paths.clone();
    let e = blocking(move || {
        attachments_at(
            &p,
            &MailAttachmentsInput {
                message: id,
                name: Some("nope.pdf".into()),
                ..Default::default()
            },
        )
        .unwrap_err()
    })
    .await;
    assert_eq!(e.kind, ErrorKind::NotFound);
    env.stop().await;
}
