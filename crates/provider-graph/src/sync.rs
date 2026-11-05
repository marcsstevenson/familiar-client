// SPDX-License-Identifier: MIT OR Apache-2.0
//! Applying Graph mail to the store through its single writer.
//!
//! [`MailSync`] runs inside `famd`, which owns the store's [`Writer`]. Each
//! call takes the writer by `&mut`, does its Graph reads through the
//! mailbox's scheduler, and commits what it learned in short transactions,
//! so readers see progress page by page.
//!
//! # Delta and saved links
//!
//! The folder tree and each folder's messages are synced by Graph delta.
//! Bookmarks live in the store's `sync_state`: resource `folders` for the
//! tree and `messages:<folder remote id>` per folder. While a round is being
//! paged the bookmark holds the `nextLink`, so an interrupted first sync of a
//! big folder resumes where it stopped; at the end of a round it holds the
//! `deltaLink`. A 410 (`SyncStateNotFound`) forgets the bookmark and runs a
//! full round, after which local messages the server no longer listed are
//! treated as removed.
//!
//! # Moves
//!
//! Graph reports a move as `@removed` in the old folder and a new item in
//! the new one. Because every request asks for immutable IDs, the new item
//! has the same ID, and upserting it moves the local row and keeps its body.
//! Removals are therefore held back until every folder in the call has been
//! read; one whose message is still in the folder that reported it is then
//! checked with a GET by immutable ID. If the message still exists it is
//! moved locally to the folder Graph names; only a 404 deletes it.
//!
//! When the ID did change (a move between mailboxes, such as to an online
//! archive), the Internet message ID links the two: a deleted message's body
//! is given to a local copy with the same Internet message ID that has none,
//! or kept for a while so a later arrival with that ID gets it without a
//! download.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use familiar_store::{
    AccountId, BodyRef, Folder, FolderId, MessageId, NewFolder, Reader, StoreWriter, WriteTx,
};

use crate::error::{Error, Result};
use crate::mail::{DELTA_PAGE_SIZE, MailApi};
use crate::model::{DeltaItem, GraphFolder, GraphMessage, WELL_KNOWN_FOLDERS};
use crate::scheduler::Priority;

/// Sync bookmark for the folder tree.
pub const FOLDERS_RESOURCE: &str = "folders";

/// Sync bookmark for one folder's messages.
pub fn messages_resource(folder_remote_id: &str) -> String {
    format!("messages:{folder_remote_id}")
}

/// Bodies of deleted messages kept for matching by Internet message ID.
const STASH_LIMIT: usize = 4096;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// What a folder-tree sync changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderTreeReport {
    /// Folders added or updated.
    pub upserted: usize,
    /// Folders deleted locally.
    pub removed: usize,
    /// Whether this was a full round (first sync, or after a 410).
    pub full_round: bool,
}

/// What a message sync changed, over all the folders in the call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageSyncReport {
    /// Messages added or updated (moves into a synced folder included).
    pub upserted: usize,
    /// Messages found in another folder by immutable ID after a removal.
    pub moved: usize,
    /// Messages deleted locally.
    pub deleted: usize,
    /// Bodies carried over to another copy by Internet message ID.
    pub bodies_carried: usize,
    /// Delta pages read.
    pub pages: usize,
    /// Folders that ran a full round.
    pub full_rounds: usize,
    /// Folders whose bookmark had expired (410) and were started over.
    pub restarted: usize,
}

impl MessageSyncReport {
    fn add(&mut self, o: &MessageSyncReport) {
        self.upserted += o.upserted;
        self.moved += o.moved;
        self.deleted += o.deleted;
        self.bodies_carried += o.bodies_carried;
        self.pages += o.pages;
        self.full_rounds += o.full_rounds;
        self.restarted += o.restarted;
    }
}

#[derive(Clone, Copy, Debug)]
struct Stashed {
    body: BodyRef,
    pinned: bool,
}

/// Syncs one account's mail into the store.
#[derive(Debug)]
pub struct MailSync {
    api: MailApi,
    account: AccountId,
    page_size: usize,
    stash: HashMap<String, Stashed>,
    stash_order: VecDeque<String>,
}

impl MailSync {
    /// Syncs `account` (a store account ID) through `api`.
    pub fn new(api: MailApi, account: AccountId) -> Self {
        Self {
            api,
            account,
            page_size: DELTA_PAGE_SIZE,
            stash: HashMap::new(),
            stash_order: VecDeque::new(),
        }
    }

