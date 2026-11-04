// SPDX-License-Identifier: MIT OR Apache-2.0
//! Profile, folders and message reads against the bundled small mailbox.

mod common;

use common::{Client, ids};
use familiar_mock_graph::MockGraph;

async fn setup() -> (MockGraph, Client) {
    let mock = MockGraph::start().await.unwrap();
    let client = Client::new(mock.base_url(), "test-token");
    (mock, client)
}

#[tokio::test]
async fn profile_and_mailbox_settings() {
    let (_mock, c) = setup().await;
    let me = c.get("/me").await;
    assert_eq!(me.status, 200);
    assert_eq!(me.body["userPrincipalName"], "pat@example.com");
    assert!(me.header("request-id").is_some());

    let s = c.get("/me/mailboxSettings").await;
    assert_eq!(s.body["timeZone"], "New Zealand Standard Time");
    assert!(
        s.body.get("signature").is_none(),
        "Graph has no signature field"
    );

    // /users/{upn} is the same mailbox; another user is refused.
    assert_eq!(
        c.get("/users/pat@example.com/mailFolders/inbox")
            .await
            .status,
        200
    );
    let other = c.get("/users/someone@example.com/messages").await;
    assert_eq!(other.status, 403);
    assert_eq!(other.error_code(), "ErrorAccessDenied");
}

#[tokio::test]
async fn folder_tree() {
    let (_mock, c) = setup().await;
    let top = c.get_all("/me/mailFolders?$top=3").await;
    let names: Vec<&str> = top
        .iter()
        .map(|f| f["displayName"].as_str().unwrap())
        .collect();
    for wk in [
        "Inbox",
        "Drafts",
        "Sent Items",
        "Deleted Items",
        "Junk Email",
        "Archive",
        "Cricket Club",
    ] {
        assert!(names.contains(&wk), "{wk} missing from {names:?}");
    }
    assert!(
        !names.contains(&"Conversation History"),
        "hidden folders are left out"
    );
    assert!(
        !names.contains(&"Receipts"),
        "child folders are not top level"
    );

    let hidden = c.get_all("/me/mailFolders?includeHiddenFolders=true").await;
    assert_eq!(hidden.len(), top.len() + 1);

    let inbox = c.get("/me/mailFolders/inbox").await;
    assert_eq!(inbox.status, 200);
    assert_eq!(inbox.body["displayName"], "Inbox");
    assert_eq!(inbox.body["totalItemCount"], 2);
    assert_eq!(inbox.body["unreadItemCount"], 2);
    assert_eq!(inbox.body["childFolderCount"], 1);
    // Well-known names and IDs resolve to the same folder.
    let by_id = c
        .get(&format!(
            "/me/mailFolders/{}",
            inbox.body["id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(by_id.body["displayName"], "Inbox");

    let kids = c.get_all("/me/mailFolders/fld-club/childFolders").await;
    assert_eq!(ids(&kids), ["fld-club-juniors"]);

    let missing = c.get("/me/mailFolders/nope").await;
    assert_eq!(missing.status, 404);
    assert_eq!(missing.error_code(), "ErrorItemNotFound");
}

#[tokio::test]
async fn message_list_paging_select_filter() {
    let (_mock, c) = setup().await;
    let first = c.get("/me/messages?$top=3&$count=true").await;
    assert_eq!(first.status, 200);
    assert_eq!(first.values().len(), 3);
    assert_eq!(first.body["@odata.count"], 7);
    assert!(
        first.body["@odata.nextLink"]
            .as_str()
            .unwrap()
            .contains("skip=3")
    );

    let all = c.get_all("/me/messages?$top=3").await;
    assert_eq!(all.len(), 7);
    let times: Vec<&str> = all
        .iter()
        .map(|m| m["receivedDateTime"].as_str().unwrap())
        .collect();
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(times, sorted, "newest first by default");

    let asc = c
        .get_all("/me/messages?$orderby=receivedDateTime%20asc")
        .await;
    assert_eq!(asc[0]["subject"], "Membership renewal 2015");

    let sel = c
        .get("/me/mailFolders/inbox/messages?$select=subject,isRead")
        .await;
    let m = &sel.values()[0];
    assert!(m.get("subject").is_some() && m.get("isRead").is_some() && m.get("id").is_some());
    assert!(m.get("body").is_none() && m.get("from").is_none());

    let unread = c.get_all("/me/messages?$filter=isRead%20eq%20false").await;
    assert!(unread.iter().all(|m| m["isRead"] == false));
    assert_eq!(unread.len(), 3);

    let old = c
        .get_all("/me/messages?$filter=receivedDateTime%20lt%202020-01-01T00:00:00Z")
        .await;
    assert_eq!(old.len(), 1);

    let bad = c.get("/me/messages?$orderby=subject").await;
    assert_eq!(bad.status, 400);
}

#[tokio::test]
async fn single_message_and_body_preferences() {
    let (mock, c) = setup().await;
    let rest = mock.mailbox("default").rest_id("imm-small-01").unwrap();
    let m = c.get(&format!("/me/messages/{rest}")).await;
    assert_eq!(m.status, 200);
    assert_eq!(m.body["id"], rest.as_str());
    assert_eq!(m.body["body"]["contentType"], "html");
    assert_eq!(m.body["conversationId"], "conv-practice");
    assert!(
        m.body["internetMessageId"]
            .as_str()
            .unwrap()
            .starts_with('<')
    );
    assert!(m.body["@odata.etag"].as_str().is_some());

    let text = Client::new(mock.base_url(), "test-token")
        .prefer("outlook.body-content-type=\"text\"")
        .prefer("IdType=\"ImmutableId\"");
    let t = text.get(&format!("/me/messages/{rest}")).await;
    assert_eq!(t.body["id"], "imm-small-01");
    assert_eq!(t.body["body"]["contentType"], "text");
    assert!(
        t.body["body"]["content"]
            .as_str()
            .unwrap()
            .contains("practice is on Thursday")
    );
    assert!(
        t.header("preference-applied")
            .unwrap()
            .contains("ImmutableId")
    );

    // A message without a body in the fixture gets a stable synthetic one.
    let a = c.get("/me/messages/imm-small-03").await;
    let b = c.get("/me/messages/imm-small-03").await;
    assert_eq!(a.body["body"], b.body["body"]);
    assert!(a.body["body"]["content"].as_str().unwrap().len() > 50);

    // A message read through the wrong folder is not found.
    let wrong = c
        .get(&format!("/me/mailFolders/sentitems/messages/{rest}"))
        .await;
    assert_eq!(wrong.status, 404);
}

#[tokio::test]
async fn unsupported_requests_fail_loudly() {
    let (mock, c) = setup().await;
    assert_eq!(c.get("/me/contacts").await.status, 400);
    let beta = c.get(&mock.base_url().replace("/v1.0", "/beta/me")).await;
    assert_eq!(beta.status, 400);
    let post = c
        .http
        .post(c.url("/me/messages"))
        .bearer_auth("test-token")
        .send()
        .await
        .unwrap();
    assert_eq!(post.status(), 405);
}
