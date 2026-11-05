// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 1.6 against the mock: folder tree, delta with saved links, moves,
//! bodies and lost sync state.

mod common;

use chrono::{TimeZone, Utc};
use common::Env;
use familiar_mock_graph::{EmailAddress, Folder, Message, MockGraph};
use familiar_provider_graph::{FOLDERS_RESOURCE, Priority, messages_resource};

const BG: Priority = Priority::Background;

#[tokio::test]
async fn folder_tree_with_parents_and_well_known_names() {
    let mut env = Env::new(MockGraph::start().await.unwrap(), "test-token");
    let r = env.sync_tree().await;
    assert!(r.full_round);

    let folders = env.folders();
    let by_name = |n: &str| folders.iter().find(|f| f.name == n).unwrap().clone();
    assert_eq!(by_name("Inbox").well_known.as_deref(), Some("inbox"));
    assert_eq!(
        by_name("Sent Items").well_known.as_deref(),
        Some("sentitems")
    );
    assert_eq!(by_name("Juniors").parent, Some(by_name("Cricket Club").id));
    assert_eq!(by_name("Receipts").parent, Some(by_name("Inbox").id));
    assert_eq!(by_name("Inbox").parent, None, "the root isn't stored");
    assert!(
        !folders.iter().any(|f| f.name == "Conversation History"),
        "hidden folders are skipped"
    );

    // The walk with childFolders gives the same visible tree.
    let tree = env.sync.api().folder_tree(BG).await.unwrap();
    let mut a: Vec<_> = tree.iter().map(|f| f.id.clone()).collect();
    let mut b: Vec<_> = folders.iter().map(|f| f.remote_id.clone()).collect();
    a.sort();
    b.sort();
    assert_eq!(a, b);

    // The delta link is saved; the next round is incremental.
    assert!(env.state(FOLDERS_RESOURCE).unwrap().contains("deltatoken"));
    let mock = env.mock.clone();
    let mbx = mock.mailbox("default");
    let added = mbx.add_folder(Folder::new("Tours").under("fld-club"));
    mbx.rename_folder("fld-receipts", "Orders").unwrap();
    env.mock.clear_requests();
    let r = env.sync_tree().await;
    assert!(!r.full_round);
    assert_eq!(r.upserted, 2);
    let folders = env.folders();
    let tours = folders.iter().find(|f| f.remote_id == added).unwrap();
    assert_eq!(
        tours.parent,
        folders
            .iter()
            .find(|f| f.name == "Cricket Club")
            .map(|f| f.id)
    );
    assert!(folders.iter().any(|f| f.name == "Orders"));
    // Well-known folders are only looked up on a full round.
    assert!(
        env.mock
            .requests()
            .iter()
            .all(|r| !r.path.ends_with("/mailFolders/inbox"))
    );
    assert_eq!(
        env.folders()
            .iter()
            .find(|f| f.name == "Inbox")
            .unwrap()
            .well_known
            .as_deref(),
        Some("inbox"),
        "an incremental round keeps well-known names"
    );

    mbx.delete_folder(&added).unwrap();
    let r = env.sync_tree().await;
    assert_eq!(r.removed, 1);
    assert!(!env.folders().iter().any(|f| f.remote_id == added));
}

#[tokio::test]
async fn first_sync_matches_the_server_and_rounds_resume_from_saved_links() {
    let mut env = Env::new(MockGraph::start().await.unwrap(), "test-token");
    env.sync_tree().await;
    env.sync_all().await;

    let mock = env.mock.clone();
    let mbx = mock.mailbox("default");
    for f in env.folders() {
        let mut server = mbx.message_ids(&f.remote_id);
        let mut local = env.remote_ids(&f.remote_id);
        server.sort();
        local.sort();
        assert_eq!(local, server, "folder {}", f.name);
        assert_eq!(f.total as usize, server.len());
        let link = env.state(&messages_resource(&f.remote_id)).unwrap();
        assert!(link.contains("deltatoken"), "{link}");
    }
    let m = env.local("imm-small-05").unwrap();
    assert_eq!(m.subject, "Committee meeting agenda");
    assert!(m.is_flagged);
    assert_eq!(m.importance, 2);
    assert_eq!(m.body, None, "headers only until a body is fetched");

    // Read, new and deleted messages arrive in the next round.
    mbx.update_message("imm-small-01", |m| m.is_read = true)
        .unwrap();
    let fresh = mbx
        .add_message(Message::new(
            "inbox",
            "Fresh news",
            EmailAddress::new("Avery Lee", "avery.lee@example.com"),
            Utc.with_ymd_and_hms(2026, 11, 4, 10, 0, 0).unwrap(),
        ))
        .unwrap();
    mbx.delete_message("imm-small-05").unwrap();
    env.mock.clear_requests();
    let r = env.sync_folders(&["fld-inbox"]).await;
    assert_eq!(r.full_rounds, 0);
    assert_eq!(r.upserted, 2);
    assert_eq!(r.deleted, 1);
    assert!(env.local("imm-small-01").unwrap().is_read);
    assert!(env.local(&fresh).is_some());
    assert!(env.local("imm-small-05").is_none());
    let reqs = env.mock.requests();
    assert!(
        reqs.iter()
            .filter(|r| r.path.ends_with("/delta"))
            .all(|r| r.query.contains("deltatoken")),
        "resumed from the saved link: {reqs:?}"
    );
    let inbox = env.folder("fld-inbox");
    assert_eq!((inbox.total, inbox.unread), (2, 1));
}

