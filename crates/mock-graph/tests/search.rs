// SPDX-License-Identifier: MIT OR Apache-2.0
//! `$search`: matching, the 1,000-result cap, paging and the combinations Graph refuses.

mod common;

use common::Client;
use familiar_mock_graph::{Mailbox, MockGraph, SyntheticSpec};

#[tokio::test]
async fn search_matches_subject_body_and_people() {
    let mock = MockGraph::start().await.unwrap();
    let c = Client::new(mock.base_url(), "test-token");

    let r = c.get_all("/me/messages?$search=%22practice%22").await;
    assert_eq!(r.len(), 3, "subject matches across folders");

    let body = c
        .get_all("/me/messages?$search=%22body:attached%20order%22")
        .await;
    assert_eq!(body.len(), 1);
    assert_eq!(body[0]["subject"], "Your order has shipped");

    let from = c
        .get_all("/me/messages?$search=%22from:casey.chen%22")
        .await;
    assert_eq!(from.len(), 1);

    let two = c
        .get_all("/me/messages?$search=%22membership%202015%22")
        .await;
    assert_eq!(
        two.len(),
        1,
        "all terms must match; finds pre-2015 archive mail"
    );

    let folder = c
        .get_all("/me/mailFolders/inbox/messages?$search=%22practice%22")
        .await;
    assert_eq!(folder.len(), 1);

    assert!(
        c.get_all("/me/messages?$search=%22zebra%22")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn search_refuses_filter_orderby_and_skip() {
    let mock = MockGraph::start().await.unwrap();
    let c = Client::new(mock.base_url(), "test-token");
    for q in [
        "$search=%22practice%22&$filter=isRead%20eq%20true",
        "$search=%22practice%22&$orderby=receivedDateTime%20desc",
        "$search=%22practice%22&$skip=10",
    ] {
        let r = c.get(&format!("/me/messages?{q}")).await;
        assert_eq!(r.status, 400, "{q}");
        assert_eq!(r.error_code(), "ErrorInvalidUrlQuery");
    }
}

#[tokio::test]
async fn search_caps_at_one_thousand_results() {
    let mock = MockGraph::builder()
        .mailbox(
            "big",
            Mailbox::synthetic(&SyntheticSpec::with_messages(3_000)),
        )
        .start()
        .await
        .unwrap();
    let c = Client::new(mock.base_url(), "big-token");
    // Every synthetic message is to or from the test user.
    let first = c.get("/me/messages?$search=%22test.user%22&$top=250").await;
    assert_eq!(first.values().len(), 250);
    assert!(
        first.body["@odata.nextLink"]
            .as_str()
            .unwrap()
            .contains("skiptoken")
    );
    let all = c
        .get_all("/me/messages?$search=%22test.user%22&$top=250")
        .await;
    assert_eq!(all.len(), 1_000);
}
