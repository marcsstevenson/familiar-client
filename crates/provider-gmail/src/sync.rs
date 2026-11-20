// SPDX-License-Identifier: MIT OR Apache-2.0
//! Applying Gmail to the store through its single writer.
//!
//! # Labels as folders
//!
//! Gmail's labels become folders: `INBOX`, `SENT`, `DRAFT`, `TRASH` and
//! `SPAM` get the well-known names the rest of Familiar uses, user labels
//! nest by the `/` in their names, and Gmail's other system labels
//! (`UNREAD`, `STARRED`, `IMPORTANT`, the categories) are not folders:
//! `UNREAD` and `STARRED` are the read and flag state. A message is in every
//! folder it has a label for, and in the store's All Mail folder unless it
//! is in Trash or Spam, so archived mail with no label is still there.
//!
//! # First sync and history
//!
//! Gmail has one change feed per mailbox. The first sync records the
//! profile's history ID, then lists every message, newest first, a page per
//! call, fetching each one's labels and headers. Its progress is kept in
//! [`LISTING_RESOURCE`], so an interrupted first sync resumes at its page.
//! Once the listing is done, [`HISTORY_RESOURCE`] holds the history ID and
//! each later call reads `history.list` from it. When Gmail no longer keeps
//! that much history (404), the listing runs again and local messages it
//! didn't see are removed.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use familiar_provider::{Error, Priority, Result};
use familiar_store::{
    AccountId, BodyRef, Cursor, FolderId, MessageId, NewFolder, Reader, StoreWriter, WriteTx,
};

use crate::api::{GmailApi, LIST_PAGE_SIZE};
use crate::mime::{body_from_raw, to_new_message};
use crate::model::{GmailLabel, GmailMessage};

/// Sync state for the label tree.
pub const LABELS_RESOURCE: &str = "labels";
/// Sync state holding the history ID once the first listing is done.
pub const HISTORY_RESOURCE: &str = "history";
/// Sync state of a listing in progress: `<history id> <page token>`.
pub const LISTING_RESOURCE: &str = "listing";
/// Remote ID of the All Mail folder, which has no Gmail label.
pub const ALL_MAIL: &str = familiar_store::ALL_MAIL_REMOTE_ID;

/// Gmail's system labels that are folders, with their well-known names and
/// display names.
pub const SYSTEM_FOLDERS: &[(&str, &str, &str)] = &[
    ("INBOX", "inbox", "Inbox"),
    ("SENT", "sentitems", "Sent"),
    ("DRAFT", "drafts", "Drafts"),
    ("SPAM", "junkemail", "Spam"),
    ("TRASH", "deleteditems", "Trash"),
];

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Whether a label ID is one of Gmail's own (`INBOX`, `CATEGORY_SOCIAL`);
/// user labels have IDs like `Label_12`.
pub fn is_system_id(id: &str) -> bool {
    id.chars().all(|c| c.is_ascii_uppercase() || c == '_')
}

/// Whether a label is shown as a folder.
pub fn is_folder_label(l: &GmailLabel) -> bool {
    if l.is_system() {
        SYSTEM_FOLDERS.iter().any(|(id, _, _)| *id == l.id)
    } else {
        true
    }
}

/// The folder a message shows as being in (the store's primary location):
/// Trash, Spam, Drafts, Inbox, Sent, its first user label, else All Mail.
pub fn primary_label<'a>(labels: &'a [String], folders: &HashSet<String>) -> &'a str {
    for want in ["TRASH", "SPAM", "DRAFT", "INBOX", "SENT"] {
        if let Some(l) = labels.iter().find(|l| *l == want) {
            return l;
        }
    }
    labels
        .iter()
        .find(|l| folders.contains(l.as_str()) && !is_system_id(l))
        .map_or(ALL_MAIL, String::as_str)
}

/// What a label sync changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LabelReport {
    /// Folders added or updated.
    pub upserted: usize,
    /// Folders removed.
    pub removed: usize,
}

/// What one mail sync call changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MailReport {
    /// Messages added or updated.
    pub upserted: usize,
    /// Messages removed locally.
    pub deleted: usize,
    /// History pages or listing pages read.
    pub pages: usize,
    /// Whether the first listing (or a resync) still has pages to go.
    pub more: bool,
    /// Whether history had expired and a full listing started over.
    pub restarted: bool,
}

