// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's JSON, as far as Familiar reads it.

use serde::Deserialize;

/// `users.getProfile`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    /// The mailbox address.
    pub email_address: String,
    /// The mailbox's history ID now.
    pub history_id: String,
    /// Messages in the mailbox.
    #[serde(default)]
    pub messages_total: u64,
}

/// One label from `labels.list`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GmailLabel {
    /// `INBOX`, `Label_12`, ...
    pub id: String,
    /// Display name; user labels nest by `/`.
    pub name: String,
    /// `system` or `user`.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    /// `labelShow`, `labelHide` or `labelShowIfUnread`.
    #[serde(default)]
    pub label_list_visibility: Option<String>,
}

impl GmailLabel {
    /// Whether this is one of Gmail's own labels.
    pub fn is_system(&self) -> bool {
        self.kind.as_deref() == Some("system")
    }
}

/// An ID pair from `messages.list` or a history record.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRef {
    /// Message ID.
    pub id: String,
    /// Thread ID.
    #[serde(default)]
    pub thread_id: String,
    /// Labels, where the answer includes them (history records).
    #[serde(default)]
    pub label_ids: Vec<String>,
}

/// A page of `messages.list`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageList {
    /// The IDs on this page.
    #[serde(default)]
    pub messages: Vec<MessageRef>,
    /// Token for the next page.
    #[serde(default)]
    pub next_page_token: Option<String>,
}

/// A header from a `metadata` message.
#[derive(Clone, Debug, Deserialize)]
pub struct Header {
    /// Header name.
    pub name: String,
    /// Header value, already unfolded by Gmail.
    pub value: String,
}

/// The `payload` of a `metadata` message.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    /// Top-level MIME type.
    #[serde(default)]
    pub mime_type: String,
    /// The headers asked for.
    #[serde(default)]
    pub headers: Vec<Header>,
}

/// `messages.get` in `metadata` or `raw` format.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GmailMessage {
    /// Message ID, fixed for the message's life.
    pub id: String,
    /// Thread ID.
    #[serde(default)]
    pub thread_id: String,
    /// Labels, `UNREAD` and `STARRED` among them.
    #[serde(default)]
    pub label_ids: Vec<String>,
    /// A short plain-text preview, HTML-escaped.
    #[serde(default)]
    pub snippet: String,
    /// Milliseconds since the epoch, as a string.
    #[serde(default)]
    pub internal_date: Option<String>,
    /// Headers (`metadata` format).
    #[serde(default)]
    pub payload: Option<Payload>,
    /// The whole message, base64url (`raw` format).
    #[serde(default)]
    pub raw: Option<String>,
}

impl GmailMessage {
    /// A header's value by name, any case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.payload
            .as_ref()?
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }

    /// Whether it carries `label`.
    pub fn has(&self, label: &str) -> bool {
        self.label_ids.iter().any(|l| l == label)
    }
}

/// A message in a history record's `messagesAdded`, `labelsAdded`, ...
#[derive(Clone, Debug, Deserialize)]
pub struct HistoryMessage {
    /// The message, with its labels after the change.
    pub message: MessageRef,
}

/// One `history.list` record.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    /// The record's history ID.
    #[serde(default)]
    pub id: String,
    /// New messages.
    #[serde(default)]
    pub messages_added: Vec<HistoryMessage>,
    /// Messages deleted for good.
    #[serde(default)]
    pub messages_deleted: Vec<HistoryMessage>,
    /// Labels added to messages.
    #[serde(default)]
    pub labels_added: Vec<HistoryMessage>,
    /// Labels removed from messages.
    #[serde(default)]
    pub labels_removed: Vec<HistoryMessage>,
}

/// A page of `history.list`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    /// The records on this page, oldest first.
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
    /// Token for the next page.
    #[serde(default)]
    pub next_page_token: Option<String>,
    /// The mailbox's history ID now: the bookmark once the last page is read.
    #[serde(default)]
    pub history_id: Option<String>,
}
