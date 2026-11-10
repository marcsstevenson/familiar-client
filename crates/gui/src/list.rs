// SPDX-License-Identifier: MIT OR Apache-2.0
//! The message list's data. The list is virtualised: Rust holds the sorted
//! message IDs of the open view, and the front end asks for the rows on
//! screen by position. Sorting happens here, never in the web view.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use familiar_core::daemon::DaemonPaths;
use familiar_core::{CommandError, ErrorKind};
use familiar_store::{FolderId, Message, MessageId, Reader, SortText};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Most rows one `list_rows` call returns.
const MAX_ROWS: usize = 500;

/// Most search results a view holds (the newest).
const MAX_SEARCH: usize = 10_000;

/// How the list is sorted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    /// Newest first.
    #[default]
    Newest,
    /// Oldest first.
    Oldest,
    /// By sender name, A to Z.
    From,
    /// By subject, A to Z.
    Subject,
}

/// What the list shows.
#[derive(Debug, Clone, Deserialize)]
pub struct ViewSpec {
    /// Folders by local ID. Empty means every account's Inbox (the unified Inbox).
    #[serde(default)]
    pub folders: Vec<i64>,
    /// Only unread messages.
    #[serde(default)]
    pub unread_only: bool,
    /// Sort order. Search results are always newest first.
    #[serde(default)]
    pub sort: Sort,
    /// Increases with each open. An open older than the current view is
    /// ignored, since overlapping opens can take the lock out of order.
    #[serde(default)]
    pub generation: u64,
    /// A search, in `fam mail search` syntax, instead of folders.
    #[serde(default)]
    pub query: Option<String>,
    /// The Follow up view instead of folders: messages with a follow-up
    /// reminder (`fam followup list`), soonest due first.
    #[serde(default)]
    pub followups: bool,
}

/// The result of opening a view.
#[derive(Debug, Serialize)]
pub struct Opened {
    /// Messages in the view.
    pub total: usize,
    /// For a search: things to know, such as a folder name that matched nothing.
    pub notices: Vec<String>,
    /// For a search: true when there were more than the view holds.
    pub capped: bool,
}

/// One row, as the list draws it.
#[derive(Debug, Serialize)]
pub struct Row {
    id: i64,
    account: String,
    from: String,
    subject: String,
    preview: String,
    /// Unix seconds.
    received_at: i64,
    is_read: bool,
    is_flagged: bool,
    has_attachments: bool,
    /// 0 low, 1 normal, 2 high.
    importance: i64,
    /// In its account's Junk Email folder (the menu offers Not junk).
    in_junk: bool,
    /// The spam check's headline and score ("Likely scam · 92%") when a
    /// saved grade is 50% or more and the message isn't in Junk Email.
    spam_warning: Option<String>,
    /// When its follow-up reminder is due (Unix seconds), if it has one.
    follow_up_due: Option<i64>,
}

/// The open view. One window, one list.
#[derive(Default)]
pub struct Lists {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    reader: Option<Reader>,
    searcher: Option<familiar_search::Searcher>,
    ids: Vec<MessageId>,
    accounts: HashMap<i64, String>,
    /// Every account's Junk Email folder.
    junk: HashSet<FolderId>,
    generation: u64,
}

/// Account keys by ID, and every account's Junk Email folder.
fn accounts_and_junk(
    r: &Reader,
) -> familiar_store::Result<(HashMap<i64, String>, HashSet<FolderId>)> {
    let mut accounts = HashMap::new();
    let mut junk = HashSet::new();
    for a in r.accounts()? {
        junk.extend(
            r.folders(a.id)?
                .into_iter()
                .filter(|f| f.well_known.as_deref() == Some("junkemail"))
                .map(|f| f.id),
        );
        accounts.insert(a.id.0, a.key);
    }
    Ok((accounts, junk))
}

fn store_error(e: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Internal, format!("store: {e}"))
}

fn open_reader() -> Result<Reader, CommandError> {
    let paths = DaemonPaths::resolve()?;
    Reader::open(&paths.store).map_err(|e| match e {
        familiar_store::Error::NoStore(_) => CommandError::new(
            ErrorKind::NotFound,
            "no mail on this device yet: sign in with `fam auth login` and let famd sync",
        ),
        e => store_error(e),
    })
}

impl Inner {
    fn reader(&mut self) -> Result<&mut Reader, CommandError> {
        if self.reader.is_none() {
            self.reader = Some(open_reader()?);
        }
        Ok(self.reader.as_mut().expect("just opened"))
    }
}

