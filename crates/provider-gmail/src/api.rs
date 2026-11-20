// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Gmail reads Familiar makes, each returning Gmail's own data.

use base64::Engine as _;

use familiar_provider::{Error, Priority, Result};

use crate::client::GmailClient;
use crate::model::{GmailLabel, GmailMessage, HistoryPage, MessageList, Profile};

/// Headers asked for with `format=metadata`.
pub const METADATA_HEADERS: &[&str] = &[
    "From",
    "To",
    "Cc",
    "Subject",
    "Date",
    "Message-ID",
    "Content-Type",
    "Importance",
    "X-Priority",
];

/// IDs per `messages.list` page (Gmail's maximum).
pub const LIST_PAGE_SIZE: usize = 500;

/// Percent-encodes a query value.
pub(crate) fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Gmail mail reads for one mailbox.
#[derive(Clone, Debug)]
pub struct GmailApi {
    client: GmailClient,
}

impl GmailApi {
    /// Reads through `client`.
    pub fn new(client: GmailClient) -> Self {
        Self { client }
    }

    /// The client underneath.
    pub fn client(&self) -> &GmailClient {
        &self.client
    }

    /// `users.getProfile`.
    pub async fn profile(&self, priority: Priority) -> Result<Profile> {
        Ok(serde_json::from_value(
            self.client.get("/profile", priority).await?,
        )?)
    }

    /// Every label.
    pub async fn labels(&self, priority: Priority) -> Result<Vec<GmailLabel>> {
        let v = self.client.get("/labels", priority).await?;
        Ok(serde_json::from_value(v["labels"].clone()).unwrap_or_default())
    }

    /// One page of message IDs, newest first. `label` limits it to one label;
    /// `q` is a Gmail search. Spam and Trash are included.
    pub async fn list(
        &self,
        label: Option<&str>,
        q: Option<&str>,
        max: usize,
        page_token: Option<&str>,
        priority: Priority,
    ) -> Result<MessageList> {
        let mut path = format!(
            "/messages?maxResults={}&includeSpamTrash=true",
            max.clamp(1, LIST_PAGE_SIZE)
        );
        if let Some(l) = label {
            path.push_str(&format!("&labelIds={}", enc(l)));
        }
        if let Some(q) = q {
            path.push_str(&format!("&q={}", enc(q)));
        }
        if let Some(t) = page_token {
            path.push_str(&format!("&pageToken={}", enc(t)));
        }
        Ok(serde_json::from_value(
            self.client.get(&path, priority).await?,
        )?)
    }

    /// A message's labels and headers, or `None` if it's gone.
    pub async fn metadata(&self, id: &str, priority: Priority) -> Result<Option<GmailMessage>> {
        let mut path = format!("/messages/{}?format=metadata", enc(id));
        for h in METADATA_HEADERS {
            path.push_str("&metadataHeaders=");
            path.push_str(h);
        }
        match self.client.get(&path, priority).await {
            Ok(v) => Ok(Some(serde_json::from_value(v)?)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Headers for `ids`, as many in flight as the scheduler allows.
    /// Messages gone since they were listed are left out.
    pub async fn metadata_many(
        &self,
        ids: &[String],
        priority: Priority,
    ) -> Result<Vec<GmailMessage>> {
        let mut set = tokio::task::JoinSet::new();
        for id in ids {
            let api = self.clone();
            let id = id.clone();
            set.spawn(async move { api.metadata(&id, priority).await });
        }
        let mut out = Vec::with_capacity(ids.len());
        let mut first_err = None;
        while let Some(done) = set.join_next().await {
            match done.map_err(|e| Error::Decode(format!("metadata task failed: {e}")))? {
                Ok(Some(m)) => out.push(m),
                Ok(None) => {}
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }

    /// Up to `limit` messages matching a Gmail search, with their headers,
    /// newest first.
    pub async fn search(
        &self,
        q: &str,
        limit: usize,
        priority: Priority,
    ) -> Result<Vec<GmailMessage>> {
        let mut ids = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = self
                .list(
                    None,
                    Some(q),
                    limit.min(LIST_PAGE_SIZE),
                    token.as_deref(),
                    priority,
                )
                .await?;
            ids.extend(page.messages.into_iter().map(|m| m.id));
            match page.next_page_token {
                Some(t) if ids.len() < limit => token = Some(t),
                _ => break,
            }
        }
        ids.truncate(limit);
        let mut found = self.metadata_many(&ids, priority).await?;
        let order: std::collections::HashMap<&str, usize> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        found.sort_by_key(|m| order.get(m.id.as_str()).copied().unwrap_or(usize::MAX));
        Ok(found)
    }

    /// The whole message as RFC 5322 bytes.
    pub async fn raw(&self, id: &str, priority: Priority) -> Result<Vec<u8>> {
        let v = self
            .client
            .get(&format!("/messages/{}?format=raw", enc(id)), priority)
            .await?;
        let raw = v["raw"]
            .as_str()
            .ok_or_else(|| Error::Decode(format!("message {id} came without raw")))?;
        decode_b64url(raw).ok_or_else(|| Error::Decode(format!("message {id}: bad base64")))
    }

    /// One page of history after `start`. A history ID Gmail no longer
    /// keeps is [`Error::SyncStateGone`].
    pub async fn history(
        &self,
        start: &str,
        page_token: Option<&str>,
        priority: Priority,
    ) -> Result<HistoryPage> {
        let mut path = format!("/history?startHistoryId={}&maxResults=500", enc(start));
        if let Some(t) = page_token {
            path.push_str(&format!("&pageToken={}", enc(t)));
        }
        match self.client.get(&path, priority).await {
            Ok(v) => Ok(serde_json::from_value(v)?),
            Err(Error::NotFound(m)) => Err(Error::SyncStateGone(m)),
            Err(e) => Err(e),
        }
    }
}

/// Gmail's base64url, padded or not.
pub(crate) fn decode_b64url(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_queries() {
        assert_eq!(enc("from:ann is:unread"), "from%3Aann%20is%3Aunread");
        assert_eq!(enc("Label_1"), "Label_1");
    }

    #[test]
    fn base64url() {
        assert_eq!(decode_b64url("aGk_").unwrap(), b"hi?");
        assert_eq!(decode_b64url("aGk=").unwrap(), b"hi");
    }
}