    /// Messages per delta page.
    pub fn with_page_size(mut self, n: usize) -> Self {
        self.page_size = n.max(1);
        self
    }

    /// The Graph reads underneath.
    pub fn api(&self) -> &MailApi {
        &self.api
    }

    /// The account being synced.
    pub fn account(&self) -> AccountId {
        self.account
    }

    // Folder tree -----------------------------------------------------------

    /// Brings the local folder tree up to date by folder delta.
    pub async fn sync_folder_tree<W: StoreWriter>(
        &mut self,
        w: &mut W,
        priority: Priority,
    ) -> Result<FolderTreeReport> {
        let acc = self.account;
        let saved = w.write(|tx| tx.sync_state(acc, FOLDERS_RESOURCE))?;
        let mut link = saved.and_then(|s| s.delta_link);
        let mut full = link.is_none();
        let mut restarted = false;
        let mut upserts: Vec<GraphFolder> = Vec::new();
        let mut removed: Vec<String> = Vec::new();
        let delta_link = loop {
            let page = match self.api.folder_delta_page(link.as_deref(), priority).await {
                Err(Error::SyncStateGone(_)) if !restarted => {
                    restarted = true;
                    full = true;
                    link = None;
                    upserts.clear();
                    removed.clear();
                    continue;
                }
                r => r?,
            };
            for item in page.items {
                match item {
                    DeltaItem::Upsert(f) if f.is_hidden => removed.push(f.id),
                    DeltaItem::Upsert(f) => upserts.push(f),
                    DeltaItem::Removed(id) => removed.push(id),
                }
            }
            if let Some(d) = page.delta_link {
                break d;
            }
            link = Some(page.next_link.ok_or_else(|| {
                Error::Decode("folder delta page with neither nextLink nor deltaLink".into())
            })?);
        };

        // A later entry for the same folder wins.
        let mut seen = HashSet::new();
        upserts.reverse();
        upserts.retain(|f| seen.insert(f.id.clone()));
        upserts.reverse();
        removed.retain(|id| !seen.contains(id));

        let local = Reader::open(&w.paths().root)?.folders(acc)?;
        if full {
            // Anything the full round didn't list is gone.
            for f in &local {
                if !seen.contains(&f.remote_id) {
                    removed.push(f.remote_id.clone());
                }
            }
        }

        // Well-known names aren't in v1.0 folder JSON; ask for them by name
        // on a full round, or when Inbox isn't known yet.
        let mut well_known: HashMap<String, String> = HashMap::new();
        let have_inbox = local
            .iter()
            .any(|f| f.well_known.as_deref() == Some("inbox"));
        if full || !have_inbox {
            for name in WELL_KNOWN_FOLDERS {
                if let Some(id) = self.api.well_known_folder(name, priority).await? {
                    well_known.insert(id, (*name).to_string());
                }
            }
        }

        let mut report = FolderTreeReport {
            full_round: full,
            ..Default::default()
        };
        w.write(|tx| {
            // Folders with queued local changes keep them until they land.
            let busy = tx.open_op_targets(acc)?;
            let keep = |id: &str| -> familiar_store::Result<bool> {
                Ok(match tx.folder_by_remote(acc, id)? {
                    Some(f) => !busy.contains(&familiar_store::folder_target(f.id)),
                    None => true,
                })
            };
            let mut ready = Vec::with_capacity(upserts.len());
            for f in &upserts {
                if keep(&f.id)? {
                    ready.push(f.clone());
                }
            }
            report.upserted = apply_folders(tx, acc, &ready, &well_known)?;
            for id in &removed {
                if id.starts_with(familiar_store::LOCAL_REMOTE_PREFIX) || !keep(id)? {
                    continue;
                }
                if let Some(f) = tx.folder_by_remote(acc, id)? {
                    tx.delete_folder(f.id)?;
                    report.removed += 1;
                }
            }
            tx.set_sync_state(acc, FOLDERS_RESOURCE, None, Some(&delta_link), now())
        })?;
        Ok(report)
    }

    // Messages --------------------------------------------------------------

    /// Syncs one folder's messages by delta. See [`MailSync::sync_folders`].
    pub async fn sync_folder<W: StoreWriter>(
        &mut self,
        w: &mut W,
        folder_remote_id: &str,
        priority: Priority,
    ) -> Result<MessageSyncReport> {
        self.sync_folders(w, &[folder_remote_id], priority).await
    }

