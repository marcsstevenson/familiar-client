// SPDX-License-Identifier: MIT OR Apache-2.0
//! Noticing new mail, for `[[hooks]]` in famd and for `fam watch`.
//!
//! [`NewMailWatch`] reads the store (read-only, so famd and `fam watch` can
//! each have one) and hands back the messages that arrived since it last
//! looked. Local message IDs only grow and a moved message keeps its row
//! (Graph immutable IDs), so "new" is simply "a row above the highest ID
//! seen". The first look takes the highest ID as its starting point, so mail
//! already there never counts.
//!
//! A message counts once it is ready to be matched: the search index has
//! caught up with it and its body is local, so `match = "body:..."` sees the
//! text. A message still waiting after [`BODY_WAIT`] counts anyway, matched
//! on what the index has.
//!
//! Left out: mail in Sent Items, Drafts, Outbox, Deleted Items and Junk,
//! mail received more than [`MAX_AGE`] ago (an old message the server only
//! now reported), and anything synced before an account's first sync
//! finished (that is backfill, not new mail).

use std::collections::{BTreeMap, HashMap, HashSet};

use familiar_search::{Query, Searcher};
use familiar_store::{AccountId, FolderId, Message, MessageId, Reader};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::daemon::DaemonPaths;

/// How long a new message waits for its body before it counts anyway.
pub const BODY_WAIT: i64 = 300;

/// Mail received longer ago than this is never "new".
pub const MAX_AGE: i64 = 86_400;

/// Folders whose new messages aren't new mail.
const NOT_NEW_MAIL: &[&str] = &["sentitems", "drafts", "outbox", "deleteditems", "junkemail"];

/// Rows read per page.
const PAGE: usize = 500;

/// A sender or recipient in an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventAddress {
    /// Display name, possibly empty.
    pub name: String,
    /// Email address.
    pub addr: String,
}

/// One new message, as `fam watch` prints it and hooks get it on stdin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MailEvent {
    /// Always `new_mail`.
    pub event: String,
    /// Local message ID, as the other `fam mail` commands take it.
    pub id: i64,
    /// Account name from config.toml.
    pub account: String,
    /// Folder path within the account, such as `Inbox` or `Inbox/Club`.
    pub folder: String,
    /// Sender.
    pub from: EventAddress,
    /// To recipients.
    pub to: Vec<EventAddress>,
    /// Subject line.
    pub subject: String,
    /// Received time (Unix seconds).
    pub received_at: i64,
    /// Short body preview.
    pub preview: String,
    /// Has file attachments.
    pub has_attachments: bool,
}

/// Watches one data directory for new mail.
pub struct NewMailWatch {
    paths: DaemonPaths,
    reader: Option<Reader>,
    searcher: Option<Searcher>,
    /// The highest message ID looked at; `None` until the first look.
    mark: Option<MessageId>,
    /// New messages not ready yet, with when they were first seen.
    pending: BTreeMap<MessageId, i64>,
}

impl NewMailWatch {
    /// A watch on the store at `paths`. Nothing is read until [`poll`](Self::poll).
    pub fn new(paths: &DaemonPaths) -> Self {
        Self {
            paths: paths.clone(),
            reader: None,
            searcher: None,
            mark: None,
            pending: BTreeMap::new(),
        }
    }

