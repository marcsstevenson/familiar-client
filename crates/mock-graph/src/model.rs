// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mailbox contents (the public fixture types) and the indexed state the server reads.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// An email address with a display name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailAddress {
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// SMTP address.
    pub address: String,
}

impl EmailAddress {
    /// Builds an address from a display name and an SMTP address.
    pub fn new(name: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            address: address.into(),
        }
    }
}

/// The signed-in user that owns a mailbox (`GET /me`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    /// Directory object ID.
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// User principal name (sign-in name).
    pub user_principal_name: String,
    /// Primary SMTP address.
    pub mail: String,
    /// Mailbox time zone reported by `mailboxSettings`.
    #[serde(default = "default_time_zone")]
    pub time_zone: String,
}

fn default_time_zone() -> String {
    "UTC".into()
}

impl Default for User {
    fn default() -> Self {
        Self {
            id: "00000000-0000-0000-0000-000000000001".into(),
            display_name: "Test User".into(),
            user_principal_name: "test.user@example.com".into(),
            mail: "test.user@example.com".into(),
            time_zone: default_time_zone(),
        }
    }
}

/// A mail folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    /// Folder ID. Left empty in a fixture, one is assigned.
    #[serde(default)]
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Parent folder ID or well-known name. `None` puts the folder at the top level.
    #[serde(default)]
    pub parent_folder_id: Option<String>,
    /// Well-known name such as `inbox` or `sentitems`.
    #[serde(default)]
    pub well_known_name: Option<String>,
    /// Hidden folders are left out of listings, as in Graph.
    #[serde(default)]
    pub is_hidden: bool,
}

impl Folder {
    /// A top-level folder with the given display name.
    pub fn new(display_name: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            display_name: display_name.into(),
            parent_folder_id: None,
            well_known_name: None,
            is_hidden: false,
        }
    }

    /// Sets the parent folder (an ID or a well-known name).
    pub fn under(mut self, parent: impl Into<String>) -> Self {
        self.parent_folder_id = Some(parent.into());
        self
    }
}

/// A mail message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Immutable ID. Left empty, one is assigned. Graph's default (REST) ID is derived from it
    /// and changes when the message moves.
    #[serde(default)]
    pub id: String,
    /// Folder ID or well-known name.
    pub folder_id: String,
    /// Subject line.
    #[serde(default)]
    pub subject: String,
    /// Sender.
    pub from: EmailAddress,
    /// To recipients.
    #[serde(default)]
    pub to: Vec<EmailAddress>,
    /// Cc recipients.
    #[serde(default)]
    pub cc: Vec<EmailAddress>,
    /// When the message arrived.
    pub received_date_time: DateTime<Utc>,
    /// When it was sent. Defaults to the received time.
    #[serde(default)]
    pub sent_date_time: Option<DateTime<Utc>>,
    /// Read state.
    #[serde(default)]
    pub is_read: bool,
    /// Draft state.
    #[serde(default)]
    pub is_draft: bool,
    /// Whether the message has attachments.
    #[serde(default)]
    pub has_attachments: bool,
    /// Follow-up flag.
    #[serde(default)]
    pub flagged: bool,
    /// `low`, `normal` or `high`.
    #[serde(default = "default_importance")]
    pub importance: String,
    /// Categories.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Conversation ID. Left empty, one is derived from the subject.
    #[serde(default)]
    pub conversation_id: String,
    /// Internet message ID. Left empty, one is derived from the ID.
    #[serde(default)]
    pub internet_message_id: String,
    /// HTML body. `None` gives a deterministic synthetic body.
    #[serde(default)]
    pub body_html: Option<String>,
    /// Bcc recipients (drafts and sent mail).
    #[serde(default)]
    pub bcc: Vec<EmailAddress>,
    /// File attachments.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

