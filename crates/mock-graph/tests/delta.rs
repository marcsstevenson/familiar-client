// SPDX-License-Identifier: MIT OR Apache-2.0
//! Per-folder message delta, folder delta, moves and lost sync state.

mod common;

use chrono::{TimeZone, Utc};
use common::{Client, ids, present, removed};
use familiar_mock_graph::{EmailAddress, Folder, Message, MockGraph};

async fn setup() -> (MockGraph, Client) {
    let mock = MockGraph::start().await.unwrap();
    let client = Client::new(mock.base_url(), "test-token").prefer("odata.maxpagesize=1");
    (mock, client)
}

#[tokio::test]
async fn initial_then_incremental_rounds() {
    let (mock, c) = setup().await;
    let mbx = mock.mailbox("default");

    let (items, pages, link) = c.delta_round("/me/mailFolders/inbox/messages/delta").await;
    assert_eq!(items.len(), 2);
    assert_eq!(pages, 2, "maxpagesize=1 gives one item per page");
    assert!(removed(&items).is_empty());
    assert!(link.contains("deltatoken"));

    // Nothing changed: an empty round.
    let (items, _, link) = c.delta_round(&link).await;
    assert!(items.is_empty());

    // Mark read, add a message, delete one.
    mbx.update_message("imm-small-01", |m| m.is_read = true)
        .unwrap();
    let new_id = mbx
        .add_message(Message::new(
            "inbox",
            "Fresh news",
            EmailAddress::new("Avery Lee", "avery.lee@example.com"),
            Utc.with_ymd_and_hms(2026, 11, 4, 10, 0, 0).unwrap(),
        ))
        .unwrap();
    let doomed = mbx.rest_id("imm-small-05").unwrap();
    mbx.delete_message("imm-small-05").unwrap();

    let (items, _, link) = c.delta_round(&link).await;
    let changed = present(&items);
    assert_eq!(changed.len(), 2);
    assert!(changed.contains(&mbx.rest_id("imm-small-01").unwrap()));
    assert!(changed.contains(&mbx.rest_id(&new_id).unwrap()));
    assert_eq!(removed(&items), [doomed]);
    let updated = items
        .iter()
        .find(|i| i["subject"] == "Practice moved to Thursday")
        .unwrap();
    assert_eq!(updated["isRead"], true);

    // Changes made while a round is being paged land in the next round, not this one.
    mbx.update_message("imm-small-01", |m| m.flagged = true)
        .unwrap();
    mbx.update_message(&new_id, |m| m.is_read = true).unwrap();
    let first = c.get(&link).await;
    assert_eq!(first.values()[0]["flag"]["flagStatus"], "flagged");
    mbx.update_message("imm-small-01", |m| m.subject = "Edited".into())
        .unwrap();
    let (rest_items, _, link) = c
        .delta_round(first.body["@odata.nextLink"].as_str().unwrap())
        .await;
    assert_eq!(present(&rest_items), [mbx.rest_id(&new_id).unwrap()]);
    let (items, _, _) = c.delta_round(&link).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["subject"], "Edited");
}

