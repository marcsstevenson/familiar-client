// SPDX-License-Identifier: MIT OR Apache-2.0
//! Recorded exchanges replay ahead of the model, with links rewritten to the mock.

mod common;

use common::Client;
use familiar_mock_graph::{MockGraph, RecordedExchange, Recording};

#[tokio::test]
async fn sample_recording_replays_with_paging() {
    let mock = MockGraph::builder()
        .recording(Recording::sample())
        .start()
        .await
        .unwrap();
    let c = Client::new(mock.base_url(), "test-token");
    let first = c.get("/me/people").await;
    assert_eq!(first.status, 200);
    let next = first.body["@odata.nextLink"].as_str().unwrap();
    assert!(
        next.starts_with(mock.base_url()),
        "{{{{baseUrl}}}} is rewritten: {next}"
    );
    let people = c.get_all("/me/people").await;
    assert_eq!(people.len(), 3);

    let auto = c.get("/me/mailboxSettings/automaticRepliesSetting").await;
    assert_eq!(auto.body["status"], "disabled");
    // Recorded responses still need a valid token.
    assert_eq!(
        Client::new(mock.base_url(), "")
            .get("/me/people")
            .await
            .status,
        401
    );
}

#[tokio::test]
async fn recordings_override_the_model_and_carry_status_and_headers() {
    let rec = Recording {
        exchanges: vec![RecordedExchange {
            method: "GET".into(),
            path: "/v1.0/me/mailFolders/inbox".into(),
            query: Default::default(),
            mailbox: None,
            status: 404,
            headers: [("x-ms-ags-diagnostic".to_string(), "recorded".to_string())].into(),
            body: serde_json::json!({ "error": { "code": "ErrorItemNotFound", "message": "recorded" } }),
        }],
    };
    let json = serde_json::to_string(&rec).unwrap();
    let mock = MockGraph::builder()
        .recording(Recording::from_json(&json).unwrap())
        .start()
        .await
        .unwrap();
    let c = Client::new(mock.base_url(), "test-token");
    let r = c.get("/me/mailFolders/inbox").await;
    assert_eq!(r.status, 404);
    assert_eq!(r.header("x-ms-ags-diagnostic"), Some("recorded"));
    assert_eq!(c.get("/me/mailFolders/drafts").await.status, 200);
}
