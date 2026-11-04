// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 1.6 against the mock: four in flight, Retry-After, user actions first,
//! token refresh.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{Env, client};
use familiar_mock_graph::{Fault, Mailbox, MockGraph, SyntheticSpec};
use familiar_provider_graph::{
    ClientOptions, GraphClient, MailApi, Priority, TokenSource, messages_resource,
};
use familiar_store::MessageId;

async fn synthetic(messages: usize, latency: Duration) -> MockGraph {
    MockGraph::builder()
        .mailbox(
            "default",
            Mailbox::synthetic(&SyntheticSpec::with_messages(messages)),
        )
        .latency(latency)
        .start()
        .await
        .unwrap()
}

fn throttled(mock: &MockGraph) -> usize {
    mock.requests().iter().filter(|r| r.status == 429).count()
}

/// Message IDs from `/me/messages`.
async fn some_ids(api: &MailApi, n: usize) -> Vec<String> {
    let v = api
        .client()
        .get(
            &format!("/me/messages?$top={n}&$select=id"),
            Priority::Background,
            &[],
        )
        .await
        .unwrap();
    v["value"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn never_more_than_four_in_flight() {
    let mock = synthetic(300, Duration::from_millis(15)).await;
    let mut env = Env::new(mock, "default-token");
    env.sync_tree().await;
    let folders: Vec<String> = env.folders().into_iter().map(|f| f.remote_id).collect();
    let refs: Vec<&str> = folders.iter().map(String::as_str).collect();
    env.sync_folders(&refs).await;

    let ids: Vec<MessageId> = env
        .reader()
        .messages(env.folder("fld-inbox").id, None, 100)
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert!(ids.len() >= 20, "inbox has {}", ids.len());
    let results = env
        .sync
        .fetch_bodies(&mut env.w, &ids, Priority::Background)
        .await
        .unwrap();
    assert_eq!(results.len(), ids.len());
    assert!(results.iter().all(|(_, r)| r.is_ok()));
    for id in &ids {
        assert!(env.reader().message(*id).unwrap().unwrap().body.is_some());
    }

    assert_eq!(
        throttled(&env.mock),
        0,
        "the mock's concurrency limit was never hit"
    );
    assert_eq!(env.mock.mailbox("default").peak_in_flight(), 4);
    assert_eq!(
        env.sync.api().client().scheduler().stats().peak_in_flight,
        4
    );
}

#[tokio::test]
async fn retry_after_is_honoured_and_sync_completes() {
    let mut env = Env::new(MockGraph::start().await.unwrap(), "test-token");
    env.sync_tree().await;
    env.mock
        .push_fault(Fault::throttle(1).on_path("/messages/delta").times(2));
    let start = Instant::now();
    let r = env.sync_folders(&["fld-inbox"]).await;
    assert!(
        start.elapsed() >= Duration::from_millis(1900),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(r.upserted, 2);
    assert_eq!(throttled(&env.mock), 2);
    assert!(env.state(&messages_resource("fld-inbox")).is_some());
    assert_eq!(env.sync.api().client().scheduler().stats().pauses, 2);
}

#[tokio::test]
async fn a_throttle_pauses_the_whole_mailbox() {
    let mock = MockGraph::start().await.unwrap();
    let api = MailApi::new(client(&mock, "test-token"));
    mock.push_fault(
        Fault::throttle(2)
            .on_path("/messages/imm-small-01")
            .times(1),
    );
    let start = Instant::now();
    let a = {
        let api = api.clone();
        tokio::spawn(async move { api.body("imm-small-01", Priority::Background).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    // A user request made during the pause waits for Retry-After too.
    api.body("imm-small-05", Priority::User).await.unwrap();
    let waited = start.elapsed();
    assert!(waited >= Duration::from_millis(1900), "{waited:?}");
    a.await.unwrap().unwrap();
    let reqs = mock.requests();
    let first_05 = reqs
        .iter()
        .position(|r| r.path.ends_with("imm-small-05"))
        .unwrap();
    assert_eq!(
        reqs[..first_05].iter().filter(|r| r.status == 429).count(),
        1
    );
}

#[tokio::test]
async fn a_request_budget_is_ridden_out() {
    let mock = MockGraph::builder()
        .request_budget(6, Duration::from_secs(1))
        .start()
        .await
        .unwrap();
    let mut env = Env::new(mock, "default-token");
    env.sync_tree().await;
    env.sync_all().await;
    assert!(throttled(&env.mock) > 0, "the budget was hit");
    let server = env.mock.mailbox("default").message_count();
    let local: i64 = env.folders().iter().map(|f| f.total).sum();
    // The hidden folder's messages aren't synced.
    assert!(
        local as usize >= server - 1 && local > 0,
        "{local} of {server}"
    );
}

#[tokio::test]
async fn user_requests_overtake_queued_background_work() {
    let mock = synthetic(100, Duration::from_millis(40)).await;
    let api = MailApi::new(client(&mock, "default-token"));
    let ids = some_ids(&api, 41).await;
    let done = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for id in &ids[..40] {
        let (api, id, done) = (api.clone(), id.clone(), done.clone());
        tasks.push(tokio::spawn(async move {
            api.body(&id, Priority::Background).await.unwrap();
            done.fetch_add(1, Ordering::SeqCst);
        }));
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(api.client().scheduler().stats().queued[1] >= 30);
    api.body(&ids[40], Priority::User).await.unwrap();
    let before_user = done.load(Ordering::SeqCst);
    // Four were already running; the user's request took the next free slot.
    assert!(
        before_user <= 8,
        "{before_user} background requests finished first"
    );
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(done.load(Ordering::SeqCst), 40);
}

/// Hands out a token the mock doesn't know until asked to refresh.
struct Stale {
    refreshed: AtomicUsize,
}

impl TokenSource for Stale {
    fn access_token(&self) -> Result<String, String> {
        Ok("stale-token".into())
    }
    fn refresh(&self) -> Result<String, String> {
        self.refreshed.fetch_add(1, Ordering::SeqCst);
        Ok("test-token".into())
    }
}

#[tokio::test]
async fn a_401_refreshes_the_token_once() {
    let mock = MockGraph::start().await.unwrap();
    let tokens = Arc::new(Stale {
        refreshed: AtomicUsize::new(0),
    });
    let c = GraphClient::new(ClientOptions::with_base(mock.base_url()), tokens.clone()).unwrap();
    let api = MailApi::new(c);
    api.body("imm-small-01", Priority::User).await.unwrap();
    api.body("imm-small-05", Priority::User).await.unwrap();
    assert_eq!(
        tokens.refreshed.load(Ordering::SeqCst),
        1,
        "new token is reused"
    );

    // A token that stays bad gives an auth error, not a loop.
    mock.expire_token("test-token");
    let err = api.body("imm-small-01", Priority::User).await.unwrap_err();
    assert!(
        matches!(err, familiar_provider_graph::Error::Auth(_)),
        "{err}"
    );
}
