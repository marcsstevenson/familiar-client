// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail mail read (WP G.5) against the mock: labels as folders, the first
//! listing, history rounds, expired history, bodies and throttling.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use familiar_mock_gmail::{Fault, Label, Message, MockGmail};
use familiar_provider_gmail::{
    ALL_MAIL, ClientOptions, GmailApi, GmailClient, GmailSync, HISTORY_RESOURCE, Priority,
    StaticToken,
};
use familiar_store::{AccountId, Cursor, NewAccount, Reader, Writer};

struct Env {
    mock: MockGmail,
    dir: tempfile::TempDir,
    w: Writer,
    acc: AccountId,
    sync: GmailSync,
}

async fn env() -> Env {
    let mock = MockGmail::start().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let acc = w
        .write(|tx| {
            tx.upsert_account(&NewAccount {
                key: "gmail".into(),
                email: familiar_mock_gmail::SMALL_EMAIL.into(),
                provider: "gmail".into(),
                ..Default::default()
            })
        })
        .unwrap();
    let mut opts = ClientOptions::with_base(format!("{}/users/me", mock.api_url()));
    opts.default_retry_after = Duration::from_millis(20);
    let client = GmailClient::new(opts, Arc::new(StaticToken("test-token".into()))).unwrap();
    let sync = GmailSync::new(GmailApi::new(client), acc).with_page_size(3);
    Env {
        mock,
        dir,
        w,
        acc,
        sync,
    }
}

impl Env {
    fn reader(&self) -> Reader {
        Reader::open(self.dir.path()).unwrap()
    }

    /// Folder name to the remote IDs of its messages.
    fn by_folder(&self) -> BTreeMap<String, Vec<String>> {
        let r = self.reader();
        let mut out = BTreeMap::new();
        for f in r.folders(self.acc).unwrap() {
            let mut ids: Vec<String> = Vec::new();
            let mut after: Option<Cursor> = None;
            loop {
                let rows = r.messages(f.id, after, 100).unwrap();
                let Some(last) = rows.last() else { break };
                after = Some(Cursor {
                    received_at: last.received_at,
                    id: last.id.0,
                });
                ids.extend(rows.iter().map(|m| m.remote_id.clone()));
            }
            ids.sort();
            out.insert(f.name, ids);
        }
        out
    }

    /// Messages in the store, each once.
    fn total(&self) -> usize {
        let all: std::collections::HashSet<String> =
            self.by_folder().into_values().flatten().collect();
        all.len()
    }

    async fn sync_all(&mut self) -> usize {
        let mut calls = 0;
        loop {
            let r = self
                .sync
                .sync_mail(&mut self.w, Priority::Background)
                .await
                .unwrap();
            calls += 1;
            if !r.more {
                return calls;
            }
        }
    }

    fn message(&self, remote: &str) -> Option<familiar_store::Message> {
        self.reader().message_by_remote(self.acc, remote).unwrap()
    }
}

#[tokio::test]
async fn labels_become_folders() {
    let mut e = env().await;
    e.sync
        .sync_labels(&mut e.w, Priority::Background)
        .await
        .unwrap();
    let folders = e.reader().folders(e.acc).unwrap();
    let find = |remote: &str| folders.iter().find(|f| f.remote_id == remote).unwrap();
    assert_eq!(find("INBOX").well_known.as_deref(), Some("inbox"));
    assert_eq!(find("SENT").well_known.as_deref(), Some("sentitems"));
    assert_eq!(find("TRASH").well_known.as_deref(), Some("deleteditems"));
    assert_eq!(find("SPAM").well_known.as_deref(), Some("junkemail"));
    assert_eq!(find(ALL_MAIL).name, "All Mail");
    // Nested by the `/` in the name.
    let acme = find("Label_2");
    assert_eq!(acme.name, "Acme");
    assert_eq!(acme.parent, Some(find("Label_1").id));
    // Not folders.
    for l in ["UNREAD", "STARRED", "IMPORTANT", "CATEGORY_PERSONAL"] {
        assert!(folders.iter().all(|f| f.remote_id != l), "{l}");
    }

    // A label deleted on Gmail goes; a new one arrives.
    // The receipt is archived with only this label.
    e.sync_all().await;
    let receipt = e.message("msg-receipt").unwrap();
    let mbx = e.mock.mailbox("default");
    mbx.delete_label("Label_3").unwrap();
    mbx.add_label(Label::user("Travel").id("Label_9")).unwrap();
    let r = e
        .sync
        .sync_labels(&mut e.w, Priority::Background)
        .await
        .unwrap();
    assert_eq!(r.removed, 1);
    let names: Vec<String> = e
        .reader()
        .folders(e.acc)
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert!(names.contains(&"Travel".to_string()));
    assert!(!names.contains(&"Receipts".to_string()));
    // Its message stays, with its local ID, in All Mail.
    let kept = e.message("msg-receipt").unwrap();
    assert_eq!(kept.id, receipt.id);
    assert_eq!(
        e.reader().folder(kept.folder).unwrap().unwrap().name,
        "All Mail"
    );
}