/// Syncs one Gmail account's mail into the store.
#[derive(Debug)]
pub struct GmailSync {
    api: GmailApi,
    account: AccountId,
    page_size: usize,
    /// Remote IDs seen by a listing that started from scratch in this run,
    /// so its last page can remove what Gmail no longer has.
    seen: Option<HashSet<String>>,
}

impl GmailSync {
    /// Syncs `account` (a store account ID) through `api`.
    pub fn new(api: GmailApi, account: AccountId) -> Self {
        Self {
            api,
            account,
            page_size: LIST_PAGE_SIZE,
            seen: None,
        }
    }

    /// Message IDs per listing page.
    pub fn with_page_size(mut self, n: usize) -> Self {
        self.page_size = n.clamp(1, LIST_PAGE_SIZE);
        self
    }

    /// The Gmail reads underneath.
    pub fn api(&self) -> &GmailApi {
        &self.api
    }

    /// The account being synced.
    pub fn account(&self) -> AccountId {
        self.account
    }

    /// Whether the first listing has finished.
    pub fn first_sync_done<W: StoreWriter>(&self, w: &mut W) -> Result<bool> {
        let acc = self.account;
        Ok(w.write(|tx| tx.sync_state(acc, HISTORY_RESOURCE))?
            .and_then(|s| s.delta_link)
            .is_some())
    }

    // Labels ----------------------------------------------------------------

    /// Brings the folder tree in line with Gmail's labels.
    pub async fn sync_labels<W: StoreWriter>(
        &mut self,
        w: &mut W,
        priority: Priority,
    ) -> Result<LabelReport> {
        let acc = self.account;
        let mut labels: Vec<GmailLabel> = self
            .api
            .labels(priority)
            .await?
            .into_iter()
            .filter(is_folder_label)
            .collect();
        // Parents before children.
        labels.sort_by_key(|l| l.name.matches('/').count());
        let by_name: HashMap<String, String> = labels
            .iter()
            .filter(|l| !l.is_system())
            .map(|l| (l.name.clone(), l.id.clone()))
            .collect();
        let mut report = LabelReport::default();
        let mut keep: HashSet<String> = labels.iter().map(|l| l.id.clone()).collect();
        keep.insert(ALL_MAIL.to_string());
        // Messages in labels that are going: they lose that label, not
        // their local copy (deleting a folder deletes the messages whose
        // primary location it is).
        let mut leaving: Vec<(FolderId, MessageId)> = Vec::new();
        let reader = Reader::open(&w.paths().root)?;
        for f in reader.folders(acc)? {
            if keep.contains(&f.remote_id) {
                continue;
            }
            let mut after = None;
            loop {
                let rows = reader.messages(f.id, after, 1000)?;
                let Some(last) = rows.last() else { break };
                after = Some(Cursor {
                    received_at: last.received_at,
                    id: last.id.0,
                });
                leaving.extend(rows.iter().map(|m| (f.id, m.id)));
            }
        }
        drop(reader);
        w.write(|tx| {
            let busy = tx.open_op_targets(acc)?;
            let all_mail = tx.ensure_all_mail(acc)?;
            for l in &labels {
                if let Some(f) = tx.folder_by_remote(acc, &l.id)? {
                    if busy.contains(&familiar_store::folder_target(f.id)) {
                        continue;
                    }
                }
                let (well_known, name, parent) =
                    match SYSTEM_FOLDERS.iter().find(|(id, _, _)| *id == l.id) {
                        Some((_, wk, name)) => (Some((*wk).to_string()), (*name).to_string(), None),
                        None => match l.name.rsplit_once('/') {
                            Some((up, leaf)) => match by_name.get(up) {
                                Some(pid) => (
                                    None,
                                    leaf.to_string(),
                                    tx.folder_by_remote(acc, pid)?.map(|f| f.id),
                                ),
                                None => (None, l.name.clone(), None),
                            },
                            None => (None, l.name.clone(), None),
                        },
                    };
                tx.upsert_folder(&NewFolder {
                    account: acc,
                    remote_id: l.id.clone(),
                    parent,
                    name,
                    well_known,
                })?;
                report.upserted += 1;
            }
            for f in tx.folders(acc)? {
                if keep.contains(&f.remote_id)
                    || f.remote_id.starts_with(familiar_store::LOCAL_REMOTE_PREFIX)
                    || busy.contains(&familiar_store::folder_target(f.id))
                {
                    continue;
                }
                for (folder, m) in &leaving {
                    if *folder == f.id {
                        let rest: Vec<FolderId> = tx
                            .message_folders(*m)?
                            .into_iter()
                            .filter(|x| *x != f.id && *x != all_mail)
                            .collect();
                        tx.set_message_folders(*m, &rest)?;
                    }
                }
                tx.delete_folder(f.id)?;
                report.removed += 1;
            }
            tx.set_sync_state(acc, LABELS_RESOURCE, None, None, now())
        })?;
        Ok(report)
    }