/// A file attachment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// Attachment ID.
    #[serde(default)]
    pub id: String,
    /// File name.
    pub name: String,
    /// MIME type.
    #[serde(default)]
    pub content_type: String,
    /// Shown in the body (referenced by `cid:`), not in the attachment list.
    #[serde(default)]
    pub is_inline: bool,
    /// Content ID for inline images.
    #[serde(default)]
    pub content_id: Option<String>,
    /// The bytes, base64.
    #[serde(default)]
    pub content_bytes: String,
}

fn default_importance() -> String {
    "normal".into()
}

impl Message {
    /// A message in `folder` (an ID or well-known name) with the minimum fields set.
    pub fn new(
        folder: impl Into<String>,
        subject: impl Into<String>,
        from: EmailAddress,
        received: DateTime<Utc>,
    ) -> Self {
        Self {
            id: String::new(),
            folder_id: folder.into(),
            subject: subject.into(),
            from,
            to: Vec::new(),
            cc: Vec::new(),
            received_date_time: received,
            sent_date_time: None,
            is_read: false,
            is_draft: false,
            has_attachments: false,
            flagged: false,
            importance: default_importance(),
            categories: Vec::new(),
            conversation_id: String::new(),
            internet_message_id: String::new(),
            body_html: None,
            bcc: Vec::new(),
            attachments: Vec::new(),
        }
    }
}

/// A calendar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Calendar {
    /// Calendar ID. Left empty, one is assigned.
    #[serde(default)]
    pub id: String,
    /// Display name.
    pub name: String,
    /// The default calendar backs `/me/calendar`, `/me/events` and `/me/calendarView`.
    #[serde(default)]
    pub is_default: bool,
    /// Colour name, for example `auto` or `lightBlue`.
    #[serde(default = "default_color")]
    pub color: String,
    /// Owner. Defaults to the mailbox user.
    #[serde(default)]
    pub owner: Option<EmailAddress>,
}

fn default_color() -> String {
    "auto".into()
}

/// A calendar event (single instance or an expanded occurrence). Times are UTC.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// Event ID. Left empty, one is assigned.
    #[serde(default)]
    pub id: String,
    /// Calendar ID. Empty means the default calendar.
    #[serde(default)]
    pub calendar_id: String,
    /// Subject.
    #[serde(default)]
    pub subject: String,
    /// Start time (UTC).
    pub start: DateTime<Utc>,
    /// End time (UTC).
    pub end: DateTime<Utc>,
    /// All-day event.
    #[serde(default)]
    pub is_all_day: bool,
    /// Location display name.
    #[serde(default)]
    pub location: String,
    /// Organizer. Defaults to the mailbox user.
    #[serde(default)]
    pub organizer: Option<EmailAddress>,
    /// Required attendees.
    #[serde(default)]
    pub attendees: Vec<EmailAddress>,
    /// HTML body. `None` gives an empty body.
    #[serde(default)]
    pub body_html: Option<String>,
    /// `free`, `tentative`, `busy`, `oof` or `workingElsewhere`.
    #[serde(default = "default_show_as")]
    pub show_as: String,
    /// Series master for an occurrence.
    #[serde(default)]
    pub series_master_id: Option<String>,
    /// `singleInstance`, `occurrence`, `exception` or `seriesMaster`.
    #[serde(default = "default_event_type", rename = "type")]
    pub event_type: String,
    /// Cancelled meeting.
    #[serde(default)]
    pub is_cancelled: bool,
    /// Reminder minutes before the start; `None` is no reminder.
    #[serde(default = "default_reminder")]
    pub reminder_minutes: Option<i64>,
    /// The mailbox user's response (`accepted`, `tentativelyAccepted`,
    /// `declined`, `notResponded`); `None` gives `organizer` or `none`.
    #[serde(default)]
    pub response: Option<String>,
    /// Outlook categories.
    #[serde(default)]
    pub categories: Vec<String>,
    /// A series master's recurrence, as Graph's `patternedRecurrence`.
    #[serde(default)]
    pub recurrence: Option<serde_json::Value>,
    /// The client's `transactionId` from the POST that made it.
    #[serde(default)]
    pub transaction_id: Option<String>,
    /// The zone it was created in (Graph's `originalStartTimeZone`).
    #[serde(default)]
    pub time_zone: Option<String>,
}

