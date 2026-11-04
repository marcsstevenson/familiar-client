// SPDX-License-Identifier: MIT OR Apache-2.0
//! Throttling (429 + Retry-After), outages, concurrency, budgets and auth errors.

mod common;

use std::time::Duration;

use common::Client;
use familiar_mock_graph::{Fault, Mailbox, MockGraph};

#[tokio::test]
async fn throttle_then_succeed() {
    let mock = MockGraph::start().await.unwrap();
    let c = Client::new(mock.base_url(), "test-token");
    mock.push_fault(Fault::throttle(3).on_path("/messages").times(2));

    let r = c.get("/me/messages").await;
    assert_eq!(r.status, 429);
    assert_eq!(r.header("retry-after"), Some("3"));
    assert_eq!(r.error_code(), "TooManyRequests");
    assert!(r.body["error"]["innerError"]["request-id"].is_string());

    // Other paths are not affected.
    assert_eq!(c.get("/me/mailFolders").await.status, 200);

    let (r, retries) = c.get_retrying("/me/messages").await;
    assert_eq!(r.status, 200);
    assert_eq!(retries, 1, "one throttled answer was left");
    let statuses: Vec<u16> = mock.requests().iter().map(|r| r.status).collect();
    assert_eq!(statuses, [429, 200, 429, 200]);
}

#[tokio::test]
async fn service_unavailable_and_generic_errors() {
    let mock = MockGraph::start().await.unwrap();
    let c = Client::new(mock.base_url(), "test-token");
    mock.push_fault(Fault::service_unavailable(1).times(1));
    let r = c.get("/me").await;
    assert_eq!(r.status, 503);
    assert_eq!(r.header("retry-after"), Some("1"));
    assert_eq!(c.get("/me").await.status, 200);

    mock.push_fault(Fault::status(500, "generalException", "boom").on_path("/calendars"));
    assert_eq!(
        c.get("/me/calendars").await.error_code(),
        "generalException"
    );
    assert_eq!(
        c.get("/me/calendars").await.status,
        500,
        "no .times() fires forever"
    );
    mock.clear_faults();
    assert_eq!(c.get("/me/calendars").await.status, 200);

    mock.push_fault(
        Fault::access_denied()
            .on_path("/mailboxSettings")
            .on_method("GET"),
    );
    let r = c.get("/me/mailboxSettings").await;
    assert_eq!(r.status, 403);
    assert_eq!(r.error_code(), "ErrorAccessDenied");
}

#[tokio::test]
async fn auth_errors() {
    let mock = MockGraph::start().await.unwrap();

    let none = Client::new(mock.base_url(), "").get("/me").await;
    assert_eq!(none.status, 401);
    assert_eq!(none.error_code(), "InvalidAuthenticationToken");
    assert!(
        none.header("www-authenticate")
            .unwrap()
            .starts_with("Bearer")
    );

    let unknown = Client::new(mock.base_url(), "made-up").get("/me").await;
    assert_eq!(unknown.status, 401);

    let c = Client::new(mock.base_url(), "test-token");
    assert_eq!(c.get("/me").await.status, 200);
    mock.expire_token("test-token");
    let expired = c.get("/me").await;
    assert_eq!(expired.status, 401);
    assert!(
        expired.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("expired")
    );

    // A refresh issues a new token, which works.
    mock.add_token("refreshed-token", "default");
    assert_eq!(
        Client::new(mock.base_url(), "refreshed-token")
            .get("/me")
            .await
            .status,
        200
    );

    // An injected expiry mid-sync, for one request only.
    let fresh = Client::new(mock.base_url(), "refreshed-token");
    mock.push_fault(Fault::auth_expired().on_path("/delta").times(1));
    assert_eq!(
        fresh
            .get("/me/mailFolders/inbox/messages/delta")
            .await
            .status,
        401
    );
    assert_eq!(
        fresh
            .get("/me/mailFolders/inbox/messages/delta")
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn tokens_pick_the_mailbox() {
    let mut other = Mailbox::small();
    other.user.user_principal_name = "pat@club.example".into();
    other.user.mail = "pat@club.example".into();
    other.messages.truncate(1);
    let mock = MockGraph::builder()
        .mailbox("hotmail", Mailbox::small())
        .mailbox("parklands", other)
        .start()
        .await
        .unwrap();
    let h = Client::new(mock.base_url(), "hotmail-token");
    let p = Client::new(mock.base_url(), "parklands-token");
    assert_eq!(h.get_all("/me/messages").await.len(), 7);
    assert_eq!(p.get_all("/me/messages").await.len(), 1);
    assert_eq!(p.get("/me").await.body["mail"], "pat@club.example");
    let mailboxes: Vec<_> = mock
        .requests()
        .into_iter()
        .filter_map(|r| r.mailbox)
        .collect();
    assert!(mailboxes.contains(&"hotmail".into()) && mailboxes.contains(&"parklands".into()));
}

#[tokio::test]
async fn more_than_four_concurrent_requests_are_throttled() {
    let mock = MockGraph::builder()
        .latency(Duration::from_millis(150))
        .start()
        .await
        .unwrap();
    let c = std::sync::Arc::new(Client::new(mock.base_url(), "test-token"));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let c = c.clone();
        tasks.push(tokio::spawn(async move { c.get("/me/messages").await }));
    }
    let mut ok = 0;
    let mut throttled = 0;
    for t in tasks {
        let r = t.await.unwrap();
        match r.status {
            200 => ok += 1,
            429 => {
                assert_eq!(r.error_code(), "ApplicationThrottled");
                assert!(
                    r.body["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("MailboxConcurrency")
                );
                assert!(r.header("retry-after").is_some());
                throttled += 1;
            }
            s => panic!("unexpected {s}"),
        }
    }
    assert_eq!(ok, 4);
    assert_eq!(throttled, 4);
    assert_eq!(mock.mailbox("default").peak_in_flight(), 4);

    // A client that keeps to four never sees a 429.
    mock.clear_requests();
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let (c, sem) = (c.clone(), sem.clone());
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            c.get("/me/messages").await.status
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap(), 200);
    }
}

#[tokio::test]
async fn request_budget_and_delays() {
    let mock = MockGraph::builder()
        .request_budget(3, Duration::from_secs(600))
        .start()
        .await
        .unwrap();
    let c = Client::new(mock.base_url(), "test-token");
    for _ in 0..3 {
        assert_eq!(c.get("/me").await.status, 200);
    }
    let r = c.get("/me").await;
    assert_eq!(r.status, 429);
    let wait: u64 = r.header("retry-after").unwrap().parse().unwrap();
    assert!((590..=600).contains(&wait), "rest of the window: {wait}");

    let mock = MockGraph::start().await.unwrap();
    let c = Client::new(mock.base_url(), "test-token");
    mock.push_fault(Fault::delay(Duration::from_millis(200)).times(1));
    let t = std::time::Instant::now();
    assert_eq!(c.get("/me").await.status, 200);
    assert!(t.elapsed() >= Duration::from_millis(200));
}