#[tokio::test]
async fn move_shows_as_removal_and_arrival_with_immutable_ids() {
    let mock = MockGraph::start().await.unwrap();
    let c = Client::new(mock.base_url(), "test-token").prefer("IdType=\"ImmutableId\"");
    let plain = Client::new(mock.base_url(), "test-token");
    let mbx = mock.mailbox("default");

    let (_, _, inbox_link) = c.delta_round("/me/mailFolders/inbox/messages/delta").await;
    let (archive, _, archive_link) = c
        .delta_round("/me/mailFolders/archive/messages/delta")
        .await;
    assert_eq!(ids(&archive), ["imm-small-06"]);

    let old_rest = mbx.rest_id("imm-small-01").unwrap();
    let new_rest = mbx.move_message("imm-small-01", "archive").unwrap();
    assert_ne!(old_rest, new_rest, "the default ID changes on a move");

    let (inbox_items, _, _) = c.delta_round(&inbox_link).await;
    assert_eq!(removed(&inbox_items), ["imm-small-01"]);
    let (archive_items, _, _) = c.delta_round(&archive_link).await;
    assert_eq!(present(&archive_items), ["imm-small-01"]);
    assert_eq!(archive_items[0]["parentFolderId"], "fld-archive");
    // The Internet message ID survives the move too.
    assert_eq!(
        archive_items[0]["internetMessageId"],
        "<imm-small-01@mock.familiar.invalid>"
    );

    // The old default ID is gone; the new one and the immutable ID work.
    assert_eq!(
        plain.get(&format!("/me/messages/{old_rest}")).await.status,
        404
    );
    assert_eq!(
        plain.get(&format!("/me/messages/{new_rest}")).await.status,
        200
    );
    assert_eq!(plain.get("/me/messages/imm-small-01").await.status, 200);
}

#[tokio::test]
async fn removal_ids_follow_the_id_preference() {
    let mock = MockGraph::start().await.unwrap();
    let plain = Client::new(mock.base_url(), "test-token");
    let mbx = mock.mailbox("default");
    let (_, _, link) = plain
        .delta_round("/me/mailFolders/inbox/messages/delta")
        .await;
    let rest = mbx.rest_id("imm-small-05").unwrap();
    mbx.move_message("imm-small-05", "deleteditems").unwrap();
    let (items, _, _) = plain.delta_round(&link).await;
    assert_eq!(removed(&items), [rest]);
}

#[tokio::test]
async fn expired_sync_state_answers_410() {
    let (mock, c) = setup().await;
    let (_, _, link) = c.delta_round("/me/mailFolders/inbox/messages/delta").await;
    let half = c.get("/me/mailFolders/inbox/messages/delta").await;
    let next = half.body["@odata.nextLink"].as_str().unwrap().to_string();

    mock.mailbox("default").expire_delta_tokens();
    for url in [&link, &next] {
        let r = c.get(url).await;
        assert_eq!(r.status, 410);
        assert_eq!(r.error_code(), "SyncStateNotFound");
    }
    // A fresh initial round works again.
    let (items, _, _) = c.delta_round("/me/mailFolders/inbox/messages/delta").await;
    assert_eq!(items.len(), 2);

    assert_eq!(
        c.get("/me/mailFolders/inbox/messages/delta?$deltatoken=junk")
            .await
            .status,
        400
    );
}

#[tokio::test]
async fn delta_honours_select_across_pages() {
    let (_mock, c) = setup().await;
    let (items, _, link) = c
        .delta_round("/me/mailFolders/inbox/messages/delta?$select=subject,isRead")
        .await;
    for i in &items {
        assert!(i.get("body").is_none() && i.get("subject").is_some());
    }
    assert!(link.contains("select"));
}

#[tokio::test]
async fn folder_delta_tracks_adds_renames_and_deletes() {
    let (mock, c) = setup().await;
    let mbx = mock.mailbox("default");
    let (items, _, link) = c.delta_round("/me/mailFolders/delta").await;
    let names: Vec<&str> = items
        .iter()
        .map(|f| f["displayName"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"Juniors"),
        "folder delta is flat and includes children"
    );
    assert!(!names.contains(&"Conversation History"));

    let id = mbx.add_folder(Folder::new("Tours").under("fld-club"));
    mbx.rename_folder("fld-receipts", "Receipts 2026").unwrap();
    mbx.delete_folder("fld-club-juniors").unwrap();

    let (items, _, _) = c.delta_round(&link).await;
    let p = present(&items);
    assert!(p.contains(&id) && p.contains(&"fld-receipts".to_string()));
    assert_eq!(removed(&items), ["fld-club-juniors"]);
    // Its message went with it.
    assert!(mbx.message("imm-small-03").is_none());
}
