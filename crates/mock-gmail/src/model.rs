// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mailboxes: labels, messages, drafts, send-as addresses and the history log.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use chrono::{DateTime, TimeZone, Utc};

use crate::mime;

/// Gmail's system labels, in the order `labels.list` returns them.
pub const SYSTEM_LABELS: &[&str] = &[
    "CHAT",
    "SENT",
    "INBOX",
    "IMPORTANT",
    "TRASH",
    "DRAFT",
    "SPAM",
    "CATEGORY_FORUMS",
    "CATEGORY_UPDATES",
    "CATEGORY_PERSONAL",
    "CATEGORY_PROMOTIONS",
    "CATEGORY_SOCIAL",
    "STARRED",
    "UNREAD",
];

/// A label's colour, Gmail's `{textColor, backgroundColor}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelColor {
    /// Text colour, for example `#ffffff`.
    pub text_color: String,
    /// Background colour, for example `#4a86e8`.
    pub background_color: String,
}

/// A Gmail label, system or user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    /// Label ID: the name for system labels, `Label_<n>` for user labels.
    pub id: String,
    /// Display name. User labels nest by `/` (`Clients/Acme`).
    pub name: String,
    /// A system label such as `INBOX`.
    pub system: bool,
    /// `show` or `hide`.
    pub message_list_visibility: Option<String>,
    /// `labelShow`, `labelShowIfUnread` or `labelHide`.
    pub label_list_visibility: Option<String>,
    /// Colour, user labels only.
    pub color: Option<LabelColor>,
}

impl Label {
    /// A user label. Its ID is assigned when the mailbox is built unless set with [`Label::id`].
    pub fn user(name: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            name: name.into(),
            system: false,
            message_list_visibility: Some("show".into()),
            label_list_visibility: Some("labelShow".into()),
            color: None,
        }
    }

    /// Hidden from the label list (`labelHide`).
    pub fn hidden(mut self) -> Self {
        self.label_list_visibility = Some("labelHide".into());
        self
    }

    /// Sets the colour.
    pub fn color(mut self, text: &str, background: &str) -> Self {
        self.color = Some(LabelColor {
            text_color: text.into(),
            background_color: background.into(),
        });
        self
    }

    /// Sets the ID, so tests can refer to it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    fn system(id: &str) -> Self {
        let visible = !(id.starts_with("CATEGORY_") || id == "UNREAD");
        Self {
            id: id.into(),
            name: id.into(),
            system: true,
            message_list_visibility: visible.then(|| "hide".into()),
            label_list_visibility: visible.then(|| {
                if id == "CHAT" {
                    "labelHide".into()
                } else {
                    "labelShow".into()
                }
            }),
            color: None,
        }
    }
}

/// A send-as address from `settings.sendAs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendAs {
    /// The address.
    pub email: String,
    /// Display name used in `From`.
    pub display_name: String,
    /// Reply-to address, often empty.
    pub reply_to: String,
    /// HTML signature, often empty.
    pub signature: String,
    /// The account's own address.
    pub is_primary: bool,
    /// The default From address.
    pub is_default: bool,
    /// Treated as an alias of the account.
    pub treat_as_alias: bool,
    /// `accepted` or `pending`; `None` for the primary address.
    pub verification_status: Option<String>,
}

impl SendAs {
    /// A verified alias.
    pub fn alias(email: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            email: email.into(),
            display_name: display_name.into(),
            reply_to: String::new(),
            signature: String::new(),
            is_primary: false,
            is_default: false,
            treat_as_alias: true,
            verification_status: Some("accepted".into()),
        }
    }

    /// Not yet verified: Gmail rewrites `From` when mail is sent from it.
    pub fn pending(mut self) -> Self {
        self.verification_status = Some("pending".into());
        self
    }

    /// Sets the HTML signature.
    pub fn signature(mut self, html: impl Into<String>) -> Self {
        self.signature = html.into();
        self
    }

    fn primary(email: &str, name: &str) -> Self {
        Self {
            email: email.into(),
            display_name: name.into(),
            reply_to: String::new(),
            signature: String::new(),
            is_primary: true,
            is_default: true,
            treat_as_alias: false,
            verification_status: None,
        }
    }
}

enum Content {
    Raw(Vec<u8>),
    Built {
        from: String,
        to: String,
        subject: String,
        body: String,
        message_id: Option<String>,
        in_reply_to: Option<String>,
        attachment: Option<(String, Vec<u8>)>,
    },
}