impl Inner {
    /// Opens a search view: the newest [`MAX_SEARCH`] matches.
    fn search(&mut self, text: &str, unread_only: bool) -> Result<Opened, CommandError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let mut q = familiar_search::Query::parse(text, now)
            .map_err(|e| CommandError::usage(format!("search: {e}")))?;
        if unread_only {
            q.unread = Some(true);
        }
        if self.searcher.is_none() {
            let index = DaemonPaths::resolve()?.index;
            self.searcher = Some(familiar_search::Searcher::open(&index).map_err(|e| {
                CommandError::new(
                    ErrorKind::NotFound,
                    format!("search isn't ready yet ({e}); famd builds the index as it syncs"),
                )
            })?);
        }
        let searcher = self.searcher.as_ref().expect("just opened");
        // See famd's latest index commit.
        searcher.reload().map_err(store_error)?;
        let reader = self.reader.get_or_insert(open_reader()?);
        let (mut ids, notices) = searcher
            .all_ids(reader, &q, MAX_SEARCH + 1)
            .map_err(store_error)?;
        let capped = ids.len() > MAX_SEARCH;
        ids.truncate(MAX_SEARCH);
        let (accounts, junk) = reader.snapshot(accounts_and_junk).map_err(store_error)?;
        self.accounts = accounts;
        self.junk = junk;
        self.ids = ids;
        Ok(Opened {
            total: self.ids.len(),
            notices,
            capped,
        })
    }
}

impl Inner {
    /// Opens the Follow up view: messages with an open reminder, soonest due
    /// first. Reminders whose message hasn't synced yet aren't shown.
    fn followups(&mut self) -> Result<Opened, CommandError> {
        let paths = DaemonPaths::resolve()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let checked = familiar_core::commands::followup::check_all_at(&paths, now)?;
        let reader = self.reader()?;
        let (accounts, junk) = reader.snapshot(accounts_and_junk).map_err(store_error)?;
        self.accounts = accounts;
        self.junk = junk;
        self.ids = checked
            .iter()
            .filter_map(|c| c.message.as_ref().map(|m| m.id))
            .collect();
        Ok(Opened {
            total: self.ids.len(),
            notices: Vec::new(),
            capped: false,
        })
    }
}

impl Lists {
    /// Opens a view: reads and sorts its message IDs.
    pub fn open(&self, spec: &ViewSpec) -> Result<Opened, CommandError> {
        let mut g = self
            .inner
            .lock()
            .map_err(|_| store_error("lock poisoned"))?;
        if spec.generation < g.generation {
            return Ok(Opened {
                total: g.ids.len(),
                notices: Vec::new(),
                capped: false,
            });
        }
        g.generation = spec.generation;
        if let Some(q) = spec.query.as_deref() {
            return g.search(q, spec.unread_only);
        }
        if spec.followups {
            return g.followups();
        }
        let (ids, accounts, junk) = g
            .reader()?
            .snapshot(|r| {
                let mut inboxes = Vec::new();
                if spec.folders.is_empty() {
                    for a in r.accounts()? {
                        inboxes.extend(
                            r.folders(a.id)?
                                .into_iter()
                                .filter(|f| f.well_known.as_deref() == Some("inbox"))
                                .map(|f| f.id),
                        );
                    }
                }
                let (accounts, junk) = accounts_and_junk(r)?;
                let folders: Vec<FolderId> = if spec.folders.is_empty() {
                    inboxes
                } else {
                    spec.folders.iter().map(|f| FolderId(*f)).collect()
                };
                Ok((sorted_ids(r, &folders, spec)?, accounts, junk))
            })
            .map_err(store_error)?;
        g.ids = ids;
        g.accounts = accounts;
        g.junk = junk;
        Ok(Opened {
            total: g.ids.len(),
            notices: Vec::new(),
            capped: false,
        })
    }

    /// Rows `start..start + count` of the open view.
    pub fn rows(&self, start: usize, count: usize) -> Result<Vec<Row>, CommandError> {
        let mut g = self
            .inner
            .lock()
            .map_err(|_| store_error("lock poisoned"))?;
        let end = start.saturating_add(count.min(MAX_ROWS)).min(g.ids.len());
        if start >= end {
            return Ok(Vec::new());
        }
        let want = g.ids[start..end].to_vec();
        let found = g.reader()?.messages_by_ids(&want).map_err(store_error)?;
        let mut by_id: HashMap<MessageId, Message> = found.into_iter().map(|m| (m.id, m)).collect();
        // Saved spam checks: a small file on this device, read per page so a
        // grade famd saves in the background shows on the next scroll.
        let spam = DaemonPaths::resolve()
            .map(|p| familiar_core::commands::check_spam::Saved::load(&p.data_dir))
            .unwrap_or_default();
        let followups = DaemonPaths::resolve()
            .map(|p| familiar_core::commands::followup::FollowUps::load(&p.data_dir))
            .unwrap_or_default();
        // A message deleted since the view opened is skipped; the store-changed
        // event reopens the view straight after.
        Ok(want
            .iter()
            .filter_map(|id| by_id.remove(id))
            .map(|m| {
                let account = g.accounts.get(&m.account.0).cloned().unwrap_or_default();
                let in_junk = g.junk.contains(&m.folder);
                let spam_warning = if in_junk {
                    None
                } else {
                    spam.0
                        .get(&account)
                        .and_then(|a| a.get(&m.remote_id))
                        .and_then(|o| o.ai.as_ref())
                        .filter(|a| a.spam.max(a.scam) >= 50)
                        .map(|a| format!("{} · {}%", a.headline, a.spam.max(a.scam)))
                };
                let follow_up_due = m
                    .internet_message_id
                    .as_deref()
                    .and_then(|imid| followups.find(&account, imid))
                    .map(|f| f.due_at);
                Row {
                    id: m.id.0,
                    account,
                    from: if m.from.name.is_empty() {
                        m.from.addr
                    } else {
                        m.from.name
                    },
                    subject: m.subject,
                    preview: m.preview,
                    received_at: m.received_at,
                    is_read: m.is_read,
                    is_flagged: m.is_flagged,
                    has_attachments: m.has_attachments,
                    importance: m.importance,
                    in_junk,
                    spam_warning,
                    follow_up_due,
                }
            })
            .collect())
    }