    fn folder_ids<W: StoreWriter>(&self, w: &mut W) -> Result<HashMap<String, FolderId>> {
        let acc = self.account;
        Ok(w.write(|tx| tx.folders(acc))?
            .into_iter()
            .map(|f| (f.remote_id, f.id))
            .collect())
    }

    // Mail ------------------------------------------------------------------

    /// One step of mail sync: a page of the first listing (or of a resync),
    /// or every page of history since the last call. Call again while
    /// [`MailReport::more`] is set.
    pub async fn sync_mail<W: StoreWriter>(
        &mut self,
        w: &mut W,
        priority: Priority,
    ) -> Result<MailReport> {
        let acc = self.account;
        let history = w
            .write(|tx| tx.sync_state(acc, HISTORY_RESOURCE))?
            .and_then(|s| s.delta_link);
        match history {
            Some(h) => match self.history_round(w, &h, priority).await {
                Err(Error::SyncStateGone(_)) => {
                    w.write(|tx| tx.reset_sync_state(acc, HISTORY_RESOURCE))?;
                    let mut r = self.listing_page(w, true, priority).await?;
                    r.restarted = true;
                    Ok(r)
                }
                r => r,
            },
            None => self.listing_page(w, false, priority).await,
        }
    }

    /// One listing page. `resync` is set when history expired: that
    /// listing must see every page in one run to know what to remove, so a
    /// resync interrupted by a restart starts over (a first sync resumes).
    async fn listing_page<W: StoreWriter>(
        &mut self,
        w: &mut W,
        resync: bool,
        priority: Priority,
    ) -> Result<MailReport> {
        let acc = self.account;
        let mut saved = w
            .write(|tx| tx.sync_state(acc, LISTING_RESOURCE))?
            .and_then(|s| s.delta_link);
        let mut resync = resync;
        if let Some(mark) = &saved {
            if mark.ends_with(" resync") {
                resync = true;
                if self.seen.is_none() {
                    saved = None;
                }
            }
        }
        let (start, token) = match saved.as_deref().and_then(|s| s.split_once(' ')) {
            Some((h, t)) => (
                h.to_string(),
                Some(t.trim_end_matches(" resync").to_string()),
            ),
            None => {
                // History from before the listing, so nothing that changes
                // while it runs is missed.
                let p = self.api.profile(priority).await?;
                self.seen = Some(HashSet::new());
                if self.folder_ids(w)?.is_empty() {
                    self.sync_labels(w, priority).await?;
                }
                (p.history_id, None)
            }
        };
        let mut report = MailReport {
            pages: 1,
            ..Default::default()
        };
        let page = self
            .api
            .list(None, None, self.page_size, token.as_deref(), priority)
            .await?;
        let ids: Vec<String> = page.messages.iter().map(|m| m.id.clone()).collect();
        if let Some(seen) = &mut self.seen {
            seen.extend(ids.iter().cloned());
        }
        let got = self.fetch_metadata(&ids, priority).await?;
        report.upserted += self.store_headers(w, &got, priority).await?;
        match page.next_page_token {
            Some(next) => {
                let mark = if resync {
                    format!("{start} {next} resync")
                } else {
                    format!("{start} {next}")
                };
                w.write(|tx| tx.set_sync_state(acc, LISTING_RESOURCE, None, Some(&mark), now()))?;
                report.more = true;
            }
            None => {
                if let Some(seen) = self.seen.take() {
                    report.deleted += self.remove_unseen(w, &seen)?;
                }
                w.write(|tx| {
                    tx.reset_sync_state(acc, LISTING_RESOURCE)?;
                    tx.set_sync_state(acc, HISTORY_RESOURCE, None, Some(&start), now())
                })?;
            }
        }
        Ok(report)
    }

