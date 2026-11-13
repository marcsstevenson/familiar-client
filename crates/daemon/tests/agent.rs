// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 2.6: agent permissions and the audit log.
//!
//! famd checks writes from agent sources against `[agent]` in the config
//! file (read again for each write), refuses what isn't granted with
//! `permission_denied` and nothing changed, and logs it as `denied`.
//! Writes from people aren't limited.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Env, KEY, fast_policy};
use familiar_core::ErrorKind;
use familiar_core::commands::agent::{AuditListInput, audit_list_at};
use familiar_core::commands::write::{WriteFlags, submit_at};
use familiar_core::compose::{ComposeRequest, Recipient};
use familiar_core::ops::{WriteOutput, WriteRequest};
use familiar_daemon::{AgentGrants, Daemon};
use familiar_store::Message;

const LIMIT: Duration = Duration::from_secs(60);

fn as_agent() -> WriteFlags {
    WriteFlags {
        source: "agent:test".into(),
        ..WriteFlags::default()
    }
}

async fn submit(
    env: &Env,
    req: WriteRequest,
    flags: WriteFlags,
) -> Result<WriteOutput, familiar_core::CommandError> {
    let p = env.paths.clone();
    tokio::task::spawn_blocking(move || submit_at(&p, req, &flags))
        .await
        .unwrap()
}

fn grant(path: &Path, agent: &str) {
    std::fs::write(
        path,
        format!(
            "version = 1\n[accounts.{KEY}]\nemail = \"pat@example.com\"\ntenant = \"consumers\"\n{agent}"
        ),
    )
    .unwrap();
}

fn inbox(env: &Env, n: usize) -> Vec<Message> {
    let r = env.reader();
    let f = r
        .folders(env.account())
        .unwrap()
        .into_iter()
        .find(|f| f.well_known.as_deref() == Some("inbox"))
        .unwrap();
    r.messages(f.id, None, n).unwrap()
}

fn audit(env: &Env, source: Option<&str>, outcome: Option<&str>) -> Vec<(String, String, String)> {
    audit_list_at(
        &env.paths,
        &AuditListInput {
            source: source.map(str::to_string),
            outcome: outcome.map(str::to_string),
            ..AuditListInput::default()
        },
    )
    .unwrap()
    .entries
    .into_iter()
    .map(|e| (e.source, e.command, e.outcome))
    .collect()
}

fn draft(send: bool) -> WriteRequest {
    WriteRequest::Compose {
        draft: ComposeRequest {
            subject: Some("From the agent".into()),
            html: "<p>hi</p>".into(),
            to: vec![Recipient {
                name: String::new(),
                address: "marc@example.com".into(),
            }],
            ..ComposeRequest::default()
        },
        send,
        send_delay_secs: 0,
        send_at: None,
    }
}

fn denied(r: Result<WriteOutput, familiar_core::CommandError>) -> String {
    let e = r.expect_err("should be refused");
    assert_eq!(e.kind, ErrorKind::PermissionDenied, "{e:?}");
    assert_eq!(e.kind.exit_code(), 5);
    e.message
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agents_get_only_what_is_granted() {
    let mut env = Env::new(60).await;
    let config: PathBuf = env.dir.path().join("config.toml");
    grant(&config, "");
    let mut o = env.options(fast_policy());
    o.agent = AgentGrants::File(config.clone());
    env.daemon = Some(Daemon::start(o).unwrap());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let m = inbox(&env, 2);
    let (one, two) = (m[0].id.0, m[1].id.0);
    let read = |ids: Vec<i64>, read: bool| WriteRequest::MarkRead {
        messages: ids,
        read,
    };
    let was = m[0].is_read;

    // Read and draft only, by default.
    let msg = denied(submit(&env, read(vec![one], !was), as_agent()).await);
    assert!(msg.contains("write") && msg.contains(KEY), "{msg}");
    assert_eq!(env.reader().message(m[0].id).unwrap().unwrap().is_read, was);
    // MCP is an agent too; a dry run is refused but not logged.
    let mcp = WriteFlags {
        source: "mcp".into(),
        dry_run: true,
        ..WriteFlags::default()
    };
    denied(submit(&env, read(vec![one], !was), mcp).await);
    // A person isn't limited.
    submit(&env, read(vec![two], true), WriteFlags::default())
        .await
        .unwrap();
    // A draft is fine; sending it isn't, and nothing is made.
    submit(&env, draft(false), as_agent()).await.unwrap();
    let msg = denied(submit(&env, draft(true), as_agent()).await);
    assert!(
        msg.contains("send") && msg.contains("[agent.accounts."),
        "{msg}"
    );
    assert_eq!(
        audit(&env, Some("agent:test"), Some("queued")),
        [("agent:test".into(), "draft.create".into(), "queued".into())],
        "only the draft was queued"
    );

    let denials = audit(&env, Some("agent:*"), Some("denied"));
    assert_eq!(
        denials,
        [
            ("agent:test".into(), "mail.send".into(), "denied".into()),
            ("agent:test".into(), "mail.read".into(), "denied".into()),
        ],
        "{denials:?}"
    );
    assert!(audit(&env, Some("mcp"), None).is_empty());
    assert!(
        audit(&env, Some("cli"), Some("queued"))
            .iter()
            .any(|(_, c, _)| c == "mail.read")
    );

    // Granting write applies without a restart; delete still needs its own grant.
    grant(
        &config,
        "[agent]\nallow = [\"read\", \"draft\", \"write\"]\n",
    );
    submit(&env, read(vec![one], !was), as_agent())
        .await
        .unwrap();
    assert_eq!(
        env.reader().message(m[0].id).unwrap().unwrap().is_read,
        !was
    );
    // An event is a write, but inviting people sends mail.
    let event = |attendees: Vec<Recipient>| WriteRequest::EventCreate {
        account: Some(KEY.into()),
        calendar: None,
        event: familiar_core::events::EventFields {
            subject: "Nets".into(),
            start_at: 1_900_000_000,
            end_at: 1_900_003_600,
            time_zone: "UTC".into(),
            attendees,
            ..Default::default()
        },
        from_mail: None,
    };
    let dry = WriteFlags {
        dry_run: true,
        ..as_agent()
    };
    submit(&env, event(vec![]), dry.clone()).await.unwrap();
    let invite = vec![Recipient {
        name: String::new(),
        address: "coach@example.com".into(),
    }];
    let msg = denied(submit(&env, event(invite), dry).await);
    assert!(msg.contains("send"), "{msg}");
    let del = || WriteRequest::Delete {
        messages: vec![one],
        permanent: false,
    };
    let msg = denied(submit(&env, del(), as_agent()).await);
    assert!(msg.contains("delete"), "{msg}");
    grant(
        &config,
        &format!(
            "[agent]\nallow = [\"read\", \"draft\", \"write\"]\n[agent.accounts.{KEY}]\nallow = [\"delete\"]\n"
        ),
    );
    submit(&env, del(), as_agent()).await.unwrap();
    let mine = audit(&env, Some("agent:test"), Some("queued"));
    assert!(mine.iter().any(|(_, c, _)| c == "mail.delete"), "{mine:?}");
    env.until("the runner logs the ops done", LIMIT, |e| {
        audit(e, Some("agent:test"), Some("done")).len() >= 2
    })
    .await;
    env.stop().await;
}