    /// Syncs several folders' messages by delta, then settles removals: a
    /// message that left one folder for another in the same call is simply
    /// moved, and any other removal is checked by immutable ID first.
    pub async fn sync_folders<W: StoreWriter>(
        &mut self,
        w: &mut W,
        folder_remote_ids: &[&str],
        priority: Priority,
    ) -> Result<MessageSyncReport> {
        let mut report = MessageSyncReport::default();
        let mut removals: Vec<(FolderId, String)> = Vec::new();
        for remote in folder_remote_ids {
            let r = self.delta_round(w, remote, priority, &mut removals).await?;
            report.add(&r);
        }
        let r = self.settle_removals(w, removals, priority).await?;
        report.add(&r);
        Ok(report)
    }

    fn local_folder<W: StoreWriter>(&self, w: &mut W, remote: &str) -> Result<Folder> {
        let acc = self.account;
        w.write(|tx| tx.folder_by_remote(acc, remote))?
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "folder {remote} is not in the store; sync the folder tree first"
                ))
            })
    }

    async fn delta_round<W: StoreWriter>(
        &mut self,
        w: &mut W,
        remote: &str,
        priority: Priority,
        removals: &mut Vec<(FolderId, String)>,
    ) -> Result<MessageSyncReport> {
        let acc = self.account;
        let folder = self.local_folder(w, remote)?;
        let resource = messages_resource(remote);
        let mut report = MessageSyncReport::default();
        let mut link = w
            .write(|tx| tx.sync_state(acc, &resource))?
            .and_then(|s| s.delta_link);
        let mut full = link.is_none();
        let mut seen: HashSet<String> = HashSet::new();
        let mut my_removals: Vec<String> = Vec::new();
        loop {
            let page = match self
                .api
                .message_delta_page(remote, link.as_deref(), self.page_size, priority)
                .await
            {
                Err(Error::SyncStateGone(_)) if report.restarted == 0 => {
                    report.restarted = 1;
                    full = true;
                    link = None;
                    seen.clear();
                    my_removals.clear();
                    w.write(|tx| tx.reset_sync_state(acc, &resource))?;
                    continue;
                }
                r => r?,
            };
            report.pages += 1;
            let bookmark = page.delta_link.clone().or_else(|| page.next_link.clone());
            let Some(bookmark) = bookmark else {
                return Err(Error::Decode(
                    "message delta page with neither nextLink nor deltaLink".into(),
                ));
            };
            // Outlook.com's incremental rounds can list a changed message with
            // only its ID and the changed state (no received time, subject or
            // conversation). Storing that as a header would blank the row, so
            // fetch the full header for such items first.
            let mut items = page.items;
            for item in &mut items {
                if let DeltaItem::Upsert(m) = item {
                    if m.received_date_time.is_none() {
                        // Gone, or moved on since: settle it like a removal.
                        *item = match self.api.message(&m.id, priority).await? {
                            Some(full)
                                if full.parent_folder_id.as_deref()
                                    == Some(folder.remote_id.as_str()) =>
                            {
                                DeltaItem::Upsert(full)
                            }
                            _ => DeltaItem::Removed(m.id.clone()),
                        };
                    }
                }
            }
            let stash = &mut self.stash;
            w.write(|tx| {
                for item in &items {
                    match item {
                        DeltaItem::Upsert(m) => {
                            upsert_header(tx, acc, folder.id, m, stash)?;
                            seen.insert(m.id.clone());
                            report.upserted += 1;
                        }
                        DeltaItem::Removed(id) => my_removals.push(id.clone()),
                    }
                }
                tx.set_sync_state(acc, &resource, Some(folder.id), Some(&bookmark), now())
            })?;
            if page.delta_link.is_some() {
                break;
            }
            link = Some(bookmark);
        }
        if full {
            report.full_rounds = 1;
            // After a full round, local messages the server didn't list have
            // left the folder while we weren't looking.
            let reader = Reader::open(&w.paths().root)?;
            let mut after = None;
            loop {
                let rows = reader.messages(folder.id, after, 1000)?;
                let Some(last) = rows.last() else { break };
                after = Some(familiar_store::Cursor {
                    received_at: last.received_at,
                    id: last.id.0,
                });
                for m in &rows {
                    if !seen.contains(&m.remote_id) {
                        my_removals.push(m.remote_id.clone());
                    }
                }
            }
        }
        removals.extend(my_removals.into_iter().map(|id| (folder.id, id)));
        Ok(report)
    }

    async fn settle_removals<W: StoreWriter>(
        &mut self,
        w: &mut W,
        removals: Vec<(FolderId, String)>,
        priority: Priority,
    ) -> Result<MessageSyncReport> {
        let acc = self.account;
        let mut report = MessageSyncReport::default();
        for (folder, id) in removals {
            if id.starts_with(familiar_store::LOCAL_REMOTE_PREFIX) {
                // Made here and not on the server yet (a draft): its op settles it.
                continue;
            }
            let Some(local) = w.write(|tx| tx.message_by_remote(acc, &id))? else {
                continue;
            };
            if local.folder != folder {
                // Already moved: it arrived in another synced folder this round.
                continue;
            }
            let busy = familiar_store::message_target(local.id);
            if w.write(|tx| Ok(tx.open_op_targets(acc)?.contains(&busy)))? {
                // A queued local change (a move, say) will settle it.
                continue;
            }
            let now_at = self.api.message(&id, priority).await?;
            let dest = match &now_at {
                Some(m) => match m.parent_folder_id.as_deref() {
                    Some(p) => w.write(|tx| tx.folder_by_remote(acc, p))?,
                    None => None,
                },
                None => None,
            };
            match (now_at, dest) {
                (Some(m), Some(dest)) if dest.id != folder => {
                    let stash = &mut self.stash;
                    w.write(|tx| upsert_header(tx, acc, dest.id, &m, stash).map(|_| ()))?;
                    report.moved += 1;
                }
                (Some(_), Some(_)) => {
                    // Graph says it's still here (a removal and re-add); keep it.
                }
                _ => {
                    // Deleted, or moved somewhere Familiar doesn't sync.
                    let carried = w.write(|tx| {
                        let carried = carry_body(tx, acc, local.id)?;
                        tx.delete_message(local.id)?;
                        Ok(carried)
                    })?;
                    match carried {
                        Carry::Given => report.bodies_carried += 1,
                        Carry::Keep(imid, s) => self.remember(imid, s),
                        Carry::Nothing => {}
                    }
                    report.deleted += 1;
                }
            }
        }
        Ok(report)
    }

    fn remember(&mut self, imid: String, s: Stashed) {
        if self.stash.insert(imid.clone(), s).is_none() {
            self.stash_order.push_back(imid);
        }
        while self.stash_order.len() > STASH_LIMIT {
            if let Some(old) = self.stash_order.pop_front() {
                self.stash.remove(&old);
            }
        }
    }

    // Header paging ---------------------------------------------------------

    /// Fetches one page of a folder's headers, newest first, and stores it.
    /// Returns the link to the next page. The delta round lists every
    /// message too; this is for showing the newest mail of a folder before
    /// its first delta round has finished.
    pub async fn store_headers_page<W: StoreWriter>(
        &mut self,
        w: &mut W,
        folder_remote_id: &str,
        page_size: usize,
        link: Option<&str>,
        priority: Priority,
    ) -> Result<(usize, Option<String>)> {
        let acc = self.account;
        let folder = self.local_folder(w, folder_remote_id)?;
        let page = self
            .api
            .headers_page(folder_remote_id, page_size, link, priority)
            .await?;
        let stash = &mut self.stash;
        w.write(|tx| {
            for m in &page.items {
                upsert_header(tx, acc, folder.id, m, stash)?;
            }
            Ok(())
        })?;
        Ok((page.items.len(), page.next_link))
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
        let body = self.api.body(&remote, priority).await?;
        store_body(w, message, &body)
    }

    /// Downloads several bodies at once, as many in flight as the scheduler
    /// allows, and stores each as it arrives. Returns one result per message,
    /// in completion order.
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
            set.spawn(async move { (id, api.body(&m.remote_id, priority).await) });
        }
        let mut out = Vec::with_capacity(messages.len());
        while let Some(done) = set.join_next().await {
            let (id, body) = done.map_err(|e| Error::Decode(format!("body task failed: {e}")))?;
            let r = match body {
                Ok(b) => store_body(w, id, &b),
                Err(e) => Err(e),
            };
            out.push((id, r));
        }
        Ok(out)
    }
}