    /// The new mail that is ready, oldest first. The first call only sets
    /// the starting point and returns nothing. Errors are worth a log line;
    /// the next call tries again.
    pub fn poll(&mut self, now: i64) -> Result<Vec<MailEvent>, String> {
        if self.reader.is_none() {
            match Reader::open(&self.paths.store) {
                Ok(r) => self.reader = Some(r),
                // Nothing synced yet: everything that comes is first sync.
                Err(familiar_store::Error::NoStore(_)) => return Ok(Vec::new()),
                Err(e) => return Err(format!("store: {e}")),
            }
        }
        if self.searcher.is_none() {
            // Not built yet is normal for a minute after famd first starts;
            // until then matching falls back to "not indexed".
            self.searcher = Searcher::open(&self.paths.index).ok();
        }
        let r = self.reader.as_ref().expect("opened above");
        let Some(mark) = self.mark else {
            self.mark = Some(r.max_message_id().map_err(|e| format!("store: {e}"))?);
            return Ok(Vec::new());
        };
        let err = |e: familiar_store::Error| format!("store: {e}");

        // New rows since the mark join the pending list.
        let mut seen = mark;
        loop {
            let page = r.messages_after(seen, PAGE).map_err(err)?;
            let full = page.len() == PAGE;
            for m in &page {
                seen = m.id;
                self.pending.insert(m.id, now);
            }
            if !full {
                break;
            }
        }
        self.mark = Some(seen);
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<MessageId> = self.pending.keys().copied().collect();
        let queued = r.queued_for_index(&ids).map_err(err)?;
        let mut rows: HashMap<MessageId, Message> = r
            .messages_by_ids(&ids)
            .map_err(err)?
            .into_iter()
            .map(|m| (m.id, m))
            .collect();
        let mut ready = Vec::new();
        let mut names = Names::default();
        for (id, first_seen) in std::mem::take(&mut self.pending) {
            // Deleted again before it was ready.
            let Some(m) = rows.remove(&id) else { continue };
            if m.received_at < now - MAX_AGE || !names.is_new_mail(r, &m).map_err(err)? {
                continue;
            }
            let waited = now - first_seen >= BODY_WAIT;
            if !waited && (queued.contains(&id) || m.body.is_none()) {
                self.pending.insert(id, first_seen);
                continue;
            }
            ready.push(names.event(r, &m).map_err(err)?);
        }
        if !ready.is_empty()
            && let Some(s) = &self.searcher
            && let Err(e) = s.reload()
        {
            return Err(format!("search index: {e}"));
        }
        Ok(ready)
    }

    /// Which of `ids` the search `query` matches. Fails if the query doesn't
    /// parse or the search index isn't there.
    pub fn matching(&self, query: &str, ids: &[i64], now: i64) -> Result<HashSet<i64>, String> {
        let q = Query::parse(query, now).map_err(|e| e.to_string())?;
        let (Some(r), Some(s)) = (&self.reader, &self.searcher) else {
            return Err("the search index isn't built yet".into());
        };
        let ids: Vec<MessageId> = ids.iter().map(|i| MessageId(*i)).collect();
        Ok(s.matching(r, &q, &ids)
            .map_err(|e| format!("search: {e}"))?
            .into_iter()
            .map(|i| i.0)
            .collect())
    }
}

/// Account keys, folder paths and first-sync state, read once per poll.
#[derive(Default)]
struct Names {
    accounts: HashMap<AccountId, (String, bool)>,
    folders: HashMap<FolderId, (String, Option<String>)>,
}

impl Names {
    fn load_account(&mut self, r: &Reader, id: AccountId) -> familiar_store::Result<()> {
        if self.accounts.contains_key(&id) {
            return Ok(());
        }
        for a in r.accounts()? {
            let synced = r
                .sync_state(a.id, crate::daemon::FIRST_SYNC_RESOURCE)?
                .is_some();
            self.accounts.insert(a.id, (a.key.clone(), synced));
            let folders = r.folders(a.id)?;
            let paths = crate::commands::folder::folder_paths(&folders);
            for f in folders {
                let path = paths.get(&f.id).cloned().unwrap_or_else(|| f.name.clone());
                self.folders.insert(f.id, (path, f.well_known));
            }
        }
        Ok(())
    }

    fn is_new_mail(&mut self, r: &Reader, m: &Message) -> familiar_store::Result<bool> {
        self.load_account(r, m.account)?;
        let synced = self.accounts.get(&m.account).is_some_and(|a| a.1);
        let excluded = self
            .folders
            .get(&m.folder)
            .and_then(|f| f.1.as_deref())
            .is_some_and(|w| NOT_NEW_MAIL.contains(&w));
        Ok(synced && !excluded)
    }

    fn event(&mut self, r: &Reader, m: &Message) -> familiar_store::Result<MailEvent> {
        self.load_account(r, m.account)?;
        let addr = |a: &familiar_store::Address| EventAddress {
            name: a.name.clone(),
            addr: a.addr.clone(),
        };
        Ok(MailEvent {
            event: "new_mail".into(),
            id: m.id.0,
            account: self
                .accounts
                .get(&m.account)
                .map(|a| a.0.clone())
                .unwrap_or_default(),
            folder: self
                .folders
                .get(&m.folder)
                .map(|f| f.0.clone())
                .unwrap_or_default(),
            from: addr(&m.from),
            to: m.to.iter().map(addr).collect(),
            subject: m.subject.clone(),
            received_at: m.received_at,
            preview: m.preview.clone(),
            has_attachments: m.has_attachments,
        })
    }
}
