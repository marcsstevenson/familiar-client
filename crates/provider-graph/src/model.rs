// SPDX-License-Identifier: MIT OR Apache-2.0
//! Graph's mail JSON, and how it maps onto the store.

use serde::Deserialize;
use serde_json::Value;

use familiar_store::{AccountId, Address, FolderId, NewMessage};

use crate::error::{Error, Result};

/// The message fields fetched for headers. Bodies are fetched separately.
pub const HEADER_FIELDS: &str = "id,parentFolderId,conversationId,internetMessageId,subject,from,\
toRecipients,ccRecipients,receivedDateTime,sentDateTime,isRead,isDraft,flag,importance,\
hasAttachments,bodyPreview";

/// The folder fields fetched.
pub const FOLDER_FIELDS: &str =
    "id,displayName,parentFolderId,childFolderCount,totalItemCount,unreadItemCount,isHidden";

/// Graph's well-known folder names that Familiar maps.
pub const WELL_KNOWN_FOLDERS: &[&str] = &[
    "inbox",
    "drafts",
    "sentitems",
    "deleteditems",
    "junkemail",
    "archive",
    "outbox",
];

/// A mail folder.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphFolder {
    /// Folder ID.
    pub id: String,
    /// Name shown to the user.
    #[serde(default)]
    pub display_name: String,
    /// Parent folder ID; the root (`msgfolderroot`) for top-level folders.
    #[serde(default)]
    pub parent_folder_id: Option<String>,
    /// Number of direct subfolders.
    #[serde(default)]
    pub child_folder_count: i64,
    /// Messages in the folder.
    #[serde(default)]
    pub total_item_count: i64,
    /// Unread messages in the folder.
    #[serde(default)]
    pub unread_item_count: i64,
    /// Hidden folders (Conversation History and the like) are not synced.
    #[serde(default)]
    pub is_hidden: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
struct GraphEmail {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    address: Option<String>,
}

/// A recipient or sender.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphRecipient {
    #[serde(default)]
    email_address: GraphEmail,
}

impl GraphRecipient {
    fn to_address(&self) -> Address {
        Address {
            name: self.email_address.name.clone().unwrap_or_default(),
            addr: self.email_address.address.clone().unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct GraphFlag {
    #[serde(default)]
    flag_status: Option<String>,
}

/// A message body.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphBody {
    /// `html` or `text`.
    #[serde(default)]
    pub content_type: String,
    /// The body itself.
    #[serde(default)]
    pub content: String,
}

impl GraphBody {
    /// Whether the body is HTML.
    pub fn is_html(&self) -> bool {
        self.content_type.eq_ignore_ascii_case("html")
    }
}

/// A message's header fields (see [`HEADER_FIELDS`]).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphMessage {
    /// Immutable ID (the client always asks for them).
    pub id: String,
    /// Folder the message is in.
    #[serde(default)]
    pub parent_folder_id: Option<String>,
    /// Graph's conversation ID.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// RFC 5322 Message-ID.
    #[serde(default)]
    pub internet_message_id: Option<String>,
    /// Subject line.
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    from: Option<GraphRecipient>,
    #[serde(default)]
    to_recipients: Vec<GraphRecipient>,
    #[serde(default)]
    cc_recipients: Vec<GraphRecipient>,
    /// RFC 3339 arrival time.
    #[serde(default)]
    pub received_date_time: Option<String>,
    /// RFC 3339 send time.
    #[serde(default)]
    pub sent_date_time: Option<String>,
    /// Read state.
    #[serde(default)]
    pub is_read: bool,
    /// Draft state.
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    flag: Option<GraphFlag>,
    /// `low`, `normal` or `high`.
    #[serde(default)]
    pub importance: Option<String>,
    /// Whether it has attachments.
    #[serde(default)]
    pub has_attachments: bool,
    /// Graph's text preview.
    #[serde(default)]
    pub body_preview: Option<String>,
    /// Only present when `body` was selected.
    #[serde(default)]
    pub body: Option<GraphBody>,
}

impl GraphMessage {
    /// Whether the follow-up flag is set.
    pub fn is_flagged(&self) -> bool {
        self.flag
            .as_ref()
            .and_then(|f| f.flag_status.as_deref())
            .is_some_and(|s| s.eq_ignore_ascii_case("flagged"))
    }

    /// The header as a store row in `folder`. The body is left out; the store
    /// keeps a body it already has.
    pub fn to_new_message(&self, account: AccountId, folder: FolderId) -> NewMessage {
        let received_at = self
            .received_date_time
            .as_deref()
            .and_then(parse_time)
            .unwrap_or(0);
        NewMessage {
            account,
            folder,
            remote_id: self.id.clone(),
            internet_message_id: self.internet_message_id.clone().filter(|s| !s.is_empty()),
            // Graph always sends one; a missing ID makes the message its own thread.
            conversation_id: self
                .conversation_id
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("familiar:{}", self.id)),
            subject: self.subject.clone().unwrap_or_default(),
            from: self
                .from
                .as_ref()
                .map(GraphRecipient::to_address)
                .unwrap_or_default(),
            to: self
                .to_recipients
                .iter()
                .map(GraphRecipient::to_address)
                .collect(),
            cc: self
                .cc_recipients
                .iter()
                .map(GraphRecipient::to_address)
                .collect(),
            received_at,
            sent_at: self.sent_date_time.as_deref().and_then(parse_time),
            is_read: self.is_read,
            is_flagged: self.is_flagged(),
            importance: importance(self.importance.as_deref()),
            has_attachments: self.has_attachments,
            preview: self.body_preview.clone().unwrap_or_default(),
            body: None,
        }
    }
}

