// SPDX-License-Identifier: MIT OR Apache-2.0
//! Profile, labels, message lists, search, formats, threads and label changes.

mod common;

use common::{Client, unb64};
use familiar_mock_gmail::{MockGmail, SMALL_EMAIL};
use serde_json::json;

async fn setup() -> (MockGmail, Client) {
    let mock = MockGmail::start().await.unwrap();
    let c = Client::new(&mock.origin(), "test-token");
    (mock, c)
}

#[tokio::test]
async fn profile_and_labels() {
    let (_mock, c) = setup().await;
    let p = c.ok("profile").await;
    assert_eq!(p["emailAddress"], SMALL_EMAIL);
    assert_eq!(p["messagesTotal"], 12);
    assert!(p["historyId"].as_str().unwrap().parse::<u64>().is_ok());

    let labels = c.ok("labels").await;
    let labels = labels["labels"].as_array().unwrap();
    assert_eq!(labels.iter().filter(|l| l["type"] == "system").count(), 14);
    let acme = labels.iter().find(|l| l["name"] == "Clients/Acme").unwrap();
    assert_eq!(acme["id"], "Label_2");
    assert_eq!(acme["color"]["backgroundColor"], "#4a86e8");
    let old = labels.iter().find(|l| l["id"] == "Label_4").unwrap();
    assert_eq!(old["labelListVisibility"], "labelHide");
    // labels.list has no counts; labels.get does.
    assert!(acme.get("messagesTotal").is_none());
    let acme = c.ok("labels/Label_2").await;
    assert_eq!(acme["messagesTotal"], 3);
    assert_eq!(acme["messagesUnread"], 1);
    assert_eq!(acme["threadsTotal"], 1);
    assert_eq!(acme["threadsUnread"], 1);
    assert_eq!(c.get("labels/Label_99").await.status, 404);
}

