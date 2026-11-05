// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 1.8 against the mock: a conversation synced from Inbox, Sent Items
//! and a custom folder assembles locally, and stays whole through new
//! replies, moves and deletes picked up by delta.

mod common;

use chrono::{TimeZone, Utc};
use common::Env;
use familiar_mock_graph::{EmailAddress, Message, MockGraph};
use familiar_store::{Conversation, SENT_ITEMS};

fn conversation(env: &Env, remote: &str) -> Conversation {
    let id = env.local(remote).expect("synced").id;
    env.reader()
        .conversation_of(id)
        .unwrap()
        .expect("conversation")
}

fn remote_ids(c: &Conversation) -> Vec<&str> {
    c.messages
        .iter()
        .map(|m| m.message.remote_id.as_str())
        .collect()
}

#[tokio::test]
async fn inbox_and_sent_items_threads_assemble_after_sync() {
    let mut env = Env::new(MockGraph::start().await.unwrap(), "test-token");
    env.sync_tree().await;
    env.sync_all().await;

    // The fixture's practice thread: Inbox, Sent Items, Cricket Club/Juniors.
    let c = conversation(&env, "imm-small-01");
    assert_eq!(
        remote_ids(&c),
        vec!["imm-small-01", "imm-small-02", "imm-small-03"]
    );
    assert_eq!(c.conversation_id, "conv-practice");
    assert_eq!(c.subject, "Practice moved to Thursday");
    assert_eq!(c, conversation(&env, "imm-small-02"));
    let folders: Vec<_> = c.folders.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(folders.len(), 3);
    assert!(folders.contains(&"Inbox") && folders.contains(&"Juniors"));
    let sent = &c.messages[1];
    assert!(sent.is_sent() && sent.from_me);
    assert_eq!(sent.folder_well_known.as_deref(), Some(SENT_ITEMS));
    assert!(!c.messages[0].from_me && !c.messages[2].from_me);
    assert_eq!(c.unread_count, 2);

    // The threaded Inbox list shows the conversation with its full count.
    let inbox = env.folder("fld-inbox");
    let rows = env.reader().conversations(inbox.id, None, 50).unwrap();
    let row = rows.iter().find(|r| r.thread == c.thread).unwrap();
    assert_eq!((row.folder_message_count, row.message_count), (1, 3));
    assert!(row.has_sent);
    assert_eq!(row.latest.id, c.messages[2].message.id);

    // Unrelated mail stays in its own conversations.
    assert_eq!(
        remote_ids(&conversation(&env, "imm-small-05")),
        vec!["imm-small-05"]
    );

    // New mail on both sides, a move and a delete, all through delta.
    let mbx = env.mock.mailbox("default");
    let at = |h| Utc.with_ymd_and_hms(2026, 11, 4, h, 0, 0).unwrap();
    let mut reply = Message::new(
        "sentitems",
        "RE: Practice moved to Thursday",
        EmailAddress::new("Pat Example", "pat@example.com"),
        at(9),
    );
    reply.conversation_id = "conv-practice".into();
    reply.is_read = true;
    let reply = mbx.add_message(reply).unwrap();
    let mut answer = Message::new(
        "inbox",
        "RE: Practice moved to Thursday",
        EmailAddress::new("Sam Walker", "sam.walker@club.example"),
        at(10),
    );
    answer.conversation_id = "conv-practice".into();
    let answer = mbx.add_message(answer).unwrap();
    // The immutable ID survives the move (the REST ID would not).
    mbx.move_message("imm-small-01", "archive").unwrap();
    mbx.delete_message("imm-small-03").unwrap();
    env.sync_all().await;

    let c = conversation(&env, &answer);
    // Oldest first: the moved original keeps its received time.
    assert_eq!(
        remote_ids(&c),
        vec![
            "imm-small-01",
            "imm-small-02",
            reply.as_str(),
            answer.as_str()
        ]
    );
    let names: Vec<_> = c
        .messages
        .iter()
        .map(|m| m.folder_well_known.as_deref().unwrap_or(&m.folder_name))
        .collect();
    assert_eq!(names, vec!["archive", SENT_ITEMS, SENT_ITEMS, "inbox"]);
    assert_eq!(
        c.messages.iter().filter(|m| m.from_me).count(),
        2,
        "both replies are Pat's"
    );
}
