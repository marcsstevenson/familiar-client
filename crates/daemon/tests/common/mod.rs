// SPDX-License-Identifier: MIT OR Apache-2.0
//! Test harness: a mock Graph with a synthetic mailbox, and famd on a store
//! in a temp dir with short poll intervals and a clock the test controls.
#![allow(dead_code, missing_docs)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use familiar_core::daemon::{DaemonPaths, FIRST_SYNC_RESOURCE, SyncPhase};
use familiar_daemon::policy::{DAY, SyncPolicy};
use familiar_daemon::{AccountSpec, Daemon, DaemonOptions};
use familiar_mock_graph::{Mailbox, MockGraph, SyntheticSpec};
use familiar_provider_graph::{ClientOptions, GraphClient, StaticToken};
use familiar_store::{AccountId, Cursor, Reader};

pub const KEY: &str = "synthetic";

pub struct Env {
    pub mock: MockGraph,
    pub dir: tempfile::TempDir,
    pub paths: DaemonPaths,
    pub now: Arc<AtomicI64>,
    pub daemon: Option<Daemon>,
    pub spec: SyntheticSpec,
}

/// A quick policy: polls every few hundred milliseconds, 30-day window.
pub fn fast_policy() -> SyncPolicy {
    SyncPolicy {
        window_days: 30,
        inbox_interval: Duration::from_millis(200),
        folder_interval: Duration::from_millis(400),
        tree_interval: Duration::from_millis(400),
        ageing_interval: Duration::from_millis(300),
        backoff_min: Duration::from_millis(100),
        backoff_max: Duration::from_millis(500),
        ..SyncPolicy::default()
    }
}

impl Env {
    pub async fn new(messages: usize) -> Self {
        let spec = SyntheticSpec {
            folders: 8,
            ..SyntheticSpec::with_messages(messages)
        };
        let mock = MockGraph::builder()
            .mailbox(KEY, Mailbox::synthetic(&spec))
            .start()
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let paths = DaemonPaths::at(dir.path());
        // One day after the newest message.
        let now = Arc::new(AtomicI64::new(spec.newest.timestamp() + DAY));
        Self {
            mock,
            dir,
            paths,
            now,
            daemon: None,
            spec,
        }
    }

    pub fn client(&self) -> GraphClient {
        GraphClient::new(
            ClientOptions::with_base(self.mock.base_url()),
            Arc::new(StaticToken(format!("{KEY}-token"))),
        )
        .unwrap()
    }

    pub fn options(&self, policy: SyncPolicy) -> DaemonOptions {
        let now = self.now.clone();
        let mut o = DaemonOptions::new(
            self.paths.clone(),
            vec![AccountSpec {
                key: KEY.into(),
                email: "pat@example.com".into(),
                display_name: "Pat".into(),
                tenant_id: None,
                client: self.client(),
            }],
        );
        o.policy = policy;
        o.clock = Arc::new(move || now.load(Ordering::SeqCst));
        o
    }

    pub fn start(&mut self, policy: SyncPolicy) {
        self.start_with(self.options(policy));
    }

    pub fn start_with(&mut self, options: DaemonOptions) {
        self.daemon = Some(Daemon::start(options).unwrap());
    }

    pub async fn stop(&mut self) {
        if let Some(d) = self.daemon.take() {
            d.stop().await;
        }
    }

    pub fn daemon(&self) -> &Daemon {
        self.daemon.as_ref().unwrap()
    }

    pub fn reader(&self) -> Reader {
        Reader::open(&self.paths.store).unwrap()
    }

    pub fn account(&self) -> AccountId {
        self.reader().account_by_key(KEY).unwrap().unwrap().id
    }

    pub fn window_start(&self, days: u32) -> i64 {
        self.now.load(Ordering::SeqCst) - i64::from(days) * DAY
    }

    pub fn phase(&self) -> SyncPhase {
        self.daemon().service().status().accounts[0].phase
    }

    /// Waits until `f` holds, failing after `limit`.
    pub async fn until(&self, what: &str, limit: Duration, mut f: impl FnMut(&Self) -> bool) {
        let start = Instant::now();
        loop {
            if f(self) {
                return;
            }
            if start.elapsed() > limit {
                let st = self.daemon.as_ref().map(|d| d.service().status());
                panic!("timed out waiting for {what}; daemon: {st:?}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// First sync done, idle, and no window body missing.
    pub fn settled(&self, window_days: u32) -> bool {
        let Ok(r) = Reader::open(&self.paths.store) else {
            return false;
        };
        let Ok(Some(acc)) = r.account_by_key(KEY) else {
            return false;
        };
        let first = r.sync_state(acc.id, FIRST_SYNC_RESOURCE).unwrap().is_some();
        let stats = r
            .body_stats(acc.id, self.window_start(window_days))
            .unwrap();
        first && stats.window_missing == 0 && self.phase() == SyncPhase::Idle
    }

    /// Every difference between the mock mailbox and the store, as text
    /// (at most 20). Empty means consistent.
    pub fn differences(&self, window_days: u32) -> Vec<String> {
        let mut out = Vec::new();
        let r = self.reader();
        let acc = self.account();
        let mbx = self.mock.mailbox(KEY);
        let since = self.window_start(window_days);
        let mut local_total = 0;
        for f in r.folders(acc).unwrap() {
            let remote: Vec<String> = mbx.message_ids(&f.remote_id);
            // remote id -> (read, has body, pinned or cached, received)
            let mut local: HashMap<String, (bool, bool, bool, i64)> = HashMap::new();
            let mut after = None;
            loop {
                let page = r.messages(f.id, after, 5000).unwrap();
                let Some(last) = page.last() else { break };
                after = Some(Cursor {
                    received_at: last.received_at,
                    id: last.id.0,
                });
                for m in page {
                    local.insert(
                        m.remote_id.clone(),
                        (
                            m.is_read,
                            m.body.is_some(),
                            m.body_pinned || m.body_cached_at.is_some(),
                            m.received_at,
                        ),
                    );
                }
            }
            local_total += local.len();
            if f.total as usize != local.len() {
                out.push(format!(
                    "{}: counter {} but {} rows",
                    f.name,
                    f.total,
                    local.len()
                ));
            }
            let remote_set: HashSet<&String> = remote.iter().collect();
            for id in &remote {
                match local.get(id) {
                    None => out.push(format!("{}: {id} missing locally", f.name)),
                    Some((read, _, _, _)) => {
                        let m = mbx.message(id).unwrap();
                        if m.is_read != *read {
                            out.push(format!("{}: {id} read state differs", f.name));
                        }
                    }
                }
            }
            for (id, (_, has_body, kept, at)) in &local {
                if !remote_set.contains(id) {
                    out.push(format!("{}: {id} is local but not on the server", f.name));
                }
                if *at >= since && !has_body {
                    out.push(format!("{}: {id} is in the window without a body", f.name));
                }
                if *at < since && *has_body && !kept {
                    out.push(format!(
                        "{}: {id} is outside the window with a body",
                        f.name
                    ));
                }
            }
        }
        if local_total != mbx.message_count() {
            out.push(format!(
                "store has {local_total} messages, server {}",
                mbx.message_count()
            ));
        }
        out.truncate(20);
        out
    }
}