#[tokio::test]
async fn first_sync_lists_everything_then_history_follows() {
    let mut e = env().await;
    // 12 messages at 3 a page.
    assert_eq!(e.sync_all().await, 4);
    assert!(e.sync.first_sync_done(&mut e.w).unwrap());
    let f = e.by_folder();
    assert_eq!(
        f["Inbox"],
        [
            "msg-acme-1",
            "msg-acme-3",
            "msg-dinner",
            "msg-photos",
            "msg-promo"
        ]
    );
    assert_eq!(f["Sent"], ["msg-acme-2"]);
    assert_eq!(f["Drafts"], ["msg-draft"]);
    assert_eq!(f["Spam"], ["msg-spam"]);
    assert_eq!(f["Trash"], ["msg-trash"]);
    assert_eq!(f["Receipts"], ["msg-receipt"]);
    assert_eq!(f["Old projects"], ["msg-2015"]);
    // A message is in every label it has.
    assert_eq!(f["Clients"], ["msg-acme-1"]);
    assert_eq!(f["Acme"], ["msg-acme-1", "msg-acme-2", "msg-acme-3"]);
    // All Mail has everything but Spam and Trash.
    assert_eq!(f["All Mail"].len(), 10);
    assert!(f["All Mail"].contains(&"msg-archived".to_string()));
    assert!(!f["All Mail"].contains(&"msg-spam".to_string()));
    let r = e.reader();
    let a1 = e.message("msg-acme-1").unwrap();
    assert_eq!(r.message_folders(a1.id).unwrap().len(), 4);
    drop(r);

    let dinner = e.message("msg-dinner").unwrap();
    assert!(!dinner.is_read);
    assert_eq!(dinner.subject, "Dinner on Friday");
    assert_eq!(dinner.from.addr, "bob@example.com");
    let photos = e.message("msg-photos").unwrap();
    assert!(photos.is_read && photos.is_flagged && photos.has_attachments);
    // One thread across Inbox, Sent and a label.
    let a1 = e.message("msg-acme-1").unwrap();
    let a2 = e.message("msg-acme-2").unwrap();
    assert_eq!(a1.thread, a2.thread);

    // Changes on Gmail arrive by history.
    let mbx = e.mock.mailbox("default");
    mbx.modify("msg-dinner", &[], &["UNREAD", "INBOX"]).unwrap();
    mbx.modify("msg-promo", &["STARRED"], &[]).unwrap();
    mbx.delete_message("msg-spam").unwrap();
    let new = mbx
        .add_message(
            Message::new("Gus <gus@example.com>", "me", "Hello", "New mail.")
                .labels(&["INBOX", "UNREAD"]),
        )
        .unwrap();
    let before = e.mock.requests().len();
    let r = e
        .sync
        .sync_mail(&mut e.w, Priority::Background)
        .await
        .unwrap();
    assert!(!r.more && !r.restarted);
    // Three changed messages read again, one deleted.
    assert_eq!((r.upserted, r.deleted), (3, 1));
    // One history page and three header fetches, no listing.
    assert_eq!(e.mock.requests().len() - before, 4);
    let f = e.by_folder();
    assert!(f["All Mail"].contains(&"msg-dinner".to_string()));
    assert!(!f["Inbox"].contains(&"msg-dinner".to_string()));
    let archived = e.message("msg-dinner").unwrap();
    assert_eq!(
        e.reader().folder(archived.folder).unwrap().unwrap().name,
        "All Mail"
    );
    assert!(f["Inbox"].contains(&new));
    assert!(f["Spam"].is_empty());
    assert!(e.message("msg-dinner").unwrap().is_read);
    assert!(e.message("msg-promo").unwrap().is_flagged);

    // Nothing new: a history call and nothing else.
    let r = e
        .sync
        .sync_mail(&mut e.w, Priority::Background)
        .await
        .unwrap();
    assert_eq!((r.upserted, r.deleted, r.pages), (0, 0, 1));
}