/// A message to add to a mailbox, either built from a few fields or given as raw RFC 5322.
///
/// ```
/// use familiar_mock_gmail::Message;
/// let m = Message::new("ann@example.com", "me@gmail.com", "Lunch?", "Friday at one?")
///     .labels(&["INBOX", "UNREAD", "Label_1"]);
/// # let _ = m;
/// ```
pub struct Message {
    id: Option<String>,
    thread_id: Option<String>,
    labels: Vec<String>,
    date: DateTime<Utc>,
    content: Content,
}

impl Message {
    /// A plain-text message, labelled `INBOX` and `UNREAD`, dated 1 Nov 2026 unless
    /// [`Message::at`] says otherwise.
    pub fn new(from: &str, to: &str, subject: &str, body: &str) -> Self {
        Self {
            id: None,
            thread_id: None,
            labels: vec!["INBOX".into(), "UNREAD".into()],
            date: Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap(),
            content: Content::Built {
                from: from.into(),
                to: to.into(),
                subject: subject.into(),
                body: body.into(),
                message_id: None,
                in_reply_to: None,
                attachment: None,
            },
        }
    }

    /// A raw RFC 5322 message, with no labels. Its internal date comes from the `Date`
    /// header, else now.
    pub fn from_raw(raw: impl Into<Vec<u8>>) -> Self {
        let raw = raw.into();
        let (h, _) = mime::split(&raw);
        let date = mime::header(&h, "date")
            .and_then(|d| DateTime::parse_from_rfc2822(d).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        Self {
            id: None,
            thread_id: None,
            labels: Vec::new(),
            date,
            content: Content::Raw(raw),
        }
    }

    /// Replaces the label IDs.
    pub fn labels(mut self, labels: &[&str]) -> Self {
        self.labels = labels.iter().map(|l| l.to_string()).collect();
        self
    }

    /// Sets the date (and, for built messages, the `Date` header).
    pub fn at(mut self, date: DateTime<Utc>) -> Self {
        self.date = date;
        self
    }

    /// Sets the message ID instead of having one assigned.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Puts the message in this thread instead of threading it by `References`.
    pub fn thread(mut self, thread_id: impl Into<String>) -> Self {
        self.thread_id = Some(thread_id.into());
        self
    }

    /// Sets the `Message-ID` header of a built message (without angle brackets).
    pub fn message_id(mut self, id: &str) -> Self {
        if let Content::Built { message_id, .. } = &mut self.content {
            *message_id = Some(id.into());
        }
        self
    }

    /// Makes a built message a reply: `In-Reply-To` and `References` name `parent`
    /// (a `Message-ID` without angle brackets), so it joins the parent's thread.
    pub fn in_reply_to(mut self, parent: &str) -> Self {
        if let Content::Built { in_reply_to, .. } = &mut self.content {
            *in_reply_to = Some(parent.into());
        }
        self
    }

    /// Adds a file attachment to a built message.
    pub fn attach(mut self, filename: &str, bytes: &[u8]) -> Self {
        if let Content::Built { attachment, .. } = &mut self.content {
            *attachment = Some((filename.into(), bytes.to_vec()));
        }
        self
    }

    fn render(&self, fallback_id: &str) -> Vec<u8> {
        match &self.content {
            Content::Raw(r) => r.clone(),
            Content::Built {
                from,
                to,
                subject,
                body,
                message_id,
                in_reply_to,
                attachment,
            } => {
                let mid = message_id
                    .clone()
                    .unwrap_or_else(|| format!("{fallback_id}@mock.gmail"));
                let mut s = format!(
                    "From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\nDate: {}\r\n\
                     Message-ID: <{mid}>\r\nMIME-Version: 1.0\r\n",
                    self.date.to_rfc2822()
                );
                if let Some(p) = in_reply_to {
                    s.push_str(&format!("In-Reply-To: <{p}>\r\nReferences: <{p}>\r\n"));
                }
                match attachment {
                    None => {
                        s.push_str("Content-Type: text/plain; charset=UTF-8\r\n\r\n");
                        s.push_str(body);
                    }
                    Some((name, bytes)) => {
                        use base64::Engine;
                        let b = "mock-boundary-1";
                        s.push_str(&format!(
                            "Content-Type: multipart/mixed; boundary=\"{b}\"\r\n\r\n\
                             --{b}\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n{body}\r\n\
                             --{b}\r\nContent-Type: application/octet-stream; name=\"{name}\"\r\n\
                             Content-Disposition: attachment; filename=\"{name}\"\r\n\
                             Content-Transfer-Encoding: base64\r\n\r\n{}\r\n--{b}--\r\n",
                            base64::engine::general_purpose::STANDARD.encode(bytes)
                        ));
                    }
                }
                s.into_bytes()
            }
        }
    }
}

/// A stored message as the mock holds it, for test assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageInfo {
    /// Message ID.
    pub id: String,
    /// Thread ID.
    pub thread_id: String,
    /// Label IDs.
    pub label_ids: Vec<String>,
    /// Gmail's `internalDate`.
    pub internal_date: DateTime<Utc>,
    /// The history ID of the last change to this message.
    pub history_id: u64,
    /// The RFC 5322 message.
    pub raw: Vec<u8>,
}

