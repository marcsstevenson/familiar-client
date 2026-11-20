// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail accounts in famd (Gmail plan WP G.5) against the mock: first sync,
//! bodies for the window, changes by history, server search, `fam api`,
//! and writes refused until Gmail writes land.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use familiar_core::commands::write::{WriteFlags, submit_at};
use familiar_core::daemon::{
    DaemonPaths, FIRST_SYNC_RESOURCE, GraphGetParams, METHOD_SEARCH_ONLINE, SearchOnlineResult,
};
use familiar_core::ops::WriteRequest;
use familiar_daemon::policy::SyncPolicy;
use familiar_daemon::{AccountSpec, Daemon, DaemonOptions};
use familiar_mock_gmail::MockGmail;
use familiar_provider_gmail::{ClientOptions, GmailClient, StaticToken};
use familiar_store::{AccountId, OpState, Reader};
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
    env.until("the first sync", |e| {
        e.reader()
            .account_by_key(KEY)
            .unwrap()
            .and_then(|a| e.reader().sync_state(a.id, FIRST_SYNC_RESOURCE).unwrap())
            .is_some_and(|s| s.synced_at.is_some())
    })
    .await;
    let acc = env.account();
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

    // Writes are refused for now, and the local change is undone.
    let photos = env.message("msg-photos").unwrap();
    let paths = env.paths.clone();
    let id = photos.id.0;
    let out = tokio::task::spawn_blocking(move || {
        submit_at(
            &paths,
            WriteRequest::Flag {
                messages: vec![id],
                flagged: false,
            },
            &WriteFlags {
                wait: true,
                wait_secs: Some(10),
                ..WriteFlags::default()
            },
        )
    })
    .await
    .unwrap();
    let _ = out;
    env.until("the refused op", |e| {
        e.reader()
            .recent_ops(0, 50)
            .unwrap()
            .iter()
            .any(|o| o.account == acc && o.state == OpState::Failed)
    })
    .await;
    assert!(env.message("msg-photos").unwrap().is_flagged);
    env.daemon.stop().await;
}
