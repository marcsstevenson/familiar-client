// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail writes: the Graph calls behind the op queue's mail and folder ops.
//!
//! Each call is safe to repeat after a lost answer: setting read state or a
//! flag twice changes nothing, moving a message that is already in the
//! destination (immutable IDs survive moves) is a no-op, and folder create
//! looks for an existing folder of that name before making another.

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::mail::MailApi;
use crate::model::{FOLDER_FIELDS, GraphFolder, GraphMessage};
use crate::scheduler::Priority;

/// Encodes a value for a `$filter` string literal.
fn odata_string(s: &str) -> String {
    s.replace('\'', "''")
}

impl MailApi {
    /// Marks a message read or unread.
    pub async fn set_read(&self, id: &str, read: bool, priority: Priority) -> Result<()> {
        self.client()
            .patch(
                &self.path(&format!("/messages/{id}")),
                &json!({ "isRead": read }),
                priority,
            )
            .await
            .map(|_| ())
    }

    /// Flags a message, or clears its flag.
    pub async fn set_flag(&self, id: &str, flagged: bool, priority: Priority) -> Result<()> {
        let status = if flagged { "flagged" } else { "notFlagged" };
        self.client()
            .patch(
                &self.path(&format!("/messages/{id}")),
                &json!({ "flag": { "flagStatus": status } }),
                priority,
            )
            .await
            .map(|_| ())
    }

    /// Sets a message's categories.
    pub async fn set_categories(
        &self,
        id: &str,
        categories: &[String],
        priority: Priority,
    ) -> Result<()> {
        self.client()
            .patch(
                &self.path(&format!("/messages/{id}")),
                &json!({ "categories": categories }),
                priority,
            )
            .await
            .map(|_| ())
    }

