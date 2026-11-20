// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail accounts in famd (Gmail plan WP G.5) against the mock: first sync,
//! bodies for the window, changes by history, server search, `fam api`,
//! and writes (G.6): labels, Trash, Spam and label folders.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use familiar_core::CommandError;
use familiar_core::commands::write::{WriteFlags, submit_at};
use familiar_core::compose::{AttachmentData, ComposeRequest, Recipient, ResponseKind};
use familiar_core::daemon::{
    DaemonPaths, FIRST_SYNC_RESOURCE, GraphGetParams, METHOD_SEARCH_ONLINE, SearchOnlineResult,
};
use familiar_core::ops::{Report, WriteOutput, WriteRequest};
use familiar_daemon::policy::SyncPolicy;
use familiar_daemon::{AccountSpec, Daemon, DaemonOptions};
use familiar_mock_gmail::{Fault, MockGmail};
use familiar_provider_gmail::{ClientOptions, GmailClient, StaticToken};
use familiar_store::{AccountId, Reader};
use serde_json::json;

const KEY: &str = "gmail";
const LIMIT: Duration = Duration::from_secs(30);

struct Env {
    mock: MockGmail,
    _dir: tempfile::TempDir,
    paths: DaemonPaths,
    daemon: Daemon,
}

impl Env {
    async fn start() -> Self {
        let mock = MockGmail::start().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let paths = DaemonPaths::at(dir.path());
        let mut opts = ClientOptions::with_base(format!("{}/users/me", mock.api_url()));
        opts.default_retry_after = Duration::from_millis(20);
        let client = GmailClient::new(opts, Arc::new(StaticToken("test-token".into()))).unwrap();
        let mut o = DaemonOptions::new(
            paths.clone(),
            vec![AccountSpec {
                key: KEY.into(),
                email: familiar_mock_gmail::SMALL_EMAIL.into(),
                display_name: "Kea".into(),
                tenant_id: None,
                client: client.into(),
            }],
        );
        o.policy = SyncPolicy {
            window_days: 30,
            inbox_interval: Duration::from_millis(200),
            tree_interval: Duration::from_millis(400),
            ageing_interval: Duration::from_millis(300),
            backoff_min: Duration::from_millis(100),
            backoff_max: Duration::from_millis(500),
            ..SyncPolicy::default()
        };
        // The fixture's mail is from October 2026.
        let now = Utc
            .with_ymd_and_hms(2026, 10, 30, 0, 0, 0)
            .unwrap()
            .timestamp();
        o.clock = Arc::new(move || now);
        o.page_size = Some(4);
        let daemon = Daemon::start(o).unwrap();
        Self {
            mock,
            _dir: dir,
            paths,
            daemon,
        }
    }

    fn reader(&self) -> Reader {
        Reader::open(&self.paths.store).unwrap()
    }

    fn account(&self) -> AccountId {
        self.reader().account_by_key(KEY).unwrap().unwrap().id
    }

    fn message(&self, remote: &str) -> Option<familiar_store::Message> {
        self.reader()
            .message_by_remote(self.account(), remote)
            .unwrap()
    }

    /// Submits `req` and waits for Gmail.
    async fn write(&self, req: WriteRequest) -> Result<WriteOutput, CommandError> {
        let paths = self.paths.clone();
        tokio::task::spawn_blocking(move || {
            submit_at(
                &paths,
                req,
                &WriteFlags {
                    wait: true,
                    wait_secs: Some(20),
                    ..WriteFlags::default()
                },
            )
        })
        .await
        .unwrap()
    }

    async fn write_ok(&self, req: WriteRequest) {
        let out = self.write(req).await.unwrap();
        for op in &out.ops {
            assert_eq!(op.state, "done", "{op:?}");
        }
    }

    fn id(&self, remote: &str) -> i64 {
        self.message(remote).unwrap().id.0
    }

    /// Gmail's labels on a message, sorted.
    fn labels(&self, remote: &str) -> Vec<String> {
        let mut l = self
            .mock
            .mailbox("default")
            .message(remote)
            .unwrap()
            .label_ids;
        l.sort();
        l
    }

    /// The local folders a message is in, by name, sorted.
    fn folders_of(&self, remote: &str) -> Vec<String> {
        let r = self.reader();
        let m = self.message(remote).unwrap();
        let all = r.folders(m.account).unwrap();
        let mut names: Vec<String> = r
            .message_folders(m.id)
            .unwrap()
            .into_iter()
            .map(|f| all.iter().find(|x| x.id == f).unwrap().name.clone())
            .collect();
        names.sort();
        names
    }

