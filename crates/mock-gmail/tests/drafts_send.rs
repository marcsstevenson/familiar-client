// SPDX-License-Identifier: MIT OR Apache-2.0
//! `messages.send`, the upload endpoint, send-as rules and the draft round-trip.

mod common;

use common::{Client, b64};
use familiar_mock_gmail::{MockGmail, SMALL_EMAIL};
use serde_json::json;

async fn setup() -> (MockGmail, Client) {
    let mock = MockGmail::start().await.unwrap();
    let c = Client::new(&mock.origin(), "test-token");
    (mock, c)
}

fn raw(from: &str, to: &str, subject: &str) -> String {
    format!("From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\n\r\nHello there.\r\n")
}

#[tokio::test]
async fn send_as_list() {
    let (_mock, c) = setup().await;
    let s = c.ok("settings/sendAs").await;
    let s = s["sendAs"].as_array().unwrap();
    assert_eq!(s.len(), 3);
    assert_eq!(s[0]["sendAsEmail"], SMALL_EMAIL);
    assert_eq!(s[0]["isPrimary"], true);
    assert!(s[0].get("verificationStatus").is_none());
    assert_eq!(s[1]["verificationStatus"], "accepted");
    assert_eq!(s[1]["signature"], "<p>Kea</p>");
    assert_eq!(s[2]["verificationStatus"], "pending");
    let one = c.ok("settings/sendAs/KEA@familiarclient.com").await;
    assert_eq!(one["treatAsAlias"], true);
    assert_eq!(c.get("settings/sendAs/x@y.z").await.status, 404);
}

#[tokio::test]
async fn send_json_and_from_rules() {
    let (mock, c) = setup().await;
    let r = c
        .post(
            "messages/send",
            json!({"raw": b64(&raw("Kea <kea@familiarclient.com>", "bob@example.com", "Hi"))}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r.body["labelIds"], json!(["SENT"]));
    let id = r.body["id"].as_str().unwrap().to_string();

    // An unverified alias is rewritten to the account's address.
    let r = c
        .post(
            "messages/send",
            json!({"raw": b64(&raw("old-kea@example.com", "bob@example.com", "Hi"))}),
        )
        .await;
    assert_eq!(r.status, 200);

    let sent = mock.mailbox("default").sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].id, id);
    assert_eq!(
        sent[0].header("from").unwrap(),
        "Kea <kea@familiarclient.com>"
    );
    assert_eq!(
        sent[1].header("from").unwrap(),
        format!("Kea Tester <{SMALL_EMAIL}>")
    );
    assert!(sent[1].header("message-id").is_some());
    assert!(sent[1].header("date").is_some());

    let r = c
        .post(
            "messages/send",
            json!({"raw": b64("Subject: none\r\n\r\nx")}),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.body["error"]["message"], "Recipient address required");
    assert_eq!(c.post("messages/send", json!({})).await.status, 400);
}

#[tokio::test]
async fn reply_joins_thread_and_mail_to_self_lands_in_inbox() {
    let (mock, c) = setup().await;
    let reply = format!(
        "From: {SMALL_EMAIL}\r\nTo: ann@acme.example\r\nSubject: Re: Quote\r\n\
         In-Reply-To: <acme-3@acme.example>\r\nReferences: <acme-3@acme.example>\r\n\r\nOK\r\n"
    );
    let r = c.post("messages/send", json!({"raw": b64(&reply)})).await;
    assert_eq!(r.body["threadId"], "msg-acme-1");

    let r = c
        .post("messages/send", json!({"raw": b64(&raw(SMALL_EMAIL, "kea@familiarclient.com", "Note to self")), "threadId": "msg-dinner"}))
        .await;
    assert_eq!(r.body["threadId"], "msg-dinner");
    let id = r.body["id"].as_str().unwrap();
    let m = mock.mailbox("default").message(id).unwrap();
    assert_eq!(m.label_ids, ["SENT", "INBOX", "UNREAD"]);
}