fn store_body<W: StoreWriter>(
    w: &mut W,
    message: MessageId,
    body: &crate::model::GraphBody,
) -> Result<BodyRef> {
    let is_html = body.is_html();
    Ok(w.write(|tx| {
        let hash = tx.put_body(body.content.as_bytes())?;
        let r = BodyRef { hash, is_html };
        tx.set_body(message, Some(r))?;
        Ok(r)
    })?)
}

/// Upserts a header into `folder`. A message the store hasn't seen gets a
/// stashed body with the same Internet message ID, if one is waiting.
fn upsert_header(
    tx: &WriteTx<'_>,
    acc: AccountId,
    folder: FolderId,
    m: &GraphMessage,
    stash: &mut HashMap<String, Stashed>,
) -> familiar_store::Result<MessageId> {
    // A message with local changes still on their way to the server keeps
    // them; the round after they land brings the server's view.
    if let Some(local) = tx.message_by_remote(acc, &m.id)? {
        if tx
            .open_op_targets(acc)?
            .contains(&familiar_store::message_target(local.id))
        {
            return Ok(local.id);
        }
    }
    let mut row = m.to_new_message(acc, folder);
    let mut pin = false;
    if let Some(imid) = row.internet_message_id.as_deref() {
        if !stash.is_empty() && tx.message_by_remote(acc, &row.remote_id)?.is_none() {
            if let Some(s) = stash.remove(imid) {
                row.body = Some(s.body);
                pin = s.pinned;
            }
        }
    }
    let id = tx.upsert_message(&row)?;
    if pin {
        tx.set_body_pinned(id, true)?;
    }
    Ok(id)
}