    async fn fetch_metadata(
        &self,
        ids: &[String],
        priority: Priority,
    ) -> Result<Vec<GmailMessage>> {
        self.api.metadata_many(ids, priority).await
    }

    /// Stores headers, each in its primary folder. Labels the store doesn't
    /// know yet bring the label tree up to date first.
    async fn store_headers<W: StoreWriter>(
        &mut self,
        w: &mut W,
        msgs: &[GmailMessage],
        priority: Priority,
    ) -> Result<usize> {
        let mut folders = self.folder_ids(w)?;
        let unknown = msgs.iter().any(|m| {
            m.label_ids
                .iter()
                .any(|l| !folders.contains_key(l) && !is_system_id(l))
        });
        if unknown {
            self.sync_labels(w, priority).await?;
            folders = self.folder_ids(w)?;
        }
        let names: HashSet<String> = folders.keys().cloned().collect();
        let acc = self.account;
        let mut n = 0;
        w.write(|tx| {
            let busy = tx.open_op_targets(acc)?;
            for m in msgs {
                let primary = primary_label(&m.label_ids, &names);
                let Some(&folder) = folders.get(primary).or_else(|| folders.get(ALL_MAIL)) else {
                    continue;
                };
                if let Some(local) = tx.message_by_remote(acc, &m.id)? {
                    if busy.contains(&familiar_store::message_target(local.id)) {
                        continue;
                    }
                }
                let id = tx.upsert_message(&to_new_message(m, acc, folder))?;
                tx.set_message_folders(id, &label_folders(&m.label_ids, &folders))?;
                n += 1;
            }
            Ok(())
        })?;
        Ok(n)
    }

    /// Removes local messages a full listing didn't see.
    fn remove_unseen<W: StoreWriter>(&self, w: &mut W, seen: &HashSet<String>) -> Result<usize> {
        let acc = self.account;
        let reader = Reader::open(&w.paths().root)?;
        let mut gone = Vec::new();
        let mut checked = HashSet::new();
        for f in reader.folders(acc)? {
            let mut after = None;
            loop {
                let rows = reader.messages(f.id, after, 1000)?;
                let Some(last) = rows.last() else { break };
                after = Some(Cursor {
                    received_at: last.received_at,
                    id: last.id.0,
                });
                for m in &rows {
                    if checked.insert(m.id)
                        && !seen.contains(&m.remote_id)
                        && !m.remote_id.starts_with(familiar_store::LOCAL_REMOTE_PREFIX)
                    {
                        gone.push(m.id);
                    }
                }
            }
        }
        drop(reader);
        delete_local(w, acc, &gone)
    }

    async fn history_round<W: StoreWriter>(
        &mut self,
        w: &mut W,
        start: &str,
        priority: Priority,
    ) -> Result<MailReport> {
        let acc = self.account;
        let mut report = MailReport::default();
        let mut token: Option<String> = None;
        // History records name what changed but don't reliably carry a
        // message's labels after the change, so changed messages are read
        // again; deletions are applied as they are.
        let mut changed: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut deleted: HashSet<String> = HashSet::new();
        let latest = loop {
            let page = self.api.history(start, token.as_deref(), priority).await?;
            report.pages += 1;
            for rec in page.history {
                for m in rec
                    .messages_added
                    .into_iter()
                    .chain(rec.labels_added)
                    .chain(rec.labels_removed)
                {
                    deleted.remove(&m.message.id);
                    if seen.insert(m.message.id.clone()) {
                        changed.push(m.message.id);
                    }
                }
                for m in rec.messages_deleted {
                    deleted.insert(m.message.id);
                }
            }
            match page.next_page_token {
                Some(t) => token = Some(t),
                None => break page.history_id.unwrap_or_else(|| start.to_string()),
            }
        };
        changed.retain(|id| !deleted.contains(id));

        let got = self.fetch_metadata(&changed, priority).await?;
        // Deleted for good, or gone by the time it was read.
        let found: HashSet<&str> = got.iter().map(|m| m.id.as_str()).collect();
        let gone: Vec<MessageId> = w.write(|tx| {
            let mut ids = Vec::new();
            for r in deleted
                .iter()
                .chain(changed.iter().filter(|r| !found.contains(r.as_str())))
            {
                if let Some(m) = tx.message_by_remote(acc, r)? {
                    ids.push(m.id);
                }
            }
            Ok(ids)
        })?;
        report.deleted += delete_local(w, acc, &gone)?;
        report.upserted += self.store_headers(w, &got, priority).await?;
        w.write(|tx| tx.set_sync_state(acc, HISTORY_RESOURCE, None, Some(&latest), now()))?;
        Ok(report)
    }