impl MessageInfo {
    /// A top-level header, unfolded.
    pub fn header(&self, name: &str) -> Option<String> {
        let (h, _) = mime::split(&self.raw);
        mime::header(&h, name).map(str::to_string)
    }
}

/// A mailbox to serve: the account, user labels, messages and send-as aliases.
///
/// Messages labelled `DRAFT` become drafts. System labels are always present.
pub struct Mailbox {
    /// The account's address.
    pub email: String,
    /// The account holder's name.
    pub name: String,
    /// User labels.
    pub labels: Vec<Label>,
    /// Messages, any order.
    pub messages: Vec<Message>,
    /// Send-as aliases besides the primary address, which is added for you.
    pub send_as: Vec<SendAs>,
}

impl Mailbox {
    /// An empty mailbox.
    pub fn new(email: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            email: email.into(),
            name: name.into(),
            labels: Vec::new(),
            messages: Vec::new(),
            send_as: Vec::new(),
        }
    }

    /// Adds a user label.
    pub fn label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    /// Adds a message.
    pub fn message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }

    /// Adds a send-as alias.
    pub fn send_as(mut self, alias: SendAs) -> Self {
        self.send_as.push(alias);
        self
    }

    /// The small hand-written mailbox most tests use. See the README for what it holds.
    pub fn small() -> Self {
        crate::fixture::small()
    }

    /// A deterministic mailbox of `messages` messages across the Inbox and a few labels,
    /// in threads of up to three, one a day back from 1 Nov 2026 in 10-minute steps.
    pub fn synthetic(messages: usize, seed: u64) -> Self {
        crate::fixture::synthetic(messages, seed)
    }
}

/// Why a change to a mailbox was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationError {
    /// No message, draft or label with this ID.
    NotFound(String),
    /// A label ID that doesn't exist.
    InvalidLabel(String),
    /// A label name or message ID already in use.
    Conflict(String),
    /// Anything else Gmail would answer 400 to.
    Invalid(String),
}

