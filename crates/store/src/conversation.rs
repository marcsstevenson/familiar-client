// SPDX-License-Identifier: MIT OR Apache-2.0
//! Conversations assembled locally from Graph's `conversationId` across every
//! folder of an account (WP 1.8).
//!
//! Graph's own conversation view needs one query per folder, which is why a
//! complete thread used to mean separate Inbox and Sent Items searches. Here
//! every message already points at its `threads` row, so a conversation is
//! one index range scan over `messages_thread`, whatever folders the messages
//! are in. Moves keep the thread (the row's `thread_id` doesn't change), and
//! the `threads` and `folder_threads` rows are kept by triggers.
//!
//! Assembly adds what a reading view needs on top of the raw rows:
//! - each message's folder, whether it is in Sent Items, Drafts or the trash,
//!   and whether it is from the account's own address;
//! - copies of one message (same Internet Message-ID, for example mail sent
//!   to yourself, which is in both Inbox and Sent Items) shown once;
//! - the subject without reply and forward prefixes, and the participants.

use crate::model::*;
use crate::query;
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::HashMap;

/// Well-known folder names (Graph) that hold sent mail.
pub const SENT_ITEMS: &str = "sentitems";
/// Well-known folder name for drafts.
pub const DRAFTS: &str = "drafts";
/// Well-known folder names whose messages count as thrown away.
pub const TRASH_FOLDERS: [&str; 2] = ["deleteditems", "junkemail"];

/// A whole conversation, oldest message first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Conversation {
    /// Local thread ID.
    pub thread: ThreadId,
    /// Owning account.
    pub account: AccountId,
    /// Graph's conversation ID.
    pub conversation_id: String,
    /// Subject of the earliest message with one, without `RE:`/`FW:` prefixes.
    pub subject: String,
    /// Everyone on the conversation (senders, To and Cc), in order of first
    /// appearance, one entry per address.
    pub participants: Vec<Address>,
    /// Messages after copies are merged.
    pub message_count: usize,
    /// Messages with at least one unread copy.
    pub unread_count: usize,
    /// Newest message time, copies included.
    pub last_received_at: i64,
    /// How many of the conversation's messages each folder holds, in folder
    /// order (copies counted in their own folder).
    pub folders: Vec<ConversationFolder>,
    /// The messages, oldest first.
    pub messages: Vec<ConversationMessage>,
}

impl Conversation {
    /// The messages that aren't in Deleted Items or Junk Email.
    pub fn without_trash(&self) -> impl Iterator<Item = &ConversationMessage> {
        self.messages.iter().filter(|m| !m.in_trash)
    }

    /// Whether any message is in Sent Items.
    pub fn has_sent(&self) -> bool {
        self.messages.iter().any(|m| {
            m.is_sent()
                || m.copies
                    .iter()
                    .any(|c| c.well_known.as_deref() == Some(SENT_ITEMS))
        })
    }
}

/// A folder's share of a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversationFolder {
    /// The folder.
    pub folder: FolderId,
    /// Display name.
    pub name: String,
    /// Graph well-known name, if any.
    pub well_known: Option<String>,
    /// Messages (including copies) of the conversation in this folder.
    pub count: usize,
}

/// One message of a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversationMessage {
    /// The stored message. When there are copies this is the one shown (see
    /// [`ConversationMessage::copies`]); `is_read` is false if any copy is unread.
    #[serde(flatten)]
    pub message: Message,
    /// Name of the folder it is in.
    pub folder_name: String,
    /// Well-known name of that folder, if any.
    pub folder_well_known: Option<String>,
    /// Sent from the account's own address.
    pub from_me: bool,
    /// In Deleted Items or Junk Email.
    pub in_trash: bool,
    /// Other stored copies of the same message (same Internet Message-ID).
    pub copies: Vec<MessageCopy>,
}

impl ConversationMessage {
    /// Whether the shown copy is in Sent Items.
    pub fn is_sent(&self) -> bool {
        self.folder_well_known.as_deref() == Some(SENT_ITEMS)
    }