fn default_reminder() -> Option<i64> {
    Some(15)
}

fn default_show_as() -> String {
    "busy".into()
}

fn default_event_type() -> String {
    "singleInstance".into()
}

impl Event {
    /// A single-instance event on the default calendar.
    pub fn new(subject: impl Into<String>, start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self {
            id: String::new(),
            calendar_id: String::new(),
            subject: subject.into(),
            start,
            end,
            is_all_day: false,
            location: String::new(),
            organizer: None,
            attendees: Vec::new(),
            body_html: None,
            show_as: default_show_as(),
            series_master_id: None,
            event_type: default_event_type(),
            is_cancelled: false,
            reminder_minutes: default_reminder(),
            response: None,
            categories: Vec::new(),
            recurrence: None,
            transaction_id: None,
            time_zone: None,
        }
    }
}

/// A whole mailbox: user, folders, messages, calendars and events.
///
/// Load one from a JSON fixture with [`Mailbox::from_json`], use the bundled
/// [`Mailbox::small`], or generate one with [`Mailbox::synthetic`](crate::SyntheticSpec).
/// Missing well-known folders and a default calendar are added when the server starts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    /// Mailbox owner.
    #[serde(default)]
    pub user: User,
    /// Folders, parents before children.
    #[serde(default)]
    pub folders: Vec<Folder>,
    /// Messages.
    #[serde(default)]
    pub messages: Vec<Message>,
    /// Calendars.
    #[serde(default)]
    pub calendars: Vec<Calendar>,
    /// Events.
    #[serde(default)]
    pub events: Vec<Event>,
}

impl Mailbox {
    /// Parses a mailbox fixture.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The bundled hand-written fixture (`fixtures/small-mailbox.json`): a few folders,
    /// threads across folders, and a family calendar. No real data.
    pub fn small() -> Self {
        Self::from_json(include_str!("../fixtures/small-mailbox.json"))
            .expect("bundled fixture parses")
    }
}

/// Well-known folders every mailbox has, in Outlook's order.
pub(crate) const WELL_KNOWN: &[(&str, &str)] = &[
    ("inbox", "Inbox"),
    ("drafts", "Drafts"),
    ("sentitems", "Sent Items"),
    ("deleteditems", "Deleted Items"),
    ("junkemail", "Junk Email"),
    ("archive", "Archive"),
    ("outbox", "Outbox"),
];

/// The hidden root folder's ID (`msgfolderroot`).
pub(crate) const ROOT_ID: &str = "fld-msgfolderroot";

// ---------------------------------------------------------------------------
// Indexed state

/// A change log per container (folder or calendar). Entries are `(seq, key)` where the key is
/// `u|<id>` for an item that is currently in the container and was last changed at `seq`, or
/// `r|<id>|<rest id>` for an item that left the container at `seq`.
#[derive(Default)]
pub(crate) struct ChangeLog {
    pub sets: HashMap<String, BTreeSet<(u64, String)>>,
}

impl ChangeLog {
    fn upsert(&mut self, container: &str, id: &str, old_seq: Option<u64>, seq: u64) {
        let set = self.sets.entry(container.to_string()).or_default();
        let key = format!("u|{id}");
        if let Some(old) = old_seq {
            set.remove(&(old, key.clone()));
        }
        set.insert((seq, key));
    }

    fn remove(&mut self, container: &str, id: &str, rest: &str, old_seq: u64, seq: u64) {
        let set = self.sets.entry(container.to_string()).or_default();
        set.remove(&(old_seq, format!("u|{id}")));
        set.insert((seq, format!("r|{id}|{rest}")));
    }