enum Carry {
    /// Another local copy got the body.
    Given,
    /// No copy wanted it; keep it for a later arrival.
    Keep(String, Stashed),
    /// Nothing to carry.
    Nothing,
}

/// Before `gone` is deleted, gives its body to another local message of the
/// account with the same Internet message ID that has no body yet.
fn carry_body(tx: &WriteTx<'_>, acc: AccountId, gone: MessageId) -> familiar_store::Result<Carry> {
    let Some(m) = tx.message(gone)? else {
        return Ok(Carry::Nothing);
    };
    let (Some(imid), Some(body)) = (m.internet_message_id.clone(), m.body) else {
        return Ok(Carry::Nothing);
    };
    for other in tx.messages_by_internet_id(&imid)? {
        if other.account == acc && other.id != gone && other.body.is_none() {
            tx.set_body(other.id, Some(body))?;
            if m.body_pinned {
                tx.set_body_pinned(other.id, true)?;
            }
            return Ok(Carry::Given);
        }
    }
    Ok(Carry::Keep(
        imid,
        Stashed {
            body,
            pinned: m.body_pinned,
        },
    ))
}

/// Upserts folders parents first, keeping well-known names already known.
fn apply_folders(
    tx: &WriteTx<'_>,
    acc: AccountId,
    folders: &[GraphFolder],
    well_known: &HashMap<String, String>,
) -> familiar_store::Result<usize> {
    let mut pending: Vec<&GraphFolder> = folders.iter().collect();
    let mut done = 0;
    loop {
        let waiting: HashSet<&str> = pending.iter().map(|f| f.id.as_str()).collect();
        let (ready, rest): (Vec<&GraphFolder>, Vec<&GraphFolder>) =
            pending.into_iter().partition(|f| {
                f.parent_folder_id
                    .as_deref()
                    .is_none_or(|p| p == f.id || !waiting.contains(p))
            });
        // A cycle can't happen in a real mailbox; if one did, place the rest anyway.
        let stuck = ready.is_empty();
        let batch = if stuck { rest.clone() } else { ready };
        for f in batch {
            let parent = match f.parent_folder_id.as_deref() {
                Some(p) if p != f.id => tx.folder_by_remote(acc, p)?.map(|x| x.id),
                _ => None,
            };
            let existing = tx.folder_by_remote(acc, &f.id)?;
            let wk = well_known
                .get(&f.id)
                .cloned()
                .or_else(|| existing.and_then(|e| e.well_known));
            tx.upsert_folder(&NewFolder {
                account: acc,
                remote_id: f.id.clone(),
                parent,
                name: f.display_name.clone(),
                well_known: wk,
            })?;
            done += 1;
        }
        if stuck || rest.is_empty() {
            return Ok(done);
        }
        pending = rest;
    }
}