#[tokio::test]
async fn upload_media_and_multipart() {
    let (mock, c) = setup().await;
    let url = format!(
        "{}/upload/gmail/v1/users/me/messages/send?uploadType=media",
        mock.origin()
    );
    let r = c
        .send(
            c.http
                .post(url)
                .header("content-type", "message/rfc822")
                .body(raw(SMALL_EMAIL, "bob@example.com", "Media")),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);

    let body = format!(
        "--u1\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{{\"threadId\": \"msg-dinner\"}}\r\n\
         --u1\r\nContent-Type: message/rfc822\r\n\r\n{}\r\n--u1--\r\n",
        raw(SMALL_EMAIL, "bob@example.com", "Multipart")
    );
    let url = format!(
        "{}/upload/gmail/v1/users/me/messages/send?uploadType=multipart",
        mock.origin()
    );
    let r = c
        .send(
            c.http
                .post(url)
                .header("content-type", "multipart/related; boundary=u1")
                .body(body),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r.body["threadId"], "msg-dinner");
    let sent = mock.mailbox("default").sent();
    assert_eq!(sent[1].header("subject").unwrap(), "Multipart");

    let url = format!(
        "{}/upload/gmail/v1/users/me/messages/send?uploadType=resumable",
        mock.origin()
    );
    assert_eq!(c.send(c.http.post(url).body("x")).await.status, 400);
}

#[tokio::test]
async fn draft_round_trip() {
    let (mock, c) = setup().await;
    let start = mock.mailbox("default").history_id();
    let r = c
        .post("drafts", json!({"message": {"raw": b64(&raw(SMALL_EMAIL, "bob@example.com", "Draft 1")), "threadId": "msg-dinner"}}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let draft = r.body["id"].as_str().unwrap().to_string();
    let msg1 = r.body["message"]["id"].as_str().unwrap().to_string();
    assert_eq!(r.body["message"]["labelIds"], json!(["DRAFT"]));
    assert_eq!(r.body["message"]["threadId"], "msg-dinner");

    let list = c.ok("drafts").await;
    assert_eq!(list["drafts"].as_array().unwrap().len(), 2);
    let got = c.ok(&format!("drafts/{draft}?format=raw")).await;
    assert_eq!(got["message"]["id"], msg1.as_str());

    // Updating keeps the draft ID but gives it a new message ID, in the same thread.
    let r = c
        .put(
            &format!("drafts/{draft}"),
            json!({"message": {"raw": b64(&raw(SMALL_EMAIL, "bob@example.com", "Draft 2"))}}),
        )
        .await;
    assert_eq!(r.body["id"], draft.as_str());
    let msg2 = r.body["message"]["id"].as_str().unwrap().to_string();
    assert_ne!(msg1, msg2);
    assert_eq!(r.body["message"]["threadId"], "msg-dinner");
    assert_eq!(c.get(&format!("messages/{msg1}")).await.status, 404);

    let r = c.post("drafts/send", json!({"id": draft})).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r.body["labelIds"], json!(["SENT"]));
    assert_eq!(r.body["threadId"], "msg-dinner");
    assert_eq!(c.get(&format!("drafts/{draft}")).await.status, 404);
    assert_eq!(
        mock.mailbox("default").sent()[0].header("subject").unwrap(),
        "Draft 2"
    );

    let h = c.ok(&format!("history?startHistoryId={start}")).await;
    let n = h["history"].as_array().unwrap().len();
    assert_eq!(n, 5, "add, delete+add on update, delete+add on send");

    // Deleting a draft deletes its message.
    let (fixture_draft, fixture_msg) = mock.mailbox("default").drafts()[0].clone();
    assert_eq!(
        c.delete(&format!("drafts/{fixture_draft}")).await.status,
        204
    );
    assert!(mock.mailbox("default").message(&fixture_msg).is_none());
    assert_eq!(
        c.delete(&format!("drafts/{fixture_draft}")).await.status,
        404
    );
}