impl fmt::Display for MutationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "not found: {id}"),
            Self::InvalidLabel(id) => write!(f, "Invalid label: {id}"),
            Self::Conflict(what) => write!(f, "already exists: {what}"),
            Self::Invalid(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for MutationError {}

pub(crate) struct Stored {
    pub id: String,
    pub thread_id: String,
    pub labels: Vec<String>,
    pub date_ms: i64,
    pub raw: Vec<u8>,
    pub history_id: u64,
    pub headers: Vec<(String, String)>,
    pub text: String,
    pub has_attachment: bool,
}

impl Stored {
    pub fn has(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }

    pub fn info(&self) -> MessageInfo {
        MessageInfo {
            id: self.id.clone(),
            thread_id: self.thread_id.clone(),
            label_ids: self.labels.clone(),
            internal_date: Utc
                .timestamp_millis_opt(self.date_ms)
                .single()
                .unwrap_or_default(),
            history_id: self.history_id,
            raw: self.raw.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Added,
    Deleted,
    LabelsAdded(Vec<String>),
    LabelsRemoved(Vec<String>),
}

pub(crate) struct HistoryRecord {
    pub id: u64,
    pub msg_id: String,
    pub thread_id: String,
    /// Labels after the change (before it, for a delete).
    pub labels: Vec<String>,
    pub change: Change,
}

/// The first history ID a mailbox hands out.
const FIRST_HISTORY_ID: u64 = 100_000;

pub(crate) struct MailboxState {
    pub email: String,
    pub name: String,
    pub labels: Vec<Label>,
    pub messages: HashMap<String, Stored>,
    /// Draft ID to message ID.
    pub drafts: BTreeMap<String, String>,
    pub send_as: Vec<SendAs>,
    pub history: Vec<HistoryRecord>,
    pub history_id: u64,
    /// History at or before this ID is gone: `history.list` from it answers 404.
    pub history_floor: u64,
    by_message_id: HashMap<String, String>,
    next_msg: u64,
    next_label: u64,
    next_draft: u64,
    pub sent: Vec<String>,
    pub in_flight: usize,
    pub peak_in_flight: usize,
    pub units_used: u64,
}

impl MailboxState {
    pub fn new(mb: Mailbox) -> Self {
        let mut send_as = vec![SendAs::primary(&mb.email, &mb.name)];
        send_as.extend(mb.send_as);
        let mut st = Self {
            email: mb.email,
            name: mb.name,
            labels: SYSTEM_LABELS.iter().map(|l| Label::system(l)).collect(),
            messages: HashMap::new(),
            drafts: BTreeMap::new(),
            send_as,
            history: Vec::new(),
            history_id: FIRST_HISTORY_ID,
            history_floor: FIRST_HISTORY_ID,
            by_message_id: HashMap::new(),
            next_msg: 0,
            next_label: 0,
            next_draft: 0,
            sent: Vec::new(),
            in_flight: 0,
            peak_in_flight: 0,
            units_used: 0,
        };
        for l in mb.labels {
            st.add_label(l).expect("fixture labels are valid");
        }
        for m in mb.messages {
            let is_draft = m.labels.iter().any(|l| l == "DRAFT");
            let id = st.insert(m).expect("fixture messages are valid");
            if is_draft {
                st.add_draft(&id);
            }
        }
        // Loading the fixture is not history a client could have seen.
        st.history.clear();
        st.history_floor = st.history_id;
        st
    }

    fn bump(&mut self) -> u64 {
        self.history_id += 1;
        self.history_id
    }

    fn record(&mut self, msg_id: &str, change: Change) {
        let id = self.bump();
        let Some(m) = self.messages.get_mut(msg_id) else {
            return;
        };
        m.history_id = id;
        let rec = HistoryRecord {
            id,
            msg_id: m.id.clone(),
            thread_id: m.thread_id.clone(),
            labels: m.labels.clone(),
            change,
        };
        self.history.push(rec);
    }

    pub fn label(&self, id: &str) -> Option<&Label> {
        self.labels.iter().find(|l| l.id == id)
    }

    fn check_labels(&self, labels: &[String]) -> Result<(), MutationError> {
        match labels.iter().find(|l| self.label(l).is_none()) {
            Some(bad) => Err(MutationError::InvalidLabel(bad.clone())),
            None => Ok(()),
        }
    }

    /// Stores a message and returns its ID.
    pub fn insert(&mut self, m: Message) -> Result<String, MutationError> {
        self.check_labels(&m.labels)?;
        let id = match &m.id {
            Some(id) if self.messages.contains_key(id) => {
                return Err(MutationError::Conflict(id.clone()));
            }
            Some(id) => id.clone(),
            None => loop {
                self.next_msg += 1;
                let id = format!("{:016x}", 0x19a0_0000_0000_0000u64 + self.next_msg);
                if !self.messages.contains_key(&id) {
                    break id;
                }
            },
        };
        let raw = m.render(&id);
        let summary = mime::summarise(&raw);
        let own_mid = mime::header(&summary.headers, "message-id").map(clean_mid);
        let thread_id = m.thread_id.clone().unwrap_or_else(|| {
            let refs = [
                mime::header(&summary.headers, "in-reply-to"),
                mime::header(&summary.headers, "references"),
            ];
            refs.iter()
                .flatten()
                .flat_map(|v| v.split_whitespace())
                .find_map(|r| self.by_message_id.get(&clean_mid(r)).cloned())
                .unwrap_or_else(|| id.clone())
        });
        if let Some(mid) = own_mid {
            self.by_message_id.entry(mid).or_insert(thread_id.clone());
        }
        let mut labels: Vec<String> = Vec::new();
        for l in &m.labels {
            if !labels.contains(l) {
                labels.push(l.clone());
            }
        }
        self.messages.insert(
            id.clone(),
            Stored {
                id: id.clone(),
                thread_id,
                labels,
                date_ms: m.date.timestamp_millis(),
                raw,
                history_id: 0,
                headers: summary.headers,
                text: summary.text,
                has_attachment: summary.has_attachment,
            },
        );
        self.record(&id, Change::Added);
        Ok(id)
    }

    /// Gives a stored message a draft ID.
    pub fn add_draft(&mut self, msg_id: &str) -> String {
        self.next_draft += 1;
        let id = format!("r{:019}", 4_000_000_000_000_000_000u64 + self.next_draft);
        self.drafts.insert(id.clone(), msg_id.to_string());
        id
    }

    /// Adds and removes labels, writing a history record for each kind of change made.
    pub fn modify(
        &mut self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<(), MutationError> {
        self.check_labels(add)?;
        self.check_labels(remove)?;
        let m = self
            .messages
            .get_mut(id)
            .ok_or_else(|| MutationError::NotFound(id.into()))?;
        let mut added = Vec::new();
        for l in add {
            if !m.has(l) && !remove.contains(l) {
                m.labels.push(l.clone());
                added.push(l.clone());
            }
        }
        let mut removed = Vec::new();
        for l in remove {
            if m.has(l) {
                m.labels.retain(|x| x != l);
                removed.push(l.clone());
            }
        }
        if !added.is_empty() {
            self.record(id, Change::LabelsAdded(added));
        }
        if !removed.is_empty() {
            self.record(id, Change::LabelsRemoved(removed));
        }
        Ok(())
    }

    /// Deletes a message for good (and its draft, if it is one).
    pub fn delete(&mut self, id: &str) -> Result<(), MutationError> {
        if !self.messages.contains_key(id) {
            return Err(MutationError::NotFound(id.into()));
        }
        self.record(id, Change::Deleted);
        self.messages.remove(id);
        self.drafts.retain(|_, m| m != id);
        Ok(())
    }

    pub fn add_label(&mut self, mut l: Label) -> Result<String, MutationError> {
        if l.name.trim().is_empty() {
            return Err(MutationError::Invalid("Invalid label name".into()));
        }
        if self
            .labels
            .iter()
            .any(|x| x.name.eq_ignore_ascii_case(&l.name))
        {
            return Err(MutationError::Conflict(l.name));
        }
        if l.id.is_empty() {
            loop {
                self.next_label += 1;
                let id = format!("Label_{}", self.next_label);
                if self.label(&id).is_none() {
                    l.id = id;
                    break;
                }
            }
        }
        l.system = false;
        let id = l.id.clone();
        self.labels.push(l);
        self.bump();
        Ok(id)
    }

    pub fn update_label(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut Label),
    ) -> Result<(), MutationError> {
        let idx = self
            .labels
            .iter()
            .position(|l| l.id == id)
            .ok_or_else(|| MutationError::NotFound(id.into()))?;
        if self.labels[idx].system {
            return Err(MutationError::Invalid(format!(
                "Invalid label: {id} is a system label"
            )));
        }
        let mut l = self.labels[idx].clone();
        f(&mut l);
        l.id = id.to_string();
        if l.name.trim().is_empty() {
            return Err(MutationError::Invalid("Invalid label name".into()));
        }
        if self
            .labels
            .iter()
            .any(|x| x.id != id && x.name.eq_ignore_ascii_case(&l.name))
        {
            return Err(MutationError::Conflict(l.name));
        }
        self.labels[idx] = l;
        self.bump();
        Ok(())
    }

    /// Deletes a user label and takes it off every message.
    pub fn delete_label(&mut self, id: &str) -> Result<(), MutationError> {
        match self.label(id) {
            None => return Err(MutationError::NotFound(id.into())),
            Some(l) if l.system => {
                return Err(MutationError::Invalid(format!(
                    "Invalid delete request: {id} is a system label"
                )));
            }
            Some(_) => {}
        }
        let mut ids: Vec<String> = self
            .messages
            .values()
            .filter(|m| m.has(id))
            .map(|m| m.id.clone())
            .collect();
        ids.sort();
        let label = vec![id.to_string()];
        for m in ids {
            self.modify(&m, &[], &label)?;
        }
        self.labels.retain(|l| l.id != id);
        self.bump();
        Ok(())
    }

    /// Forgets all history so far, as Gmail does after about a week.
    pub fn expire_history(&mut self) {
        self.history.clear();
        self.history_floor = self.history_id;
    }

    /// The primary address and verified aliases, lower-cased.
    pub fn own_addresses(&self, verified_only: bool) -> Vec<String> {
        self.send_as
            .iter()
            .filter(|s| {
                !verified_only
                    || s.is_primary
                    || s.verification_status.as_deref() == Some("accepted")
            })
            .map(|s| s.email.to_ascii_lowercase())
            .collect()
    }

    /// Message IDs newest first.
    pub fn ordered(&self) -> Vec<&Stored> {
        let mut v: Vec<&Stored> = self.messages.values().collect();
        v.sort_by(|a, b| (b.date_ms, &b.id).cmp(&(a.date_ms, &a.id)));
        v
    }
}

fn clean_mid(s: &str) -> String {
    s.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}