    pub fn get(&self, container: &str) -> Option<&BTreeSet<(u64, String)>> {
        self.sets.get(container)
    }
}

pub(crate) struct MessageRecord {
    pub msg: Message,
    pub rest_id: String,
    pub rest_version: u32,
    pub change_key: u64,
    pub changed_seq: u64,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
}

pub(crate) struct EventRecord {
    pub ev: Event,
    pub change_key: u64,
    pub changed_seq: u64,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
}

pub(crate) struct FolderRecord {
    pub folder: Folder,
    pub changed_seq: u64,
}

/// Errors from mailbox mutations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationError {
    /// No message, event, folder or calendar with that ID.
    NotFound(String),
}

impl std::fmt::Display for MutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "not found: {id}"),
        }
    }
}

impl std::error::Error for MutationError {}

pub(crate) struct MailboxState {
    pub user: User,
    /// Folders in insertion order, root first.
    pub folders: Vec<FolderRecord>,
    pub messages: HashMap<String, MessageRecord>,
    /// REST ID → immutable ID.
    pub rest_ids: HashMap<String, String>,
    /// Folder → (received, immutable ID).
    pub by_folder: HashMap<String, BTreeSet<(DateTime<Utc>, String)>>,
    pub msg_log: ChangeLog,
    pub folder_log: ChangeLog,
    pub event_log: ChangeLog,
    pub calendars: Vec<Calendar>,
    pub events: HashMap<String, EventRecord>,
    pub seq: u64,
    pub delta_epoch: u64,
    next_id: u64,
    pub in_flight: usize,
    pub peak_in_flight: usize,
}

/// The single container key used for the folder change log.
pub(crate) const FOLDERS: &str = "";

impl MailboxState {
    pub fn new(mailbox: Mailbox) -> Self {
        let mut st = Self {
            user: mailbox.user,
            folders: Vec::new(),
            messages: HashMap::new(),
            rest_ids: HashMap::new(),
            by_folder: HashMap::new(),
            msg_log: ChangeLog::default(),
            folder_log: ChangeLog::default(),
            event_log: ChangeLog::default(),
            calendars: Vec::new(),
            events: HashMap::new(),
            seq: 0,
            delta_epoch: 1,
            next_id: 0,
            in_flight: 0,
            peak_in_flight: 0,
        };
        st.folders.push(FolderRecord {
            folder: Folder {
                id: ROOT_ID.into(),
                display_name: "Top of Information Store".into(),
                parent_folder_id: None,
                well_known_name: Some("msgfolderroot".into()),
                is_hidden: true,
            },
            changed_seq: 0,
        });
        // Well-known folders first, using a fixture's definition when it has one.
        for (wkn, name) in WELL_KNOWN {
            let from_fixture = mailbox
                .folders
                .iter()
                .find(|f| f.well_known_name.as_deref() == Some(*wkn))
                .cloned();
            let folder = from_fixture.unwrap_or_else(|| Folder {
                id: String::new(),
                display_name: (*name).into(),
                parent_folder_id: None,
                well_known_name: Some((*wkn).into()),
                is_hidden: false,
            });
            st.insert_folder(folder);
        }
        for f in mailbox.folders {
            if f.well_known_name.is_none() {
                st.insert_folder(f);
            }
        }
        let mut has_default = false;
        for mut c in mailbox.calendars {
            if c.id.is_empty() {
                c.id = st.new_id("cal");
            }
            if has_default {
                c.is_default = false;
            }
            has_default |= c.is_default;
            st.calendars.push(c);
        }
        if !has_default {
            if st.calendars.is_empty() {
                let id = st.new_id("cal");
                st.calendars.push(Calendar {
                    id,
                    name: "Calendar".into(),
                    is_default: true,
                    color: default_color(),
                    owner: None,
                });
            } else {
                st.calendars[0].is_default = true;
            }
        }
        for m in mailbox.messages {
            st.insert_message(m).expect("fixture message folder exists");
        }
        for e in mailbox.events {
            st.insert_event(e).expect("fixture event calendar exists");
        }
        st
    }