    // Bodies ----------------------------------------------------------------

    /// Downloads and stores one message's body.
    pub async fn fetch_body<W: StoreWriter>(
        &self,
        w: &mut W,
        message: MessageId,
        priority: Priority,
    ) -> Result<BodyRef> {
        let remote = w
            .write(|tx| tx.message(message))?
            .ok_or_else(|| Error::NotFound(format!("message {}", message.0)))?
            .remote_id;
        let raw = self.api.raw(&remote, priority).await?;
        store_body(w, message, &raw)
    }

    /// Downloads several bodies at once and stores each as it arrives.
    /// Returns one result per message, in completion order.
    pub async fn fetch_bodies<W: StoreWriter>(
        &self,
        w: &mut W,
        messages: &[MessageId],
        priority: Priority,
    ) -> Result<Vec<(MessageId, Result<BodyRef>)>> {
        let mut set = tokio::task::JoinSet::new();
        for &id in messages {
            let Some(m) = w.write(|tx| tx.message(id))? else {
                continue;
            };
            let api = self.api.clone();
            set.spawn(async move { (id, api.raw(&m.remote_id, priority).await) });
        }
        let mut out = Vec::with_capacity(messages.len());
        while let Some(done) = set.join_next().await {
            let (id, raw) = done.map_err(|e| Error::Decode(format!("body task failed: {e}")))?;
            let r = match raw {
                Ok(raw) => store_body(w, id, &raw),
                Err(e) => Err(e),
            };
            out.push((id, r));
        }
        Ok(out)
    }
}

/// The folders for a message's labels, All Mail left to the store.
///
/// A message in Trash or Spam is shown there only, as Gmail shows it:
/// its other labels stay on Gmail but it leaves those folders.
fn label_folders(labels: &[String], folders: &HashMap<String, FolderId>) -> Vec<FolderId> {
    for hidden in ["TRASH", "SPAM"] {
        if labels.iter().any(|l| l == hidden) {
            return folders.get(hidden).copied().into_iter().collect();
        }
    }
    labels
        .iter()
        .filter(|l| l.as_str() != ALL_MAIL)
        .filter_map(|l| folders.get(l).copied())
        .collect()
}

/// Parses `raw` and stores its body for `message`.
pub fn store_body<W: StoreWriter>(w: &mut W, message: MessageId, raw: &[u8]) -> Result<BodyRef> {
    let body = body_from_raw(raw);
    Ok(w.write(|tx| {
        let hash = tx.put_body(body.content.as_bytes())?;
        let r = BodyRef {
            hash,
            is_html: body.is_html,
        };
        tx.set_body(message, Some(r))?;
        Ok(r)
    })?)
}

/// Deletes local messages, except ones with local changes still queued.
fn delete_local<W: StoreWriter>(w: &mut W, acc: AccountId, ids: &[MessageId]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    Ok(w.write(|tx: &WriteTx<'_>| {
        let busy = tx.open_op_targets(acc)?;
        let mut n = 0;
        for &id in ids {
            if !busy.contains(&familiar_store::message_target(id)) {
                tx.delete_message(id)?;
                n += 1;
            }
        }
        Ok(n)
    })?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_folder_order() {
        let folders: HashSet<String> = ["INBOX", "SENT", "Label_1", "Label_2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            primary_label(&l(&["UNREAD", "Label_1", "INBOX"]), &folders),
            "INBOX"
        );
        assert_eq!(primary_label(&l(&["INBOX", "TRASH"]), &folders), "TRASH");
        assert_eq!(
            primary_label(&l(&["IMPORTANT", "Label_2"]), &folders),
            "Label_2"
        );
        assert_eq!(primary_label(&l(&["CATEGORY_SOCIAL"]), &folders), ALL_MAIL);
        assert_eq!(primary_label(&l(&["Label_9"]), &folders), ALL_MAIL);
    }
}
