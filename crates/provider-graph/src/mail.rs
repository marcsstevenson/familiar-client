// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail reads: the folder tree, header pages, bodies and delta rounds.
//!
//! These calls return Graph's data and write nothing. [`crate::MailSync`]
//! applies them to the store.

use serde_json::Value;

use crate::client::GraphClient;
use crate::error::{Error, Result};
use crate::model::{
    DeltaItem, FOLDER_FIELDS, GraphBody, GraphFolder, GraphMessage, HEADER_FIELDS, Page,
    decode_delta_page, decode_page,
};
use crate::scheduler::Priority;

/// Messages per delta page (`Prefer: odata.maxpagesize`).
pub const DELTA_PAGE_SIZE: usize = 200;

/// Read access to one mailbox.
#[derive(Clone, Debug)]
pub struct MailApi {
    client: GraphClient,
    /// `/me` or `/users/<id>`.
    user: String,
}

impl MailApi {
    /// Reads the signed-in user's mailbox.
    pub fn new(client: GraphClient) -> Self {
        Self {
            client,
            user: "/me".into(),
        }
    }

    /// The client underneath.
    pub fn client(&self) -> &GraphClient {
        &self.client
    }

    fn path(&self, rest: &str) -> String {
        format!("{}{rest}", self.user)
    }

    /// The whole visible folder tree, walked from the top with `childFolders`,
    /// parents before children. Hidden folders are left out.
    pub async fn folder_tree(&self, priority: Priority) -> Result<Vec<GraphFolder>> {
        let mut out = Vec::new();
        let mut i = 0;
        let mut level = self
            .collect_folders(
                &self.path(&format!("/mailFolders?$top=100&$select={FOLDER_FIELDS}")),
                priority,
            )
            .await?;
        loop {
            out.append(&mut level);
            let parents: Vec<String> = out[i..]
                .iter()
                .filter(|f| f.child_folder_count > 0)
                .map(|f| f.id.clone())
                .collect();
            i = out.len();
            if parents.is_empty() {
                return Ok(out);
            }
            for id in parents {
                let url = self.path(&format!(
                    "/mailFolders/{id}/childFolders?$top=100&$select={FOLDER_FIELDS}"
                ));
                level.extend(self.collect_folders(&url, priority).await?);
            }
        }
    }

    async fn collect_folders(&self, first: &str, priority: Priority) -> Result<Vec<GraphFolder>> {
        let mut out = Vec::new();
        let mut url = first.to_string();
        loop {
            let page: Page<GraphFolder> = decode_page(self.client.get(&url, priority, &[]).await?)?;
            out.extend(page.items.into_iter().filter(|f| !f.is_hidden));
            match page.next_link {
                Some(n) => url = n,
                None => return Ok(out),
            }
        }
    }

    /// The ID of a well-known folder (`inbox`, `sentitems`, ...), or `None`
    /// if the mailbox has no such folder (personal accounts may lack `archive`).
    pub async fn well_known_folder(
        &self,
        name: &str,
        priority: Priority,
    ) -> Result<Option<String>> {
        match self
            .client
            .get(
                &self.path(&format!("/mailFolders/{name}?$select=id")),
                priority,
                &[],
            )
            .await
        {
            Ok(v) => Ok(v["id"].as_str().map(str::to_string)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// One page of the folder-tree delta. Start with `link = None`, then pass
    /// `next_link` until a page carries the `delta_link` for next time.
    pub async fn folder_delta_page(
        &self,
        link: Option<&str>,
        priority: Priority,
    ) -> Result<Page<DeltaItem<GraphFolder>>> {
        let url = match link {
            Some(l) => l.to_string(),
            None => self.path(&format!("/mailFolders/delta?$select={FOLDER_FIELDS}")),
        };
        decode_delta_page(self.client.get(&url, priority, &[]).await?)
    }

    /// One page of a folder's headers, newest first. Start with `link = None`
    /// and follow `next_link`.
    pub async fn headers_page(
        &self,
        folder: &str,
        page_size: usize,
        link: Option<&str>,
        priority: Priority,
    ) -> Result<Page<GraphMessage>> {
        let url = match link {
            Some(l) => l.to_string(),
            None => self.path(&format!(
                "/mailFolders/{folder}/messages?$top={}&$orderby=receivedDateTime%20desc&$select={HEADER_FIELDS}",
                page_size.clamp(1, 1000)
            )),
        };
        decode_page(self.client.get(&url, priority, &[]).await?)
    }

    /// One page of a folder's message delta. Start with `link = None` (a full
    /// round), or a saved delta link; follow `next_link` until a page carries
    /// the `delta_link`.
    pub async fn message_delta_page(
        &self,
        folder: &str,
        link: Option<&str>,
        page_size: usize,
        priority: Priority,
    ) -> Result<Page<DeltaItem<GraphMessage>>> {
        let url = match link {
            Some(l) => l.to_string(),
            None => self.path(&format!(
                "/mailFolders/{folder}/messages/delta?$select={HEADER_FIELDS}"
            )),
        };
        let size = format!("odata.maxpagesize={}", page_size.max(1));
        decode_delta_page(self.client.get(&url, priority, &[&size]).await?)
    }

    /// One page of a server search across the whole mailbox. `kql` is the
    /// `$search` value including its surrounding double quotes. Graph returns
    /// at most 1,000 results in all, in its own order, and refuses `$filter`
    /// and `$orderby` alongside; follow `next_link` for more.
    pub async fn search_page(
        &self,
        kql: &str,
        page_size: usize,
        link: Option<&str>,
        priority: Priority,
    ) -> Result<Page<GraphMessage>> {
        let url = match link {
            Some(l) => l.to_string(),
            None => self.path(&format!(
                "/messages?$search={}&$top={}&$select={HEADER_FIELDS}",
                encode_query_value(kql),
                page_size.clamp(1, 1000)
            )),
        };
        decode_page(self.client.get(&url, priority, &[]).await?)
    }

    /// One message's headers, wherever it is now. `None` if it no longer exists.
    pub async fn message(&self, id: &str, priority: Priority) -> Result<Option<GraphMessage>> {
        match self
            .client
            .get(
                &self.path(&format!("/messages/{id}?$select={HEADER_FIELDS}")),
                priority,
                &[],
            )
            .await
        {
            Ok(v) => Ok(Some(serde_json::from_value(v)?)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// A message's body, as Graph stores it (usually HTML).
    pub async fn body(&self, id: &str, priority: Priority) -> Result<GraphBody> {
        let v: Value = self
            .client
            .get(
                &self.path(&format!("/messages/{id}?$select=body")),
                priority,
                &[],
            )
            .await?;
        Ok(serde_json::from_value(v["body"].clone())?)
    }
}

/// Percent-encodes a query parameter value.
fn encode_query_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
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