    pub fn new_id(&mut self, kind: &str) -> String {
        self.next_id += 1;
        format!("{kind}-{:06}", self.next_id)
    }

    pub fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    // --- folders -----------------------------------------------------------

    pub fn resolve_folder(&self, id_or_name: &str) -> Option<String> {
        self.folders
            .iter()
            .find(|r| {
                r.folder.id == id_or_name
                    || r.folder
                        .well_known_name
                        .as_deref()
                        .is_some_and(|w| w.eq_ignore_ascii_case(id_or_name))
            })
            .map(|r| r.folder.id.clone())
    }

    pub fn folder(&self, id: &str) -> Option<&FolderRecord> {
        self.folders.iter().find(|r| r.folder.id == id)
    }

    pub fn insert_folder(&mut self, mut folder: Folder) -> String {
        if folder.id.is_empty() {
            folder.id = match &folder.well_known_name {
                Some(w) => format!("fld-{w}"),
                None => self.new_id("fld"),
            };
        }
        folder.parent_folder_id = Some(
            folder
                .parent_folder_id
                .as_deref()
                .and_then(|p| self.resolve_folder(p))
                .unwrap_or_else(|| ROOT_ID.to_string()),
        );
        let seq = self.next_seq();
        self.folder_log.upsert(FOLDERS, &folder.id, None, seq);
        let id = folder.id.clone();
        self.by_folder.entry(id.clone()).or_default();
        self.folders.push(FolderRecord {
            folder,
            changed_seq: seq,
        });
        id
    }

