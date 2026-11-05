// SPDX-License-Identifier: MIT OR Apache-2.0
//! Synthetic mailboxes: deterministic, sized, and fully syncable through delta.

mod common;

use std::collections::HashSet;

use common::{Client, present};
use familiar_mock_graph::{Mailbox, MockGraph, SyntheticSpec};
use serde_json::Value;

#[test]
fn generation_is_deterministic() {
    let spec = SyntheticSpec::with_messages(500);
    assert_eq!(Mailbox::synthetic(&spec), Mailbox::synthetic(&spec));
    let other = SyntheticSpec { seed: 2, ..spec };
    assert_ne!(
        Mailbox::synthetic(&other),
        Mailbox::synthetic(&SyntheticSpec::with_messages(500))
    );
}

#[test]
fn small_fixture_parses() {
    let m = Mailbox::small();
    assert_eq!(m.messages.len(), 7);
    assert_eq!(m.user.mail, "pat@example.com");
}

#[tokio::test]
async fn full_sync_of_twenty_thousand_messages() {
    let n = 20_000;
    let mock = MockGraph::builder()
        .mailbox("big", Mailbox::synthetic(&SyntheticSpec::with_messages(n)))
        .start()
        .await
        .unwrap();
    let c = Client::new(mock.base_url(), "big-token")
        .prefer("odata.maxpagesize=1000")
        .prefer("IdType=\"ImmutableId\"");

    let folders = c.get_all("/me/mailFolders/delta").await;
    let mut seen = HashSet::new();
    let mut total_items = 0;
    let mut conversations: HashSet<String> = HashSet::new();
    for f in &folders {
        let id = f["id"].as_str().unwrap();
        let (items, _, _) = c
            .delta_round(&format!(
                "/me/mailFolders/{id}/messages/delta?$select=subject,conversationId,receivedDateTime"
            ))
            .await;
        assert_eq!(
            items.len() as u64,
            f["totalItemCount"].as_u64().unwrap(),
            "{}",
            f["displayName"]
        );
        total_items += items.len();
        for i in &items {
            conversations.insert(i["conversationId"].as_str().unwrap().into());
        }
        seen.extend(present(&items));
    }
    assert_eq!(total_items, n);
    assert_eq!(seen.len(), n, "every message once, in exactly one folder");
    assert!(conversations.len() < n, "replies thread into conversations");

    // Threads span folders.
    let by_conv = c
        .get_all("/me/messages?$top=1000&$select=conversationId,parentFolderId")
        .await;
    let mut folders_per_conv: std::collections::HashMap<&str, HashSet<&str>> = Default::default();
    for m in &by_conv {
        folders_per_conv
            .entry(m["conversationId"].as_str().unwrap())
            .or_default()
            .insert(m["parentFolderId"].as_str().unwrap());
    }
    assert!(folders_per_conv.values().any(|f| f.len() > 1));

    let events: Vec<Value> = c.get_all("/me/calendars/cal-family/events").await;
    // Twelve occurrences and their series master.
    assert_eq!(events.len(), 13);
}