/// Found on the WP 1.13 live run: Outlook.com listed new and changed Inbox
/// messages with only an ID and read state, and they were stored blank
/// (received at 0, no subject, sender or conversation).
#[tokio::test]
async fn sparse_delta_items_are_stored_with_their_full_header() {
    let mut env = Env::new(MockGraph::start().await.unwrap(), "test-token");
    env.sync_tree().await;
    env.sync_all().await;
    let before = env.local("imm-small-01").unwrap();

    let mock = env.mock.clone();
    let mbx = mock.mailbox("default");
    mock.set_sparse_message_delta(true);
    mbx.update_message("imm-small-01", |m| m.is_read = true)
        .unwrap();
    let fresh = mbx
        .add_message(Message::new(
            "inbox",
            "Fresh news",
            EmailAddress::new("Avery Lee", "avery.lee@example.com"),
            Utc.with_ymd_and_hms(2026, 11, 4, 10, 0, 0).unwrap(),
        ))
        .unwrap();
    let r = env.sync_folders(&["fld-inbox"]).await;
    assert_eq!(r.full_rounds, 0);
    assert_eq!(r.upserted, 2);

    let changed = env.local("imm-small-01").unwrap();
    assert!(changed.is_read);
    assert_eq!(changed.subject, before.subject);
    assert_eq!(changed.received_at, before.received_at);
    assert_eq!(changed.thread, before.thread);
    let new = env.local(&fresh).unwrap();
    assert_eq!(new.subject, "Fresh news");
    assert_eq!(new.from.addr, "avery.lee@example.com");
    assert_eq!(
        new.received_at,
        Utc.with_ymd_and_hms(2026, 11, 4, 10, 0, 0)
            .unwrap()
            .timestamp()
    );
}

#[tokio::test]
async fn small_pages_keep_the_next_link_as_bookmark() {
    let mock = MockGraph::start().await.unwrap();
    let mut env = Env::new(mock, "test-token").page_size(1);
    env.sync_tree().await;
    let r = env.sync_folders(&["fld-inbox"]).await;
    assert_eq!(r.pages, 2, "one message per page");
    assert_eq!(env.remote_ids("fld-inbox").len(), 2);
}

#[tokio::test]
async fn move_between_synced_folders_keeps_the_body_without_extra_requests() {
    let mut env = Env::new(MockGraph::start().await.unwrap(), "test-token");
    env.sync_tree().await;
    env.sync_all().await;
    env.fetch_body("imm-small-01").await;

    env.mock
        .mailbox("default")
        .move_message("imm-small-01", "archive")
        .unwrap();
    env.mock.clear_requests();
    // Inbox reports the removal before Archive reports the arrival.
    let r = env.sync_folders(&["fld-inbox", "fld-archive"]).await;
    assert_eq!((r.upserted, r.moved, r.deleted), (1, 0, 0));

    let m = env.local("imm-small-01").unwrap();
    assert_eq!(m.folder, env.folder("fld-archive").id);
    assert!(env.body_text(&m).contains("Thursday"), "body kept");
    assert!(
        env.mock
            .requests()
            .iter()
            .all(|r| !r.path.contains("/messages/imm-small-01")),
        "no lookup needed when the arrival is seen in the same call"
    );
    assert_eq!(env.folder("fld-inbox").total, 1);
    assert_eq!(env.folder("fld-archive").total, 2);
}

#[tokio::test]
async fn move_seen_only_from_the_source_folder_is_found_by_immutable_id() {
    let mut env = Env::new(MockGraph::start().await.unwrap(), "test-token");
    env.sync_tree().await;
    env.sync_all().await;
    env.fetch_body("imm-small-01").await;

    env.mock
        .mailbox("default")
        .move_message("imm-small-01", "fld-club-juniors")
        .unwrap();
    // Only Inbox is polled (the 30-second poll).
    let r = env.sync_folders(&["fld-inbox"]).await;
    assert_eq!((r.moved, r.deleted), (1, 0));
    let m = env.local("imm-small-01").unwrap();
    assert_eq!(m.folder, env.folder("fld-club-juniors").id);
    assert!(env.body_text(&m).contains("Thursday"));

    // The destination's own round later sees it as an ordinary update.
    let r = env.sync_folders(&["fld-club-juniors"]).await;
    assert_eq!((r.moved, r.deleted), (0, 0));
    assert_eq!(env.folder("fld-club-juniors").total, 2);
    let thread = env.reader().thread(m.thread).unwrap();
    assert_eq!(thread.len(), 3, "the conversation still spans folders");
}

