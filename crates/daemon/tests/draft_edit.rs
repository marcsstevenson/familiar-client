// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam draft edit` and `--markdown`, against the mock: the editor is a
//! closure that rewrites the file.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{Env, KEY, fast_policy};
use familiar_core::CommandError;
use familiar_core::commands::draft::{ComposeArgs, ComposeSettings, compose_at};
use familiar_core::commands::draft_edit::{DraftEditInput, edit_at};
use familiar_core::commands::write::WriteFlags;
use familiar_core::compose::ResponseKind;
use familiar_core::ops::WriteOutput;
use familiar_daemon::policy::SyncPolicy;
use familiar_store::MessageId;

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
        aliases: vec![],
    }
}

fn input(draft: Option<i64>) -> DraftEditInput {
    DraftEditInput {
        draft,
        account: None,
        no_signature: false,
        wait: true,
        wait_secs: Some(30),
    }
}

fn draft_id(out: &WriteOutput) -> i64 {
    out.ops
        .iter()
        .find(|o| o.kind == "create_draft")
        .and_then(|o| o.item)
        .expect("a draft")
}

/// Runs `fam draft edit` with an "editor" that checks what it was given
/// and writes `new` (or leaves the file alone when `None`).
async fn edit(
    env: &Env,
    draft: Option<i64>,
    expect: Vec<&'static str>,
    new: Option<&'static str>,
) -> Result<WriteOutput, CommandError> {
    let p = env.paths.clone();
    blocking(move || {
        edit_at(&p, &input(draft), &settings(), &|path: &Path| {
            let was = std::fs::read_to_string(path).unwrap();
            for e in &expect {
                assert!(was.contains(e), "{e:?} not in:\n{was}");
            }
            if let Some(n) = new {
                std::fs::write(path, n).unwrap();
            }
            Ok(())
        })
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edit_drafts_in_markdown() {
    let mut env = Env::new(60).await;
    env.start(policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let mbox = || env.mock.mailbox(KEY);
    let flags = WriteFlags {
        wait: true,
        wait_secs: Some(30),
        ..WriteFlags::default()
    };

    // A plain draft, then edited: Markdown in, HTML out, signature kept once.
    let p = env.paths.clone();
    let f = flags.clone();
    let out = blocking(move || {
        compose_at(
            &p,
            &ComposeArgs {
                to: vec![format!("Me <{ME}>")],
                subject: Some("Familiar Test edit".into()),
                body: Some("Hello".into()),
                ..ComposeArgs::default()
            },
            &settings(),
            None,
            &f,
        )
    })
    .await
    .unwrap();
    let draft = draft_id(&out);
    let out = edit(
        &env,
        Some(draft),
        vec!["To: Me <test.user@example.com>\nCc: \nBcc: \nSubject: Familiar Test edit\n\nHello\n"],
        Some("To: Me <test.user@example.com>\nSubject: Familiar Test edited\n\nHi **there**,\n\n- one\n- two\n\nThanks,\nPat\n"),
    )
    .await
    .unwrap();
    assert_eq!(out.ops.len(), 1, "{out:?}");
    let local = env.reader().message(MessageId(draft)).unwrap().unwrap();
    let server = mbox().message(&local.remote_id).unwrap();
    assert_eq!(server.subject, "Familiar Test edited");
    let html = server.body_html.clone().unwrap();
    assert!(html.contains("Hi <strong>there</strong>,"), "{html}");
    assert!(html.contains("<li>one</li>"), "{html}");
    assert!(html.contains("Thanks,<br />\nPat"), "{html}");
    assert_eq!(html.matches("-- Pat (new)").count(), 1, "{html}");
    assert!(!html.contains("Hello"), "{html}");

    // Opened again it reads back as the Markdown written; closing it
    // unchanged saves nothing.
    let out = edit(
        &env,
        Some(draft),
        vec!["Hi **there**,\n\n- one\n- two\n\nThanks,\nPat\n"],
        None,
    )
    .await
    .unwrap();
    assert!(out.ops.is_empty(), "{out:?}");

    // A reply: the quoted original stays below the new text.
    let orig = {
        let r = env.reader();
        let f = r
            .folders(env.account())
            .unwrap()
            .into_iter()
            .find(|f| f.well_known.as_deref() == Some("inbox"))
            .unwrap();
        r.messages(f.id, None, 10).unwrap().remove(0)
    };
    let p = env.paths.clone();
    let f = flags.clone();
    let id = orig.id.0;
    let out = blocking(move || {
        compose_at(
            &p,
            &ComposeArgs {
                respond_to: Some(id),
                response: Some(ResponseKind::Reply),
                body: Some("First go".into()),
                ..ComposeArgs::default()
            },
            &settings(),
            None,
            &f,
        )
    })
    .await
    .unwrap();
    let reply = draft_id(&out);
    let before = {
        let local = env.reader().message(MessageId(reply)).unwrap().unwrap();
        mbox().message(&local.remote_id).unwrap().body_html.unwrap()
    };
    let quote_at = before.find("divRplyFwdMsg").unwrap();
    edit(
        &env,
        Some(reply),
        vec!["\n\nFirst go\n"],
        Some("To: someone@example.com\nSubject: Re: x\n\nSecond *go*\n"),
    )
    .await
    .unwrap();
    let local = env.reader().message(MessageId(reply)).unwrap().unwrap();
    let server = mbox().message(&local.remote_id).unwrap();
    let html = server.body_html.unwrap();
    assert!(html.contains("Second <em>go</em>"), "{html}");
    assert!(!html.contains("First go"), "{html}");
    assert!(html.contains("-- Pat (reply)"), "{html}");
    let hr = before[..quote_at].rfind("<hr").unwrap();
    let quote = &before[hr..hr + before[hr..].find("</body>").unwrap()];
    assert!(html.contains(quote), "{quote}\nnot in\n{html}");
    assert_eq!(server.to[0].address, "someone@example.com");

    // A new message written in the editor gets the new-message signature.
    let out = edit(
        &env,
        None,
        vec!["To: \nCc: \nBcc: \nSubject: \n\n"],
        Some("To: test.user@example.com\nSubject: Familiar Test new\n\n# Plan\n\nSee [here](https://x.test)."),
    )
    .await
    .unwrap();
    let new = draft_id(&out);
    let local = env.reader().message(MessageId(new)).unwrap().unwrap();
    let server = mbox().message(&local.remote_id).unwrap();
    assert!(server.is_draft);
    assert_eq!(server.subject, "Familiar Test new");
    let html = server.body_html.unwrap();
    assert!(html.contains("<h1>Plan</h1>"), "{html}");
    assert!(
        html.contains("<a href=\"https://x.test\">here</a>"),
        "{html}"
    );
    assert!(html.contains("-- Pat (new)"), "{html}");

    // A file left empty saves nothing.
    let out = edit(&env, None, vec![], None).await.unwrap();
    assert!(out.ops.is_empty(), "{out:?}");

    // A bad header line keeps what was written and says where.
    let err = edit(&env, Some(draft), vec![], Some("From: me\n\nlost?"))
        .await
        .unwrap_err();
    assert!(err.message.contains("still in"), "{err:?}");
    let kept = err.message.rsplit("still in ").next().unwrap();
    assert!(std::fs::read_to_string(kept).unwrap().contains("lost?"));
    let _ = std::fs::remove_file(kept);
}