#[tokio::test]
async fn expired_history_lists_again_and_drops_what_went() {
    let mut e = env().await;
    e.sync_all().await;
    let mbx = e.mock.mailbox("default");
    // A week of changes, then Gmail forgets them.
    mbx.modify("msg-photos", &[], &["STARRED"]).unwrap();
    mbx.expire_history();
    mbx.delete_message("msg-promo").unwrap();
    mbx.modify("msg-dinner", &[], &["UNREAD"]).unwrap();
    let r = e
        .sync
        .sync_mail(&mut e.w, Priority::Background)
        .await
        .unwrap();
    assert!(r.restarted && r.more);
    e.sync_all().await;
    assert!(e.message("msg-promo").is_none());
    assert!(e.message("msg-dinner").unwrap().is_read);
    let h = e
        .reader()
        .sync_state(e.acc, HISTORY_RESOURCE)
        .unwrap()
        .unwrap();
    assert_eq!(
        h.delta_link,
        Some(e.mock.mailbox("default").history_id().to_string())
    );
}

#[tokio::test]
async fn an_interrupted_first_sync_resumes_at_its_page() {
    let mut e = env().await;
    let r = e
        .sync
        .sync_mail(&mut e.w, Priority::Background)
        .await
        .unwrap();
    assert!(r.more);
    // famd restarts: a new GmailSync over the same store.
    let api = e.sync.api().clone();
    e.sync = GmailSync::new(api, e.acc).with_page_size(3);
    assert_eq!(e.sync_all().await, 3);
    assert_eq!(e.total(), 12);
}

#[tokio::test]
async fn bodies_come_from_the_raw_message() {
    let mut e = env().await;
    e.sync_all().await;
    let photos = e.message("msg-photos").unwrap();
    let dinner = e.message("msg-dinner").unwrap();
    let out = e
        .sync
        .fetch_bodies(&mut e.w, &[photos.id, dinner.id], Priority::Background)
        .await
        .unwrap();
    assert_eq!(out.len(), 2);
    for (_, r) in &out {
        r.as_ref().unwrap();
    }
    let r = e.reader();
    let m = r.message(photos.id).unwrap().unwrap();
    let body = r.body(&m.body.unwrap().hash).unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("Photos attached."), "{text}");
    assert!(!text.contains("mock-jpeg"));
}

#[tokio::test]
async fn throttling_waits_and_retries() {
    let mut e = env().await;
    e.mock
        .push_fault(Fault::rate_limited().on_path("/messages").times(1));
    e.mock
        .push_fault(Fault::user_rate_limited().on_path("/profile").times(1));
    e.sync_all().await;
    assert_eq!(e.total(), 12);
}

#[tokio::test]
async fn server_search_uses_gmail_syntax() {
    let e = env().await;
    let found = e
        .sync
        .api()
        .search("from:ann quote", 10, Priority::User)
        .await
        .unwrap();
    let ids: Vec<&str> = found.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"msg-acme-1"), "{ids:?}");
    assert!(!ids.contains(&"msg-dinner"));
    assert!(found.iter().all(|m| m.header("Subject").is_some()));
}

#[tokio::test]
async fn an_interrupted_resync_starts_over() {
    let mut e = env().await;
    e.sync_all().await;
    let mbx = e.mock.mailbox("default");
    mbx.modify("msg-photos", &[], &["STARRED"]).unwrap();
    mbx.expire_history();
    mbx.delete_message("msg-promo").unwrap();
    let r = e
        .sync
        .sync_mail(&mut e.w, Priority::Background)
        .await
        .unwrap();
    assert!(r.restarted && r.more);
    // famd restarts mid-resync: the listing starts over, so it still
    // knows everything Gmail has and drops what went.
    let api = e.sync.api().clone();
    e.sync = GmailSync::new(api, e.acc).with_page_size(3);
    e.sync_all().await;
    assert!(e.message("msg-promo").is_none());
    assert_eq!(e.total(), 11);
}
