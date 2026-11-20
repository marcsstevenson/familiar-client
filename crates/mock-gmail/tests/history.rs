// SPDX-License-Identifier: MIT OR Apache-2.0
//! `history.list`: records for each change, filters, paging and expiry.

mod common;

use common::Client;
use familiar_mock_gmail::{Message, MockGmail};
use serde_json::{Value, json};

async fn setup() -> (MockGmail, Client, u64) {
    let mock = MockGmail::start().await.unwrap();
    let c = Client::new(&mock.origin(), "test-token");
    let start = c.ok("profile").await["historyId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    (mock, c, start)
}

fn kinds(h: &Value) -> Vec<String> {
    h["history"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            [
                "messagesAdded",
                "messagesDeleted",
                "labelsAdded",
                "labelsRemoved",
            ]
            .into_iter()
            .find(|k| r.get(*k).is_some())
            .unwrap()
            .to_string()
        })
        .collect()
}

#[tokio::test]
async fn changes_show_up_once_in_order() {
    let (mock, c, start) = setup().await;
    let h = c.ok(&format!("history?startHistoryId={start}")).await;
    assert!(h.get("history").is_none(), "fixture loading is not history");
    assert_eq!(h["historyId"], start.to_string());

    let mb = mock.mailbox("default");
    let new = mb
        .add_message(Message::new("zed@example.com", "me", "New one", "Hello"))
        .unwrap();
    mb.modify("msg-dinner", &["STARRED"], &["UNREAD"]).unwrap();
    mb.delete_message("msg-promo").unwrap();

    let h = c.ok(&format!("history?startHistoryId={start}")).await;
    assert_eq!(
        kinds(&h),
        [
            "messagesAdded",
            "labelsAdded",
            "labelsRemoved",
            "messagesDeleted"
        ]
    );
    let recs = h["history"].as_array().unwrap();
    assert_eq!(recs[0]["messagesAdded"][0]["message"]["id"], new.as_str());
    assert_eq!(
        recs[0]["messagesAdded"][0]["message"]["labelIds"],
        json!(["INBOX", "UNREAD"])
    );
    assert_eq!(recs[1]["labelsAdded"][0]["labelIds"], json!(["STARRED"]));
    assert_eq!(recs[2]["labelsRemoved"][0]["labelIds"], json!(["UNREAD"]));
    assert_eq!(recs[3]["messagesDeleted"][0]["message"]["id"], "msg-promo");
    let latest: u64 = h["historyId"].as_str().unwrap().parse().unwrap();
    assert_eq!(latest, mb.history_id());

    // From the latest history ID there is nothing new.
    let h = c.ok(&format!("history?startHistoryId={latest}")).await;
    assert!(h.get("history").is_none());
    // The message's own historyId is its last change.
    let m = c.ok("messages/msg-dinner?format=minimal").await;
    assert_eq!(m["historyId"], recs[2]["id"]);
}

#[tokio::test]
async fn filters_and_paging() {
    let (mock, c, start) = setup().await;
    let mb = mock.mailbox("default");
    mb.modify("msg-dinner", &[], &["INBOX"]).unwrap(); // archive
    mb.modify("msg-receipt", &["STARRED"], &[]).unwrap();
    for i in 0..5 {
        mb.add_message(Message::new("a@example.com", "me", &format!("n{i}"), "x"))
            .unwrap();
    }

    // labelId=INBOX still sees the archive, which removed INBOX.
    let h = c
        .ok(&format!("history?startHistoryId={start}&labelId=INBOX"))
        .await;
    assert_eq!(kinds(&h)[0], "labelsRemoved");
    assert_eq!(kinds(&h).len(), 6);

    let h = c
        .ok(&format!(
            "history?startHistoryId={start}&historyTypes=labelAdded"
        ))
        .await;
    assert_eq!(kinds(&h), ["labelsAdded"]);

    let mut seen = 0;
    let mut token = String::new();
    loop {
        let page = if token.is_empty() {
            String::new()
        } else {
            format!("&pageToken={token}")
        };
        let h = c
            .ok(&format!(
                "history?startHistoryId={start}&maxResults=2{page}"
            ))
            .await;
        seen += h["history"].as_array().unwrap().len();
        match h["nextPageToken"].as_str() {
            Some(t) => token = t.to_string(),
            None => break,
        }
    }
    assert_eq!(seen, 7);
}

#[tokio::test]
async fn expired_history_is_404() {
    let (mock, c, start) = setup().await;
    let mb = mock.mailbox("default");
    mb.modify("msg-dinner", &["STARRED"], &[]).unwrap();
    mb.expire_history();
    let r = c.get(&format!("history?startHistoryId={start}")).await;
    assert_eq!(r.status, 404);
    assert_eq!(r.reason(), "notFound");
    assert_eq!(r.body["error"]["status"], "NOT_FOUND");

    // A full resync records the profile's history ID and carries on from there.
    let now = c.ok("profile").await["historyId"]
        .as_str()
        .unwrap()
        .to_string();
    mb.modify("msg-dinner", &[], &["STARRED"]).unwrap();
    let h = c.ok(&format!("history?startHistoryId={now}")).await;
    assert_eq!(kinds(&h), ["labelsRemoved"]);

    assert_eq!(c.get("history").await.status, 400);
    assert_eq!(c.get("history?startHistoryId=abc").await.status, 400);
}