/// `low` = 0, `normal` = 1, `high` = 2, as the store keeps it.
pub fn importance(s: Option<&str>) -> i64 {
    match s.map(str::to_ascii_lowercase).as_deref() {
        Some("low") => 0,
        Some("high") => 2,
        _ => 1,
    }
}

/// RFC 3339 to Unix seconds.
pub fn parse_time(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.timestamp())
}

/// One entry of a delta page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeltaItem<T> {
    /// Added or changed.
    Upsert(T),
    /// Gone from the collection: deleted, or moved somewhere else. The ID is
    /// immutable, so a move shows up again under the same ID elsewhere.
    Removed(String),
}

/// One page of a collection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    /// The items.
    pub items: Vec<T>,
    /// Link to the next page, if there is one.
    pub next_link: Option<String>,
    /// For delta: the link to resume from next round, on the last page.
    pub delta_link: Option<String>,
}

pub(crate) fn decode_page<T: serde::de::DeserializeOwned>(v: Value) -> Result<Page<T>> {
    let (items, next_link, delta_link) = split_page(v)?;
    let items = items
        .into_iter()
        .map(serde_json::from_value)
        .collect::<std::result::Result<Vec<T>, _>>()?;
    Ok(Page {
        items,
        next_link,
        delta_link,
    })
}

pub(crate) fn decode_delta_page<T: serde::de::DeserializeOwned>(
    v: Value,
) -> Result<Page<DeltaItem<T>>> {
    let (items, next_link, delta_link) = split_page(v)?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if item.get("@removed").is_some() {
            let id = item["id"]
                .as_str()
                .ok_or_else(|| Error::Decode("@removed entry without an id".into()))?;
            out.push(DeltaItem::Removed(id.to_string()));
        } else {
            out.push(DeltaItem::Upsert(serde_json::from_value(item)?));
        }
    }
    Ok(Page {
        items: out,
        next_link,
        delta_link,
    })
}

type RawPage = (Vec<Value>, Option<String>, Option<String>);

fn split_page(mut v: Value) -> Result<RawPage> {
    let items = match v.get_mut("value").map(Value::take) {
        Some(Value::Array(a)) => a,
        _ => return Err(Error::Decode("collection without a value array".into())),
    };
    let link = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Ok((items, link("@odata.nextLink"), link("@odata.deltaLink")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn header_maps_to_store_row() {
        let m: GraphMessage = serde_json::from_value(json!({
            "id": "imm-1", "parentFolderId": "fld-inbox", "conversationId": "conv",
            "internetMessageId": "<a@b>", "subject": "Hi",
            "from": {"emailAddress": {"name": "Sam", "address": "sam@example.com"}},
            "toRecipients": [{"emailAddress": {"name": "Pat", "address": "pat@example.com"}}],
            "receivedDateTime": "2026-11-03T19:02:00Z", "sentDateTime": "2026-11-03T19:01:00Z",
            "isRead": true, "flag": {"flagStatus": "flagged"}, "importance": "high",
            "hasAttachments": true, "bodyPreview": "Hello"
        }))
        .unwrap();
        let n = m.to_new_message(AccountId(1), FolderId(2));
        assert_eq!(n.remote_id, "imm-1");
        assert_eq!(n.received_at, 1_793_732_520);
        assert_eq!(n.sent_at, Some(1_793_732_460));
        assert!(n.is_read && n.is_flagged && n.has_attachments);
        assert_eq!(n.importance, 2);
        assert_eq!(n.from.addr, "sam@example.com");
        assert_eq!(n.to[0].name, "Pat");
        assert_eq!(n.internet_message_id.as_deref(), Some("<a@b>"));
    }

    #[test]
    fn sparse_message_still_maps() {
        let m: GraphMessage = serde_json::from_value(json!({"id": "x"})).unwrap();
        let n = m.to_new_message(AccountId(1), FolderId(1));
        assert_eq!(n.conversation_id, "familiar:x");
        assert_eq!(n.importance, 1);
        assert_eq!(n.internet_message_id, None);
    }

    #[test]
    fn delta_page_splits_removals() {
        let p: Page<DeltaItem<GraphMessage>> = decode_delta_page(json!({
            "value": [
                {"id": "a", "subject": "kept"},
                {"id": "b", "@removed": {"reason": "deleted"}}
            ],
            "@odata.deltaLink": "https://x/delta?$deltatoken=1"
        }))
        .unwrap();
        assert!(matches!(&p.items[0], DeltaItem::Upsert(m) if m.id == "a"));
        assert_eq!(p.items[1], DeltaItem::Removed("b".into()));
        assert_eq!(p.next_link, None);
        assert!(p.delta_link.is_some());
    }
}