    fn gmail_label(&self, name: &str) -> bool {
        self.mock
            .mailbox("default")
            .labels()
            .iter()
            .any(|l| l.name == name)
    }

    async fn first_sync(&self) {
        self.until("the first sync", |e| {
            e.reader()
                .account_by_key(KEY)
                .unwrap()
                .and_then(|a| e.reader().sync_state(a.id, FIRST_SYNC_RESOURCE).unwrap())
                .is_some_and(|s| s.synced_at.is_some())
        })
        .await;
    }

    async fn until(&self, what: &str, mut f: impl FnMut(&Self) -> bool) {
        let start = Instant::now();
        while !f(self) {
            if start.elapsed() > LIMIT {
                panic!(
                    "timed out waiting for {what}; daemon: {:?}",
                    self.daemon.service().status()
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gmail_account_syncs_through_famd() {
    let env = Env::start().await;
    env.until("the Google calendar and its events", |e| {
        let r = e.reader();
        r.account_by_key(KEY).unwrap().is_some_and(|a| {
            r.calendars(a.id)
                .unwrap()
                .iter()
                .any(|c| c.name == "Kea calendar")
                && r.event_by_remote(a.id, "event-october").unwrap().is_some()
        })
    })
    .await;
    env.until("the first sync", |e| {
        e.reader()
            .account_by_key(KEY)
            .unwrap()
            .and_then(|a| e.reader().sync_state(a.id, FIRST_SYNC_RESOURCE).unwrap())
            .is_some_and(|s| s.synced_at.is_some())
    })
    .await;
    assert_eq!(env.reader().accounts().unwrap()[0].provider, "gmail");
    for remote in ["msg-acme-1", "msg-archived", "msg-2015", "msg-trash"] {
        assert!(env.message(remote).is_some(), "{remote}");
    }

    // Bodies for the 30-day window, none for 2015.
    env.until("window bodies", |e| {
        e.message("msg-photos").unwrap().body.is_some()
            && e.message("msg-dinner").unwrap().body.is_some()
    })
    .await;
    assert!(env.message("msg-2015").unwrap().body.is_none());

    // A change on Gmail comes in by history.
    env.mock
        .mailbox("default")
        .modify("msg-dinner", &[], &["UNREAD"])
        .unwrap();
    env.until("the read state", |e| {
        e.message("msg-dinner").unwrap().is_read
    })
    .await;

    // Server search goes to Gmail in its own syntax.
    let v = env
        .daemon
        .service()
        .handle(
            METHOD_SEARCH_ONLINE,
            json!({"kql": "\"quote\"", "gmail_q": "quote", "limit": 10}),
        )
        .await
        .unwrap();
    let found: SearchOnlineResult = serde_json::from_value(v).unwrap();
    assert!(found.failures.is_empty(), "{:?}", found.failures);
    let hit = found
        .hits
        .iter()
        .find(|h| h.remote_id == "msg-acme-1")
        .unwrap();
    assert_eq!(hit.local_id, Some(env.message("msg-acme-1").unwrap().id.0));
    assert!(hit.folder_id.is_some());

    // `fam api` reaches Gmail with the account's token.
    let v = env
        .daemon
        .service()
        .graph_get(GraphGetParams {
            account: Some(KEY.into()),
            path: "/gmail/v1/users/me/profile".into(),
        })
        .await
        .unwrap();
    assert_eq!(v["emailAddress"], familiar_mock_gmail::SMALL_EMAIL);

    env.daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn gmail_writes_change_labels() {
    let env = Env::start().await;
    env.first_sync().await;
    let has = |l: &[String], x: &str| l.iter().any(|y| y == x);

    // Read and flag are labels.
    env.write_ok(WriteRequest::Flag {
        messages: vec![env.id("msg-dinner")],
        flagged: true,
    })
    .await;
    env.write_ok(WriteRequest::MarkRead {
        messages: vec![env.id("msg-dinner")],
        read: true,
    })
    .await;
    let l = env.labels("msg-dinner");
    assert!(has(&l, "STARRED") && !has(&l, "UNREAD"), "{l:?}");

    // Archive takes the Inbox label off; locally it's in All Mail only.
    env.write_ok(WriteRequest::Archive {
        messages: vec![env.id("msg-dinner")],
    })
    .await;
    assert!(!has(&env.labels("msg-dinner"), "INBOX"));
    assert_eq!(env.folders_of("msg-dinner"), ["All Mail"]);

    // A move swaps the label it came from for the one it goes to.
    env.write_ok(WriteRequest::Move {
        messages: vec![env.id("msg-promo")],
        to: "Receipts".into(),
    })
    .await;
    let l = env.labels("msg-promo");
    assert!(has(&l, "Label_3") && !has(&l, "INBOX"), "{l:?}");
    assert_eq!(env.folders_of("msg-promo"), ["All Mail", "Receipts"]);

    // Delete is Trash, and moving out of Trash untrashes it.
    env.write_ok(WriteRequest::Delete {
        messages: vec![env.id("msg-photos")],
        permanent: false,
    })
    .await;
    assert!(has(&env.labels("msg-photos"), "TRASH"));
    assert_eq!(env.folders_of("msg-photos"), ["Trash"]);
    let refused = env
        .write(WriteRequest::Delete {
            messages: vec![env.id("msg-photos")],
            permanent: true,
        })
        .await;
    assert!(refused.is_err(), "{refused:?}");
    env.write_ok(WriteRequest::Move {
        messages: vec![env.id("msg-photos")],
        to: "inbox".into(),
    })
    .await;
    let l = env.labels("msg-photos");
    assert!(has(&l, "INBOX") && !has(&l, "TRASH"), "{l:?}");

    // Reporting junk is Spam.
    env.write_ok(WriteRequest::Report {
        messages: vec![env.id("msg-acme-3")],
        report: Report::Junk,
    })
    .await;
    let l = env.labels("msg-acme-3");
    assert!(has(&l, "SPAM") && !has(&l, "INBOX"), "{l:?}");
    assert_eq!(env.folders_of("msg-acme-3"), ["Spam"]);

    // A move Gmail refuses puts every label back.
    let before = env.folders_of("msg-acme-1");
    env.mock.push_fault(
        Fault::status(400, "invalidArgument", "no")
            .on_path("modify")
            .times(1),
    );
    let refused = env
        .write(WriteRequest::Move {
            messages: vec![env.id("msg-acme-1")],
            to: "Clients".into(),
        })
        .await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(env.folders_of("msg-acme-1"), before);
    env.daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn gmail_folders_are_labels() {
    let env = Env::start().await;
    env.first_sync().await;
    let folder = |name: &str| WriteRequest::FolderCreate {
        account: None,
        name: name.into(),
        parent: None,
    };
    env.write_ok(folder("Projects")).await;
    env.write_ok(WriteRequest::FolderCreate {
        account: None,
        name: "Kea".into(),
        parent: Some("Projects".into()),
    })
    .await;
    assert!(env.gmail_label("Projects") && env.gmail_label("Projects/Kea"));
    let refused = env
        .write(WriteRequest::FolderCreate {
            account: None,
            name: "Projects".into(),
            parent: Some("inbox".into()),
        })
        .await;
    assert!(refused.is_err(), "{refused:?}");

    // Renaming renames the sub-labels too.
    env.write_ok(WriteRequest::FolderRename {
        account: None,
        folder: "Projects".into(),
        name: "Work stuff".into(),
    })
    .await;
    assert!(env.gmail_label("Work stuff") && env.gmail_label("Work stuff/Kea"));
    assert!(!env.gmail_label("Projects/Kea"));

    // Deleting deletes the labels; the mail stays.
    env.write_ok(WriteRequest::Move {
        messages: vec![env.id("msg-receipt")],
        to: "Work stuff/Kea".into(),
    })
    .await;
    env.write_ok(WriteRequest::FolderDelete {
        account: None,
        folder: "Work stuff".into(),
        permanent: false,
    })
    .await;
    assert!(!env.gmail_label("Work stuff") && !env.gmail_label("Work stuff/Kea"));
    env.until("the label folders to go", |e| {
        let r = e.reader();
        !r.folders(e.account())
            .unwrap()
            .iter()
            .any(|f| f.name == "Work stuff" || f.name == "Kea")
    })
    .await;
    assert_eq!(env.folders_of("msg-receipt"), ["All Mail"]);
    env.daemon.stop().await;
}

fn to(addr: &str) -> Vec<Recipient> {
    vec![Recipient {
        name: String::new(),
        address: addr.into(),
    }]
}

fn compose(draft: ComposeRequest, send: bool) -> WriteRequest {
    WriteRequest::Compose {
        draft,
        send,
        send_delay_secs: 0,
        send_at: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn gmail_drafts_and_sending() {
    let env = Env::start().await;
    // A message with an inline image and an attachment, to reply to.
    let logo = familiar_provider_gmail::Outgoing {
        from: ("Ann".into(), "ann@acme.example".into()),
        to: vec![(String::new(), "kea@example.com".into())],
        subject: "Our new logo".into(),
        html: "<p>See <img src=\"cid:logo\"></p>".into(),
        parts: vec![
            familiar_provider_gmail::Part {
                name: "logo.png".into(),
                content_type: "image/png".into(),
                inline: true,
                content_id: Some("logo".into()),
                data: b"\x89PNG-logo".to_vec(),
            },
            familiar_provider_gmail::Part {
                name: "brief.pdf".into(),
                content_type: "application/pdf".into(),
                ..familiar_provider_gmail::Part::default()
            },
        ],
        ..familiar_provider_gmail::Outgoing::default()
    };
    env.mock
        .mailbox("default")
        .add_message(
            familiar_mock_gmail::Message::from_raw(logo.build())
                .id("msg-logo")
                .labels(&["INBOX"]),
        )
        .unwrap();
    env.first_sync().await;
    // The first sync may have listed the mailbox before it was added; the
    // history poll brings it in then.
    env.until("the logo message", |e| e.message("msg-logo").is_some())
        .await;
    let raw_of = |id: &str| {
        String::from_utf8_lossy(&env.mock.mailbox("default").message(id).unwrap().raw).into_owned()
    };

    // A new draft is saved to Gmail, and the local copy follows its ID.
    // (The fixture has a draft of its own.)
    let theirs = env.mock.mailbox("default").drafts();
    let ours = |e: &Env| -> Vec<(String, String)> {
        e.mock
            .mailbox("default")
            .drafts()
            .into_iter()
            .filter(|d| !theirs.iter().any(|t| t.0 == d.0))
            .collect()
    };
    let out = env
        .write(compose(
            ComposeRequest {
                subject: Some("Familiar Test draft".into()),
                html: "<p>First go</p>".into(),
                to: to("ann@acme.example"),
                ..ComposeRequest::default()
            },
            false,
        ))
        .await
        .unwrap();
    let draft = out.ops[0].item.unwrap();
    let drafts = ours(&env);
    assert_eq!(drafts.len(), 1);
    let local = |e: &Env| {
        e.reader()
            .message(familiar_store::MessageId(draft))
            .unwrap()
    };
    assert_eq!(local(&env).unwrap().remote_id, drafts[0].1);

    // An update replaces it whole and keeps what it doesn't change.
    env.write_ok(WriteRequest::DraftUpdate {
        message: draft,
        from: None,
        subject: Some("Familiar Test draft v2".into()),
        html: None,
        to: None,
        cc: None,
        bcc: None,
        attachments: vec![AttachmentData {
            name: "quote.pdf".into(),
            content_type: "application/pdf".into(),
            inline: false,
            content_id: None,
            content_b64: "JVBERi0xLjQ=".into(),
            size: 8,
        }],
        remove_attachments: vec![],
    })
    .await;
    let drafts = ours(&env);
    assert_eq!(drafts.len(), 1);
    let raw = raw_of(&drafts[0].1);
    assert!(raw.contains("Familiar Test draft v2"), "{raw}");
    assert!(
        raw.contains("First go") && raw.contains("quote.pdf"),
        "{raw}"
    );
    assert!(raw.contains("ann@acme.example"), "{raw}");
    assert_eq!(local(&env).unwrap().remote_id, drafts[0].1);

    // Sending sends that draft.
    env.write_ok(WriteRequest::Send {
        message: draft,
        delay_secs: 0,
        send_at: None,
    })
    .await;
    assert!(ours(&env).is_empty());
    let sent = env.mock.mailbox("default").sent();
    assert_eq!(sent.len(), 1);
    assert!(String::from_utf8_lossy(&sent[0].raw).contains("Familiar Test draft v2"));

    // A reply joins the original's thread.
    let dinner = env.mock.mailbox("default").message("msg-dinner").unwrap();
    env.write_ok(compose(
        ComposeRequest {
            respond_to: Some(env.id("msg-dinner")),
            response: Some(ResponseKind::Reply),
            html: "<p>Yes please</p>".into(),
            ..ComposeRequest::default()
        },
        true,
    ))
    .await;
    let sent = env.mock.mailbox("default").sent();
    let reply = sent.last().unwrap();
    assert_eq!(reply.thread_id, dinner.thread_id);
    assert!(String::from_utf8_lossy(&reply.raw).contains("In-Reply-To"));

    // A reply quoting an inline image carries the image, not the attachments.
    env.write_ok(compose(
        ComposeRequest {
            respond_to: Some(env.id("msg-logo")),
            response: Some(ResponseKind::Reply),
            html: "<p>Love it</p><blockquote><img src=\"cid:logo\"></blockquote>".into(),
            ..ComposeRequest::default()
        },
        true,
    ))
    .await;
    let sent = env.mock.mailbox("default").sent();
    let parts = familiar_provider_gmail::parts_of(&sent.last().unwrap().raw);
    assert_eq!(parts.len(), 1, "{parts:?}");
    assert_eq!(parts[0].content_id.as_deref(), Some("logo"));
    assert_eq!(parts[0].data, b"\x89PNG-logo");

    // A From Gmail hasn't verified is refused, not rewritten.
    let refused = env
        .write(compose(
            ComposeRequest {
                from: Some(Recipient {
                    name: String::new(),
                    address: "old-kea@example.com".into(),
                }),
                subject: Some("From the old address".into()),
                html: "<p>hi</p>".into(),
                to: to("ann@acme.example"),
                ..ComposeRequest::default()
            },
            true,
        ))
        .await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(env.mock.mailbox("default").sent().len(), 3);
    env.daemon.stop().await;
}
#[tokio::test(flavor = "multi_thread")]
async fn gmail_event_create_retry_edit_and_delete() {
    use familiar_core::events::{EventChanges, EventFields};
    use familiar_store::EventId;

    const EVENTS: &str = "/calendar/v3/calendars/cal-primary/events";
    let env = Env::start().await;
    env.first_sync().await;
    env.until("the calendar fixture", |e| {
        e.reader()
            .event_by_remote(e.account(), "event-october")
            .unwrap()
            .is_some()
    })
    .await;

    env.mock.push_fault(
        Fault::rate_limited()
            .on_method("POST")
            .on_path(EVENTS)
            .times(1),
    );
    let start = Utc.with_ymd_and_hms(2026, 10, 31, 1, 0, 0).unwrap();
    let out = env
        .write(WriteRequest::EventCreate {
            account: Some(KEY.into()),
            calendar_id: None,
            calendar: Some("Kea calendar".into()),
            from_mail: None,
            event: EventFields {
                subject: "Site visit".into(),
                location: "Field station".into(),
                start_at: start.timestamp(),
                end_at: start.timestamp() + 3600,
                time_zone: "Pacific/Auckland".into(),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    assert_eq!(out.ops[0].state, "done", "{out:?}");
    let item = EventId(out.ops[0].item.unwrap());
    let remote = env.reader().event(item).unwrap().unwrap().data.remote_id;
    let requests = env.mock.requests();
    let posts: Vec<_> = requests
        .iter()
        .filter(|r| r.method == "POST" && r.path == EVENTS)
        .collect();
    assert_eq!(
        posts.iter().map(|r| r.status).collect::<Vec<_>>(),
        [429, 200]
    );
    for post in posts {
        let event: serde_json::Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(event["id"], remote);
        assert_eq!(event["summary"], "Site visit");
        assert_eq!(event["location"], "Field station");
        for (field, instant) in [
            ("start", start.timestamp()),
            ("end", start.timestamp() + 3600),
        ] {
            let time = event[field]["dateTime"].as_str().unwrap();
            assert_eq!(
                chrono::DateTime::parse_from_rfc3339(time)
                    .unwrap()
                    .timestamp(),
                instant
            );
        }
    }

    env.write_ok(WriteRequest::EventUpdate {
        event: item.0,
        series: false,
        changes: EventChanges {
            subject: Some("Revised visit".into()),
            ..Default::default()
        },
    })
    .await;
    let path = format!("{EVENTS}/{remote}");
    let patch = env
        .mock
        .requests()
        .into_iter()
        .find(|r| r.method == "PATCH" && r.path == path)
        .unwrap();
    assert_eq!(patch.status, 200);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&patch.body).unwrap()["summary"],
        "Revised visit"
    );
    assert_eq!(
        env.reader().event(item).unwrap().unwrap().data.subject,
        "Revised visit"
    );

    env.write_ok(WriteRequest::EventDelete {
        event: item.0,
        series: false,
        comment: None,
    })
    .await;
    assert!(
        env.mock
            .requests()
            .iter()
            .any(|r| r.method == "DELETE" && r.path == path && r.status == 204)
    );
    assert!(env.reader().event(item).unwrap().is_none());
    env.daemon.stop().await;
}