    /// Whether the shown copy is in Drafts.
    pub fn is_draft(&self) -> bool {
        self.folder_well_known.as_deref() == Some(DRAFTS)
    }
}

/// Another copy of a message, in the same or another folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessageCopy {
    /// Local message ID.
    pub id: MessageId,
    /// Folder it is in.
    pub folder: FolderId,
    /// That folder's well-known name, if any.
    pub well_known: Option<String>,
}

/// A row of a threaded folder list: the conversation as seen from one folder,
/// plus its totals across every folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversationSummary {
    /// The conversation.
    pub thread: ThreadId,
    /// The folder the list is for.
    pub folder: FolderId,
    /// Newest message time in this folder (the list's sort key).
    pub last_received_at: i64,
    /// Newest message in this folder.
    pub last_message: MessageId,
    /// Messages of the conversation in this folder.
    pub folder_message_count: i64,
    /// Unread messages of the conversation in this folder.
    pub folder_unread_count: i64,
    /// Messages in every folder (copies counted separately).
    pub message_count: i64,
    /// Unread messages in every folder.
    pub unread_count: i64,
    /// Subject of this folder's newest message, without reply prefixes.
    pub subject: String,
    /// Newest message in any folder, which may be a reply in Sent Items.
    pub latest: LatestMessage,
    /// Whether the conversation has a message in Sent Items.
    pub has_sent: bool,
    /// Whether any message of the conversation has attachments.
    pub has_attachments: bool,
}

impl ConversationSummary {
    /// The paging cursor for the next page after this row.
    pub fn cursor(&self) -> Cursor {
        Cursor {
            received_at: self.last_received_at,
            id: self.thread.0,
        }
    }
}

/// The newest message of a conversation, for a list row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LatestMessage {
    /// Local ID.
    pub id: MessageId,
    /// Folder it is in.
    pub folder: FolderId,
    /// Sender.
    pub from: Address,
    /// Received time.
    pub received_at: i64,
    /// Preview text.
    pub preview: String,
}