#[tokio::test]
async fn label_create_rename_delete() {
    let (mock, c) = setup().await;
    let r = c
        .post(
            "labels",
            json!({"name": "Familiar Test", "labelListVisibility": "labelShow"}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let id = r.body["id"].as_str().unwrap().to_string();
    assert_eq!(r.body["type"], "user");

    let r = c.post("labels", json!({"name": "familiar test"})).await;
    assert_eq!(r.status, 409);

    let r = c
        .patch(&format!("labels/{id}"), json!({"name": "Familiar Test 2"}))
        .await;
    assert_eq!(r.body["name"], "Familiar Test 2");
    assert_eq!(
        c.patch("labels/INBOX", json!({"name": "In"})).await.status,
        400
    );

    // Renaming a parent leaves its children's names alone, as Gmail does.
    let r = c
        .patch("labels/Label_1", json!({"name": "Customers"}))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(c.ok("labels/Label_2").await["name"], "Clients/Acme");

    // Deleting a label takes it off its messages, in history too.
    let start = mock.mailbox("default").history_id();
    assert_eq!(c.delete("labels/Label_2").await.status, 204);
    let m = mock.mailbox("default").message("msg-acme-1").unwrap();
    assert!(!m.label_ids.contains(&"Label_2".to_string()));
    let h = c.ok(&format!("history?startHistoryId={start}")).await;
    assert_eq!(h["history"].as_array().unwrap().len(), 3);
    assert_eq!(c.delete("labels/INBOX").await.status, 400);
}

#[tokio::test]
async fn list_excludes_spam_and_trash_and_pages() {
    let (_mock, c) = setup().await;
    let all = c.list_all("").await;
    assert_eq!(all.len(), 10);
    assert!(!all.contains(&"msg-spam".to_string()));
    // Newest first.
    assert_eq!(all[0], "msg-promo");
    assert_eq!(all.last().unwrap(), "msg-2015");

    let with = c.list_all("includeSpamTrash=true").await;
    assert_eq!(with.len(), 12);

    let paged = c.list_all("maxResults=3").await;
    assert_eq!(paged, all);
    let first = c.ok("messages?maxResults=3").await;
    assert_eq!(first["resultSizeEstimate"], 10);
    assert!(first["nextPageToken"].is_string());

    let inbox = c.list_all("labelIds=INBOX&labelIds=UNREAD").await;
    assert_eq!(inbox, ["msg-promo", "msg-dinner", "msg-acme-3"]);
    let spam = c.list_all("labelIds=SPAM").await;
    assert_eq!(spam, ["msg-spam"]);
    assert_eq!(c.get("messages?labelIds=NOPE").await.status, 400);

    let empty = c.ok("messages?q=from:nobody@nowhere").await;
    assert_eq!(empty["resultSizeEstimate"], 0);
    assert!(empty.get("messages").is_none());
}

#[tokio::test]
async fn search_query() {
    let (_mock, c) = setup().await;
    let q = |q: &str| {
        let c = &c;
        let q = form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>();
        async move { c.list_all(&format!("q={q}")).await }
    };
    assert_eq!(
        q("from:ann@acme.example").await,
        ["msg-acme-3", "msg-acme-1"]
    );
    assert_eq!(q("subject:\"new site\" is:unread").await, ["msg-acme-3"]);
    assert_eq!(q("label:clients-acme").await.len(), 3);
    assert_eq!(q("label:Label_3").await, ["msg-receipt"]);
    assert_eq!(q("has:attachment").await, ["msg-photos"]);
    assert_eq!(q("is:starred").await, ["msg-photos"]);
    assert_eq!(q("before:2016/01/01").await, ["msg-2015"]);
    assert!(q("after:2026/10/26 -in:inbox").await.is_empty());
    assert_eq!(q("in:spam").await, ["msg-spam"]);
    assert_eq!(q("dinner").await, ["msg-draft", "msg-dinner"]);
    assert_eq!(q("rfc822msgid:acme-1@acme.example").await, ["msg-acme-1"]);
    assert_eq!(q("in:anywhere prize").await, ["msg-spam"]);
}

#[tokio::test]
async fn message_formats_and_attachments() {
    let (_mock, c) = setup().await;
    let m = c.ok("messages/msg-dinner?format=minimal").await;
    assert_eq!(m["threadId"], "msg-dinner");
    assert_eq!(
        m["labelIds"],
        json!(["INBOX", "UNREAD", "CATEGORY_PERSONAL"])
    );
    assert!(m["internalDate"].as_str().unwrap().parse::<i64>().is_ok());
    assert_eq!(m["snippet"], "Are you free for dinner on Friday?");
    assert!(m.get("payload").is_none());

    let m = c
        .ok("messages/msg-dinner?format=metadata&metadataHeaders=From&metadataHeaders=Subject")
        .await;
    let headers = m["payload"]["headers"].as_array().unwrap();
    assert_eq!(headers.len(), 2);
    assert_eq!(headers[1]["value"], "Dinner on Friday");

    let m = c.ok("messages/msg-dinner?format=raw").await;
    let raw = unb64(m["raw"].as_str().unwrap());
    assert!(raw.starts_with("From: Bob <bob@example.com>\r\n"), "{raw}");

    let m = c.ok("messages/msg-photos").await;
    let parts = m["payload"]["parts"].as_array().unwrap();
    assert_eq!(m["payload"]["mimeType"], "multipart/mixed");
    assert_eq!(parts[0]["mimeType"], "text/plain");
    assert_eq!(
        unb64(parts[0]["body"]["data"].as_str().unwrap()),
        "Photos attached."
    );
    assert_eq!(parts[1]["filename"], "beach.jpg");
    let att = parts[1]["body"]["attachmentId"].as_str().unwrap();
    let a = c
        .ok(&format!("messages/msg-photos/attachments/{att}"))
        .await;
    assert_eq!(a["size"], 13);
    assert_eq!(c.get("messages/msg-photos?format=fancy").await.status, 400);
    assert_eq!(c.get("messages/nope").await.status, 404);
}

#[tokio::test]
async fn threads_and_user_id() {
    let (_mock, c) = setup().await;
    let t = c.ok("threads/msg-acme-1?format=minimal").await;
    let ids: Vec<&str> = t["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["msg-acme-1", "msg-acme-2", "msg-acme-3"]);
    assert_eq!(c.get("threads/none").await.status, 404);

    let by_address = c
        .get(&format!("/gmail/v1/users/{SMALL_EMAIL}/profile"))
        .await;
    assert_eq!(by_address.status, 200);
    let other = c.get("/gmail/v1/users/someone@gmail.com/profile").await;
    assert_eq!(other.status, 403);
}

#[tokio::test]
async fn modify_trash_and_delete() {
    let (mock, c) = setup().await;
    let r = c
        .post(
            "messages/msg-dinner/modify",
            json!({"addLabelIds": ["STARRED", "Label_1"], "removeLabelIds": ["UNREAD", "INBOX"]}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(
        r.body["labelIds"],
        json!(["CATEGORY_PERSONAL", "STARRED", "Label_1"])
    );
    let r = c
        .post(
            "messages/msg-dinner/modify",
            json!({"addLabelIds": ["Nope"]}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.body["error"]["message"], "Invalid label: Nope");

    let r = c
        .post(
            "messages/batchModify",
            json!({"ids": ["msg-promo", "msg-acme-3", "gone"], "removeLabelIds": ["UNREAD"]}),
        )
        .await;
    assert_eq!(r.status, 204);
    assert!(
        !mock
            .mailbox("default")
            .message("msg-promo")
            .unwrap()
            .label_ids
            .contains(&"UNREAD".into())
    );

    let r = c.post("messages/msg-promo/trash", json!({})).await;
    assert!(
        r.body["labelIds"]
            .as_array()
            .unwrap()
            .contains(&json!("TRASH"))
    );
    assert!(!c.list_all("").await.contains(&"msg-promo".to_string()));
    c.post("messages/msg-promo/untrash", json!({})).await;
    assert!(c.list_all("").await.contains(&"msg-promo".to_string()));

    assert_eq!(c.delete("messages/msg-promo").await.status, 204);
    assert_eq!(c.get("messages/msg-promo").await.status, 404);
    assert_eq!(c.delete("messages/msg-promo").await.status, 404);
    let r = c
        .post(
            "messages/batchDelete",
            json!({"ids": ["msg-dinner", "msg-photos"]}),
        )
        .await;
    assert_eq!(r.status, 204);
    assert_eq!(mock.mailbox("default").message_count(), 9);
}

#[tokio::test]
async fn synthetic_mailbox_is_deterministic() {
    use familiar_mock_gmail::Mailbox;
    let a = MockGmail::builder()
        .mailbox("big", Mailbox::synthetic(2_000, 7))
        .start()
        .await
        .unwrap();
    let b = MockGmail::builder()
        .mailbox("big", Mailbox::synthetic(2_000, 7))
        .start()
        .await
        .unwrap();
    let ca = Client::new(&a.origin(), "big-token");
    let cb = Client::new(&b.origin(), "big-token");
    let ids = ca.list_all("maxResults=500").await;
    assert_eq!(ids.len(), 2_000);
    assert_eq!(ids, cb.list_all("maxResults=500").await);
    let p = ca.ok("profile").await;
    let threads = p["threadsTotal"].as_u64().unwrap();
    assert!(threads > 500 && threads < 2_000, "{threads}");
}