    pub fn update_folder(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut Folder),
    ) -> Result<(), MutationError> {
        let id = self
            .resolve_folder(id)
            .ok_or_else(|| MutationError::NotFound(id.into()))?;
        let seq = self.next_seq();
        let rec = self
            .folders
            .iter_mut()
            .find(|r| r.folder.id == id)
            .expect("resolved");
        f(&mut rec.folder);
        rec.folder.id = id.clone();
        let old = rec.changed_seq;
        rec.changed_seq = seq;
        self.folder_log.upsert(FOLDERS, &id, Some(old), seq);
        Ok(())
    }

    /// Deletes a folder, its subfolders and their messages.
    pub fn delete_folder(&mut self, id: &str) -> Result<(), MutationError> {
        let id = self
            .resolve_folder(id)
            .ok_or_else(|| MutationError::NotFound(id.into()))?;
        let children: Vec<String> = self
            .folders
            .iter()
            .filter(|r| r.folder.parent_folder_id.as_deref() == Some(id.as_str()))
            .map(|r| r.folder.id.clone())
            .collect();
        for c in children {
            self.delete_folder(&c)?;
        }
        let msgs: Vec<String> = self
            .by_folder
            .get(&id)
            .map(|s| s.iter().map(|(_, m)| m.clone()).collect())
            .unwrap_or_default();
        for m in msgs {
            self.delete_message(&m)?;
        }
        let pos = self
            .folders
            .iter()
            .position(|r| r.folder.id == id)
            .expect("resolved");
        let rec = self.folders.remove(pos);
        let seq = self.next_seq();
        self.folder_log
            .remove(FOLDERS, &id, &id, rec.changed_seq, seq);
        self.by_folder.remove(&id);
        Ok(())
    }

    // --- messages ----------------------------------------------------------

    pub fn lookup_message(&self, id: &str) -> Option<&str> {
        if let Some(imm) = self.rest_ids.get(id) {
            return Some(imm);
        }
        self.messages.get_key_value(id).map(|(k, _)| k.as_str())
    }

    fn rest_id_for(imm: &str, version: u32) -> String {
        format!("rest{version}-{imm}")
    }

    pub fn insert_message(&mut self, mut msg: Message) -> Result<String, MutationError> {
        let folder = self
            .resolve_folder(&msg.folder_id)
            .ok_or_else(|| MutationError::NotFound(msg.folder_id.clone()))?;
        msg.folder_id = folder.clone();
        if msg.id.is_empty() {
            msg.id = self.new_id("imm");
        }
        if msg.internet_message_id.is_empty() {
            msg.internet_message_id = format!("<{}@mock.familiar.invalid>", msg.id);
        }
        if msg.conversation_id.is_empty() {
            msg.conversation_id = conversation_from_subject(&msg.subject);
        }
        let seq = self.next_seq();
        let id = msg.id.clone();
        let rest_id = Self::rest_id_for(&id, 1);
        self.rest_ids.insert(rest_id.clone(), id.clone());
        self.by_folder
            .entry(folder.clone())
            .or_default()
            .insert((msg.received_date_time, id.clone()));
        self.msg_log.upsert(&folder, &id, None, seq);
        let created = msg.received_date_time;
        self.messages.insert(
            id.clone(),
            MessageRecord {
                msg,
                rest_id,
                rest_version: 1,
                change_key: 1,
                changed_seq: seq,
                created,
                modified: created,
            },
        );
        Ok(id)
    }

    pub fn update_message(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut Message),
    ) -> Result<(), MutationError> {
        let imm = self
            .lookup_message(id)
            .ok_or_else(|| MutationError::NotFound(id.into()))?
            .to_string();
        let seq = self.next_seq();
        let rec = self.messages.get_mut(&imm).expect("looked up");
        let before_received = rec.msg.received_date_time;
        let folder = rec.msg.folder_id.clone();
        f(&mut rec.msg);
        // ID and folder are not changed by an update; use move_message for that.
        rec.msg.id = imm.clone();
        rec.msg.folder_id = folder.clone();
        rec.change_key += 1;
        rec.modified = Utc::now();
        let old = rec.changed_seq;
        rec.changed_seq = seq;
        let after_received = rec.msg.received_date_time;
        if before_received != after_received {
            let set = self.by_folder.entry(folder.clone()).or_default();
            set.remove(&(before_received, imm.clone()));
            set.insert((after_received, imm.clone()));
        }
        self.msg_log.upsert(&folder, &imm, Some(old), seq);
        Ok(())
    }

    /// Moves a message. Its REST ID changes; its immutable ID does not.
    pub fn move_message(&mut self, id: &str, folder: &str) -> Result<String, MutationError> {
        let imm = self
            .lookup_message(id)
            .ok_or_else(|| MutationError::NotFound(id.into()))?
            .to_string();
        let dest = self
            .resolve_folder(folder)
            .ok_or_else(|| MutationError::NotFound(folder.into()))?;
        let seq = self.next_seq();
        let rec = self.messages.get_mut(&imm).expect("looked up");
        let src = rec.msg.folder_id.clone();
        if src == dest {
            return Ok(rec.rest_id.clone());
        }
        let old_rest = rec.rest_id.clone();
        rec.rest_version += 1;
        rec.rest_id = Self::rest_id_for(&imm, rec.rest_version);
        rec.msg.folder_id = dest.clone();
        rec.change_key += 1;
        rec.modified = Utc::now();
        let old_seq = rec.changed_seq;
        rec.changed_seq = seq;
        let received = rec.msg.received_date_time;
        let new_rest = rec.rest_id.clone();
        self.rest_ids.remove(&old_rest);
        self.rest_ids.insert(new_rest.clone(), imm.clone());
        if let Some(s) = self.by_folder.get_mut(&src) {
            s.remove(&(received, imm.clone()));
        }
        self.by_folder
            .entry(dest.clone())
            .or_default()
            .insert((received, imm.clone()));
        self.msg_log.remove(&src, &imm, &old_rest, old_seq, seq);
        self.msg_log.upsert(&dest, &imm, None, seq);
        Ok(new_rest)
    }

    /// Hard-deletes a message (as Graph reports a permanent delete or a move out of a folder).
    pub fn delete_message(&mut self, id: &str) -> Result<(), MutationError> {
        let imm = self
            .lookup_message(id)
            .ok_or_else(|| MutationError::NotFound(id.into()))?
            .to_string();
        let seq = self.next_seq();
        let rec = self.messages.remove(&imm).expect("looked up");
        self.rest_ids.remove(&rec.rest_id);
        if let Some(s) = self.by_folder.get_mut(&rec.msg.folder_id) {
            s.remove(&(rec.msg.received_date_time, imm.clone()));
        }
        self.msg_log
            .remove(&rec.msg.folder_id, &imm, &rec.rest_id, rec.changed_seq, seq);
        Ok(())
    }

    // --- calendars and events ---------------------------------------------

    pub fn default_calendar(&self) -> &str {
        &self
            .calendars
            .iter()
            .find(|c| c.is_default)
            .expect("a default calendar always exists")
            .id
    }

    pub fn resolve_calendar(&self, id: &str) -> Option<String> {
        if id.is_empty() {
            return Some(self.default_calendar().to_string());
        }
        self.calendars
            .iter()
            .find(|c| c.id == id || c.name.eq_ignore_ascii_case(id))
            .map(|c| c.id.clone())
    }

    pub fn add_calendar(&mut self, mut cal: Calendar) -> String {
        if cal.id.is_empty() {
            cal.id = self.new_id("cal");
        }
        cal.is_default = false;
        let id = cal.id.clone();
        self.calendars.push(cal);
        id
    }

    pub fn insert_event(&mut self, mut ev: Event) -> Result<String, MutationError> {
        let cal = self
            .resolve_calendar(&ev.calendar_id)
            .ok_or_else(|| MutationError::NotFound(ev.calendar_id.clone()))?;
        ev.calendar_id = cal.clone();
        if ev.id.is_empty() {
            ev.id = self.new_id("evt");
        }
        let seq = self.next_seq();
        let id = ev.id.clone();
        self.event_log.upsert(&cal, &id, None, seq);
        let created = ev.start - chrono::Duration::days(7);
        self.events.insert(
            id.clone(),
            EventRecord {
                ev,
                change_key: 1,
                changed_seq: seq,
                created,
                modified: created,
            },
        );
        Ok(id)
    }

    pub fn update_event(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut Event),
    ) -> Result<(), MutationError> {
        if !self.events.contains_key(id) {
            return Err(MutationError::NotFound(id.into()));
        }
        let seq = self.next_seq();
        let rec = self.events.get_mut(id).expect("checked");
        let cal = rec.ev.calendar_id.clone();
        f(&mut rec.ev);
        rec.ev.id = id.to_string();
        rec.ev.calendar_id = cal.clone();
        rec.change_key += 1;
        rec.modified = Utc::now();
        let old = rec.changed_seq;
        rec.changed_seq = seq;
        self.event_log.upsert(&cal, id, Some(old), seq);
        Ok(())
    }

    pub fn delete_event(&mut self, id: &str) -> Result<(), MutationError> {
        let rec = self
            .events
            .remove(id)
            .ok_or_else(|| MutationError::NotFound(id.into()))?;
        let seq = self.next_seq();
        self.event_log
            .remove(&rec.ev.calendar_id, id, id, rec.changed_seq, seq);
        Ok(())
    }
}

/// A stable conversation ID from a subject with reply and forward prefixes removed.
pub(crate) fn conversation_from_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let stripped = ["re:", "fw:", "fwd:"]
            .iter()
            .find(|p| lower.starts_with(*p))
            .map(|p| s[p.len()..].trim_start());
        match stripped {
            Some(rest) => s = rest,
            None => break,
        }
    }
    format!("conv-{:016x}", fnv1a(s.to_ascii_lowercase().as_bytes()))
}

pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