    /// The position of message `id` in the open view.
    pub fn index_of(&self, id: i64) -> Option<usize> {
        let g = self.inner.lock().ok()?;
        g.ids.iter().position(|m| m.0 == id)
    }
}

/// The view's IDs in display order.
fn sorted_ids(
    r: &Reader,
    folders: &[FolderId],
    spec: &ViewSpec,
) -> familiar_store::Result<Vec<MessageId>> {
    let text = |col| -> familiar_store::Result<Vec<MessageId>> {
        let mut v: Vec<(String, MessageId)> = r
            .message_sort_text(folders, spec.unread_only, col)?
            .into_iter()
            .map(|(id, t)| (t.to_lowercase(), id))
            .collect();
        // Same text: newest first (higher ID is newer within a sync).
        v.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        Ok(v.into_iter().map(|(_, id)| id).collect())
    };
    Ok(match spec.sort {
        Sort::Newest => r
            .message_order(folders, spec.unread_only)?
            .into_iter()
            .map(|(id, _)| id)
            .collect(),
        Sort::Oldest => r
            .message_order(folders, spec.unread_only)?
            .into_iter()
            .rev()
            .map(|(id, _)| id)
            .collect(),
        Sort::From => text(SortText::From)?,
        Sort::Subject => text(SortText::Subject)?,
    })
}

/// Tells the front end when the store has changed (famd synced, or a command
/// wrote), at most once a second, as the `store_changed` event.
pub fn watch_store(app: &AppHandle) {
    let mut reader = None;
    let mut last = None;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if reader.is_none() {
            reader = open_reader().ok();
        }
        let Some(r) = reader.as_ref() else { continue };
        match r.data_version() {
            Ok(v) => {
                // Also on the first read: the store may have appeared, or been
                // written, since the panes first asked for it.
                if last != Some(v) {
                    let _ = app.emit("store_changed", ());
                }
                last = Some(v);
            }
            Err(_) => reader = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use familiar_store::{Address, NewAccount, NewFolder, NewMessage, Writer};

    #[test]
    fn views_sort_by_date_sender_and_subject() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = Writer::open(dir.path()).unwrap();
        let (f, ids) = w
            .write(|tx| {
                let a = tx.upsert_account(&NewAccount {
                    key: "home".into(),
                    email: "home@example.com".into(),
                    ..Default::default()
                })?;
                let f = tx.upsert_folder(&NewFolder {
                    account: a,
                    remote_id: "inbox".into(),
                    name: "Inbox".into(),
                    well_known: Some("inbox".into()),
                    ..Default::default()
                })?;
                let mut ids = Vec::new();
                for (i, (from, subject)) in [("bea", "Zebra"), ("Al", "apple"), ("cy", "Mango")]
                    .into_iter()
                    .enumerate()
                {
                    ids.push(tx.upsert_message(&NewMessage {
                        account: a,
                        folder: f,
                        remote_id: format!("m{i}"),
                        conversation_id: format!("c{i}"),
                        subject: subject.into(),
                        from: Address {
                            name: from.into(),
                            addr: format!("{from}@example.com"),
                        },
                        received_at: 100 + i as i64,
                        ..Default::default()
                    })?);
                }
                Ok((f, ids))
            })
            .unwrap();
        let r = Reader::open(dir.path()).unwrap();
        let order = |sort| {
            let spec = ViewSpec {
                folders: vec![f.0],
                unread_only: false,
                sort,
                generation: 0,
                query: None,
                followups: false,
            };
            sorted_ids(&r, &[f], &spec).unwrap()
        };
        let [bea, al, cy] = [ids[0], ids[1], ids[2]];
        assert_eq!(order(Sort::Newest), [cy, al, bea]);
        assert_eq!(order(Sort::Oldest), [bea, al, cy]);
        assert_eq!(order(Sort::From), [al, bea, cy]);
        assert_eq!(order(Sort::Subject), [al, cy, bea]);
    }
}