/// `subject` without leading reply and forward prefixes (`RE:`, `FW:`,
/// `Fwd:`, `AW:`, `WG:`, `SV:`, `VS:`, `Antw:`, also `RE[2]:`), repeated.
pub fn normalize_subject(subject: &str) -> &str {
    const PREFIXES: [&str; 9] = ["re", "fw", "fwd", "aw", "wg", "sv", "vs", "antw", "tr"];
    let mut s = subject.trim();
    loop {
        let Some(colon) = s.find(':') else { return s };
        let head = s[..colon].trim_end();
        // Allow a counter such as "RE[2]" or "RE(2)".
        let word = head
            .split_once(['[', '('])
            .filter(|(_, rest)| {
                rest.strip_suffix([']', ')'])
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
            .map_or(head, |(w, _)| w);
        if PREFIXES.iter().any(|p| word.eq_ignore_ascii_case(p)) {
            s = s[colon + 1..].trim_start();
        } else {
            return s;
        }
    }
}

struct FolderInfo {
    name: String,
    well_known: Option<String>,
}

impl FolderInfo {
    fn is_trash(&self) -> bool {
        self.well_known
            .as_deref()
            .is_some_and(|w| TRASH_FOLDERS.contains(&w))
    }
}

pub(crate) fn thread_of_message(c: &Connection, id: MessageId) -> Result<Option<ThreadId>> {
    let mut s = c.prepare_cached("SELECT thread_id FROM messages WHERE id = ?1")?;
    Ok(s.query_row([id], |r| r.get(0)).optional()?)
}

pub(crate) fn conversation(c: &Connection, thread: ThreadId) -> Result<Option<Conversation>> {
    let mut s =
        c.prepare_cached("SELECT account_id, conversation_id FROM threads WHERE id = ?1")?;
    let Some((account, conversation_id)): Option<(AccountId, String)> = s
        .query_row([thread], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
    else {
        return Ok(None);
    };
    let me = c
        .prepare_cached("SELECT email FROM accounts WHERE id = ?1")?
        .query_row([account], |r| r.get::<_, String>(0))
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("account {}", account.0)))?
        .to_lowercase();

    let folders: HashMap<FolderId, FolderInfo> = query::folders(c, account)?
        .into_iter()
        .map(|f| {
            (
                f.id,
                FolderInfo {
                    name: f.name,
                    well_known: f.well_known,
                },
            )
        })
        .collect();
    let info = |id: FolderId| folders.get(&id);

    let rows = query::thread_messages(c, thread)?;
    let last_received_at = rows.iter().map(|m| m.received_at).max().unwrap_or(0);

    // Per-folder counts, before merging copies.
    let mut per_folder: Vec<ConversationFolder> = Vec::new();
    for m in &rows {
        match per_folder.iter_mut().find(|f| f.folder == m.folder) {
            Some(f) => f.count += 1,
            None => {
                let fi = info(m.folder);
                per_folder.push(ConversationFolder {
                    folder: m.folder,
                    name: fi.map(|f| f.name.clone()).unwrap_or_default(),
                    well_known: fi.and_then(|f| f.well_known.clone()),
                    count: 1,
                });
            }
        }
    }
    per_folder.sort_by_key(|f| f.folder);

    // Group copies by Internet Message-ID; a message without one stands alone.
    let mut groups: Vec<Vec<Message>> = Vec::new();
    let mut by_imid: HashMap<String, usize> = HashMap::new();
    for m in rows {
        match m.internet_message_id.as_deref().filter(|s| !s.is_empty()) {
            Some(imid) => match by_imid.get(imid) {
                Some(&i) => groups[i].push(m),
                None => {
                    by_imid.insert(imid.to_string(), groups.len());
                    groups.push(vec![m]);
                }
            },
            None => groups.push(vec![m]),
        }
    }

    let mut messages = Vec::with_capacity(groups.len());
    for mut group in groups {
        // The copy to show: not thrown away, then has a body, then the oldest row.
        let pick = group
            .iter()
            .enumerate()
            .min_by_key(|(_, m)| {
                (
                    info(m.folder).is_some_and(FolderInfo::is_trash),
                    m.body.is_none(),
                    m.id,
                )
            })
            .map(|(i, _)| i)
            .unwrap_or(0);
        let mut message = group.swap_remove(pick);
        message.is_read = message.is_read && group.iter().all(|m| m.is_read);
        message.has_attachments |= group.iter().any(|m| m.has_attachments);
        message.is_flagged |= group.iter().any(|m| m.is_flagged);
        let mut copies: Vec<MessageCopy> = group
            .iter()
            .map(|m| MessageCopy {
                id: m.id,
                folder: m.folder,
                well_known: info(m.folder).and_then(|f| f.well_known.clone()),
            })
            .collect();
        copies.sort_by_key(|c| c.id);
        let fi = info(message.folder);
        messages.push(ConversationMessage {
            folder_name: fi.map(|f| f.name.clone()).unwrap_or_default(),
            folder_well_known: fi.and_then(|f| f.well_known.clone()),
            from_me: !me.is_empty() && message.from.addr.to_lowercase() == me,
            in_trash: fi.is_some_and(FolderInfo::is_trash),
            copies,
            message,
        });
    }
    // Groups were formed in (received_at, id) order of their first copy; the
    // shown copy may differ, so sort again on it.
    messages.sort_by_key(|m| (m.message.received_at, m.message.id));

    let subject = messages
        .iter()
        .map(|m| normalize_subject(&m.message.subject))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
        .to_string();

    let mut participants: Vec<Address> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for m in &messages {
        let m = &m.message;
        for a in std::iter::once(&m.from).chain(&m.to).chain(&m.cc) {
            let key = a.addr.trim().to_lowercase();
            if key.is_empty() {
                continue;
            }
            match seen.get(&key) {
                Some(&i) => {
                    if participants[i].name.is_empty() && !a.name.is_empty() {
                        participants[i].name = a.name.clone();
                    }
                }
                None => {
                    seen.insert(key, participants.len());
                    participants.push(Address {
                        name: a.name.clone(),
                        addr: a.addr.trim().to_string(),
                    });
                }
            }
        }
    }

    Ok(Some(Conversation {
        thread,
        account,
        conversation_id,
        subject,
        participants,
        message_count: messages.len(),
        unread_count: messages.iter().filter(|m| !m.message.is_read).count(),
        last_received_at,
        folders: per_folder,
        messages,
    }))
}

/// Conversations in `folder`, newest first, keyset paged on
/// (last_received_at in the folder, thread id) like [`query::folder_threads`].
pub(crate) fn folder_conversations(
    c: &Connection,
    folder: FolderId,
    after: Option<Cursor>,
    limit: usize,
) -> Result<Vec<ConversationSummary>> {
    let (t, id) = after.map_or((i64::MAX, i64::MAX), |k| (k.received_at, k.id));
    let mut s = c.prepare_cached(
        "SELECT ft.thread_id, ft.folder_id, ft.last_received_at, ft.last_message_id,
                ft.message_count, ft.unread_count, th.message_count, th.unread_count,
                fm.subject,
                lm.id, lm.folder_id, lm.from_name, lm.from_addr, lm.received_at, lm.preview,
                EXISTS (SELECT 1 FROM folders sf JOIN messages sm
                          ON sm.thread_id = ft.thread_id AND sm.folder_id = sf.id
                         WHERE sf.account_id = th.account_id AND sf.well_known = 'sentitems'),
                EXISTS (SELECT 1 FROM messages am
                         WHERE am.thread_id = ft.thread_id AND am.has_attachments = 1)
           FROM folder_threads ft
           JOIN threads th ON th.id = ft.thread_id
           JOIN messages fm ON fm.id = ft.last_message_id
           JOIN messages lm ON lm.id = th.last_message_id
          WHERE ft.folder_id = ?1 AND (ft.last_received_at, ft.thread_id) < (?2, ?3)
          ORDER BY ft.last_received_at DESC, ft.thread_id DESC LIMIT ?4",
    )?;
    let rows = s.query_map(params![folder, t, id, limit as i64], |r| {
        let subject: String = r.get(8)?;
        Ok(ConversationSummary {
            thread: r.get(0)?,
            folder: r.get(1)?,
            last_received_at: r.get(2)?,
            last_message: r.get(3)?,
            folder_message_count: r.get(4)?,
            folder_unread_count: r.get(5)?,
            message_count: r.get(6)?,
            unread_count: r.get(7)?,
            subject: normalize_subject(&subject).to_string(),
            latest: LatestMessage {
                id: r.get(9)?,
                folder: r.get(10)?,
                from: Address {
                    name: r.get(11)?,
                    addr: r.get(12)?,
                },
                received_at: r.get(13)?,
                preview: r.get(14)?,
            },
            has_sent: r.get(15)?,
            has_attachments: r.get(16)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::normalize_subject;

    #[test]
    fn subject_prefixes_are_removed() {
        for (input, want) in [
            ("Practice moved", "Practice moved"),
            ("RE: Practice moved", "Practice moved"),
            ("Re: RE: Fwd: Practice moved", "Practice moved"),
            ("FW:Practice moved", "Practice moved"),
            ("RE[2]: Practice moved", "Practice moved"),
            ("AW: WG: Termin", "Termin"),
            ("  re :  spaced  ", "spaced"),
            ("Agenda: Tuesday", "Agenda: Tuesday"),
            ("RE[x]: odd", "RE[x]: odd"),
            ("RE:", ""),
            ("", ""),
        ] {
            assert_eq!(normalize_subject(input), want, "{input:?}");
        }
    }
}
