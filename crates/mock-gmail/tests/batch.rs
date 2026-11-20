// SPDX-License-Identifier: MIT OR Apache-2.0
//! `POST /batch/gmail/v1`: parts served in order, each with its own faults and quota.

mod common;

use common::Client;
use familiar_mock_gmail::{Fault, MockGmail};

#[tokio::test]
async fn batch_of_metadata_gets() {
    let mock = MockGmail::start().await.unwrap();
    let c = Client::new(&mock.origin(), "test-token");
    let (r, parts) = c
        .batch(&[
            (
                "GET",
                "/gmail/v1/users/me/messages/msg-dinner?format=metadata",
            ),
            (
                "GET",
                "https://gmail.googleapis.com/gmail/v1/users/me/messages/msg-photos?format=minimal",
            ),
            ("GET", "/gmail/v1/users/me/messages/nope?format=metadata"),
        ])
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(r.text.contains("Content-ID: <response-item0>"));
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0].0, 200);
    assert_eq!(parts[0].1["id"], "msg-dinner");
    assert!(parts[0].1["payload"]["headers"].is_array());
    assert_eq!(parts[1].1["id"], "msg-photos");
    assert_eq!(parts[2].0, 404);

    let reqs = mock.requests();
    assert_eq!(reqs.iter().filter(|r| r.in_batch).count(), 3);
    assert_eq!(mock.mailbox("default").units_used(), 15);
}

#[tokio::test]
async fn parts_fail_on_their_own() {
    let mock = MockGmail::start().await.unwrap();
    let c = Client::new(&mock.origin(), "test-token");
    mock.push_fault(Fault::rate_limited().on_path("/msg-photos"));
    let (_, parts) = c
        .batch(&[
            (
                "GET",
                "/gmail/v1/users/me/messages/msg-dinner?format=minimal",
            ),
            (
                "GET",
                "/gmail/v1/users/me/messages/msg-photos?format=minimal",
            ),
        ])
        .await;
    assert_eq!(parts[0].0, 200);
    assert_eq!(parts[1].0, 429);
    assert_eq!(
        parts[1].1["error"]["errors"][0]["reason"],
        "rateLimitExceeded"
    );
}

#[tokio::test]
async fn more_than_100_parts_is_refused() {
    let mock = MockGmail::start().await.unwrap();
    let c = Client::new(&mock.origin(), "test-token");
    let many: Vec<(&str, &str)> = (0..101)
        .map(|_| {
            (
                "GET",
                "/gmail/v1/users/me/messages/msg-dinner?format=minimal",
            )
        })
        .collect();
    let (r, _) = c.batch(&many).await;
    assert_eq!(r.status, 400);
    let (r, parts) = c.batch(&many[..100]).await;
    assert_eq!(r.status, 200);
    assert_eq!(parts.len(), 100);
}

#[tokio::test]
async fn batch_without_a_token_fails_each_part() {
    let mock = MockGmail::start().await.unwrap();
    let c = Client::new(&mock.origin(), "");
    let (r, _) = c.batch(&[("GET", "/gmail/v1/users/me/profile")]).await;
    assert_eq!(r.status, 401);
}