#[tokio::test]
async fn move_with_a_new_id_is_matched_by_internet_message_id() {
    let mut env = Env::new(MockGraph::start().await.unwrap(), "test-token");
    env.sync_tree().await;
    env.sync_all().await;
    env.fetch_body("imm-small-01").await;
    let old = env.local("imm-small-01").unwrap();
    let imid = old.internet_message_id.clone().unwrap();

    // A move that changes the immutable ID (to another mailbox, say) looks
    // like a delete and an unrelated new message with the same Message-ID.
    let mock = env.mock.clone();
    let mbx = mock.mailbox("default");
    let mut copy = mbx.message("imm-small-01").unwrap();
    copy.id = String::new();
    copy.folder_id = "archive".into();
    copy.body_html = Some("<p>different bytes from the server</p>".into());
    mbx.delete_message("imm-small-01").unwrap();
    let new_id = mbx.add_message(copy).unwrap();

    // Source first, in its own call: the body waits for the arrival.
    let r = env.sync_folders(&["fld-inbox"]).await;
    assert_eq!(r.deleted, 1);
    let r = env.sync_folders(&["fld-archive"]).await;
    assert_eq!(r.upserted, 1);
    let m = env.local(&new_id).unwrap();
    assert_eq!(m.internet_message_id.as_deref(), Some(imid.as_str()));
    assert_eq!(m.body, old.body, "the local body was carried over");

    // Destination first: the old copy gives its body to the new one.
    env.fetch_body(&new_id).await;
    let mut again = mbx.message(&new_id).unwrap();
    again.id = String::new();
    again.folder_id = "inbox".into();
    mbx.delete_message(&new_id).unwrap();
    let third = mbx.add_message(again).unwrap();
    let r = env.sync_folders(&["fld-inbox", "fld-archive"]).await;
    assert_eq!((r.deleted, r.bodies_carried), (1, 1));
    let m = env.local(&third).unwrap();
    assert!(env.body_text(&m).contains("different bytes"));
    assert!(env.local(&new_id).is_none());
}

#[tokio::test]
async fn expired_delta_link_starts_over_and_reconciles() {
    let mut env = Env::new(MockGraph::start().await.unwrap(), "test-token");
    env.sync_tree().await;
    env.sync_all().await;

    let mock = env.mock.clone();
    let mbx = mock.mailbox("default");
    mbx.expire_delta_tokens();
    // Changes made while our bookmark is dead must not be lost.
    mbx.delete_message("imm-small-05").unwrap();
    mbx.update_message("imm-small-01", |m| m.subject = "Edited".into())
        .unwrap();

    let r = env.sync_folders(&["fld-inbox"]).await;
    assert_eq!((r.restarted, r.full_rounds, r.deleted), (1, 1, 1));
    assert_eq!(env.local("imm-small-01").unwrap().subject, "Edited");
    assert!(env.local("imm-small-05").is_none());
    assert!(
        env.state(&messages_resource("fld-inbox"))
            .unwrap()
            .contains("deltatoken")
    );

    // The folder tree recovers the same way.
    let r = env.sync_tree().await;
    assert!(r.full_round);
    assert_eq!(r.removed, 0);
}

#[tokio::test]
async fn header_pages_and_bodies() {
    let mut env = Env::new(MockGraph::start().await.unwrap(), "test-token");
    env.sync_tree().await;
    let (n, next) = env
        .sync
        .store_headers_page(&mut env.w, "fld-inbox", 1, None, BG)
        .await
        .unwrap();
    assert_eq!(n, 1);
    // Newest first.
    assert!(env.local("imm-small-01").is_some());
    assert!(env.local("imm-small-05").is_none());
    let (n, next) = env
        .sync
        .store_headers_page(&mut env.w, "fld-inbox", 1, next.as_deref(), BG)
        .await
        .unwrap();
    assert_eq!((n, next), (1, None));
    assert!(env.local("imm-small-05").is_some());

    let id = env.local("imm-small-05").unwrap().id;
    let body = env
        .sync
        .fetch_body(&mut env.w, id, Priority::User)
        .await
        .unwrap();
    assert!(body.is_html);
    let m = env.local("imm-small-05").unwrap();
    assert!(env.body_text(&m).contains("volunteer roster"));
    // A later header update keeps the body.
    env.mock
        .mailbox("default")
        .update_message("imm-small-05", |m| m.is_read = true)
        .unwrap();
    env.sync_folders(&["fld-inbox"]).await;
    let m = env.local("imm-small-05").unwrap();
    assert!(m.is_read);
    assert_eq!(m.body, Some(body));
}