    /// Moves a message to `dest` (a folder ID or well-known name) and returns
    /// it as it is now.
    pub async fn move_message(
        &self,
        id: &str,
        dest: &str,
        priority: Priority,
    ) -> Result<GraphMessage> {
        let v = self
            .client()
            .post(
                &self.path(&format!("/messages/{id}/move")),
                &json!({ "destinationId": dest }),
                priority,
            )
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    /// Deletes a message for good (Recoverable Items). Familiar's "delete"
    /// moves to Deleted Items instead; this is for items already there.
    pub async fn delete_message(&self, id: &str, priority: Priority) -> Result<()> {
        self.client()
            .delete(&self.path(&format!("/messages/{id}")), priority)
            .await
    }

    /// A direct subfolder of `parent` (`None`: the top level) with this name.
    pub async fn find_child_folder(
        &self,
        parent: Option<&str>,
        name: &str,
        priority: Priority,
    ) -> Result<Option<GraphFolder>> {
        let base = match parent {
            Some(p) => format!("/mailFolders/{p}/childFolders"),
            None => "/mailFolders".to_string(),
        };
        let filter = format!("displayName eq '{}'", odata_string(name));
        let url = format!(
            "{}?$filter={}&$select={FOLDER_FIELDS}&includeHiddenFolders=true",
            self.path(&base),
            crate::mail::encode_query_value(&filter)
        );
        let v = self.client().get(&url, priority, &[]).await?;
        let items: Vec<GraphFolder> = serde_json::from_value(v["value"].clone())?;
        Ok(items
            .into_iter()
            .find(|f| f.display_name.eq_ignore_ascii_case(name)))
    }

    /// Creates a folder under `parent` (`None`: the top level). If one of
    /// that name is already there (a retry after a lost answer, or a
    /// clash), returns it instead.
    pub async fn create_folder(
        &self,
        parent: Option<&str>,
        name: &str,
        priority: Priority,
    ) -> Result<GraphFolder> {
        let base = match parent {
            Some(p) => format!("/mailFolders/{p}/childFolders"),
            None => "/mailFolders".to_string(),
        };
        match self
            .client()
            .post(&self.path(&base), &json!({ "displayName": name }), priority)
            .await
        {
            Ok(v) => Ok(serde_json::from_value(v)?),
            Err(Error::Graph { status: 409, .. }) => self
                .find_child_folder(parent, name, priority)
                .await?
                .ok_or_else(|| Error::Decode(format!("folder {name:?} exists but can't be found"))),
            Err(e) => Err(e),
        }
    }

    /// Renames a folder.
    pub async fn rename_folder(&self, id: &str, name: &str, priority: Priority) -> Result<()> {
        self.client()
            .patch(
                &self.path(&format!("/mailFolders/{id}")),
                &json!({ "displayName": name }),
                priority,
            )
            .await
            .map(|_| ())
    }

    /// Moves a folder under `dest` (a folder ID or well-known name, or
    /// `msgfolderroot` for the top level).
    pub async fn move_folder(&self, id: &str, dest: &str, priority: Priority) -> Result<()> {
        self.client()
            .post(
                &self.path(&format!("/mailFolders/{id}/move")),
                &json!({ "destinationId": dest }),
                priority,
            )
            .await
            .map(|_| ())
    }

    /// Deletes a folder. Exchange moves it to Deleted Items, or deletes it
    /// for good when it is already there.
    pub async fn delete_folder(&self, id: &str, priority: Priority) -> Result<()> {
        self.client()
            .delete(&self.path(&format!("/mailFolders/{id}")), priority)
            .await
    }

    /// Creates a draft in Drafts from a Graph message JSON body and
    /// returns it.
    pub async fn create_draft(&self, message: &Value, priority: Priority) -> Result<Value> {
        self.client()
            .post(&self.path("/messages"), message, priority)
            .await
    }

    /// Creates a reply (`createReply`), reply-all (`createReplyAll`) or
    /// forward (`createForward`) draft of message `id` and returns it.
    pub async fn create_response(
        &self,
        id: &str,
        action: &str,
        priority: Priority,
    ) -> Result<Value> {
        self.client()
            .post(
                &self.path(&format!("/messages/{id}/{action}")),
                &json!({}),
                priority,
            )
            .await
    }

    /// Changes a draft's fields (a Graph message JSON body).
    pub async fn update_draft(&self, id: &str, fields: &Value, priority: Priority) -> Result<()> {
        self.client()
            .patch(&self.path(&format!("/messages/{id}")), fields, priority)
            .await
            .map(|_| ())
    }

    /// Adds a file attachment (a Graph `fileAttachment` JSON body, up to 3 MB).
    pub async fn add_attachment(
        &self,
        id: &str,
        attachment: &Value,
        priority: Priority,
    ) -> Result<Value> {
        self.client()
            .post(
                &self.path(&format!("/messages/{id}/attachments")),
                attachment,
                priority,
            )
            .await
    }

    /// Sends a draft. Graph moves it to Sent Items.
    pub async fn send_draft(&self, id: &str, priority: Priority) -> Result<()> {
        self.client()
            .post(
                &self.path(&format!("/messages/{id}/send")),
                &json!({}),
                priority,
            )
            .await
            .map(|_| ())
    }

    /// The ID of a message in Sent Items with this Internet message ID, if any.
    pub async fn find_sent(
        &self,
        internet_message_id: &str,
        priority: Priority,
    ) -> Result<Option<String>> {
        let filter = format!(
            "internetMessageId eq '{}'",
            odata_string(internet_message_id)
        );
        let url = format!(
            "{}?$filter={}&$select=id",
            self.path("/mailFolders/sentitems/messages"),
            crate::mail::encode_query_value(&filter)
        );
        let v = self.client().get(&url, priority, &[]).await?;
        Ok(v["value"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|m| m["id"].as_str())
            .map(str::to_string))
    }

    /// A message's Internet message ID.
    pub async fn internet_message_id(
        &self,
        id: &str,
        priority: Priority,
    ) -> Result<Option<String>> {
        let v = self
            .client()
            .get(
                &self.path(&format!("/messages/{id}?$select=internetMessageId")),
                priority,
                &[],
            )
            .await?;
        Ok(v["internetMessageId"].as_str().map(str::to_string))
    }

    /// Any Graph call, for writes the typed methods don't cover.
    pub async fn raw(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        priority: Priority,
    ) -> Result<Value> {
        self.client()
            .request(method, &self.path(path), body, priority, &[])
            .await
    }
}
