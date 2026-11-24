// SPDX-License-Identifier: MIT OR Apache-2.0
//! The user's Outlook contacts, refreshed in full and writable by ID.
//!
//! Contacts are few (hundreds, not thousands), so each sync lists them all
//! (the default contacts folder, then each contact folder under it) and
//! replaces the store's copy in one round: contacts the server no longer
//! lists are removed.

use familiar_store::{AccountId, Address, ContactChanges, NewContact, StoreWriter};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::GraphClient;
use crate::mail::encode_query_value;
use crate::model::{Page, decode_page};
use familiar_provider::Priority;
use familiar_provider::Result;

/// Fields asked for.
pub const CONTACT_FIELDS: &str =
    "id,displayName,companyName,emailAddresses,mobilePhone,businessPhones,homePhones";

/// An email address on a contact.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GraphContactEmail {
    /// Name shown for it.
    pub name: Option<String>,
    /// The address.
    pub address: Option<String>,
}

impl GraphContactEmail {
    fn to_address(&self, display: &str) -> Option<Address> {
        let (inner_name, addr) = split_mailbox(self.address.as_deref()?);
        let name = self
            .name
            .clone()
            .filter(|n| !n.trim().is_empty() && !n.contains('<'))
            .or(inner_name)
            .unwrap_or_else(|| display.to_owned());
        (!addr.is_empty()).then_some(Address { name, addr })
    }
}

/// A contact as Graph returns it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct GraphContact {
    /// ID.
    pub id: String,
    /// Folder containing the contact.
    pub parent_folder_id: Option<String>,
    /// Display name.
    pub display_name: Option<String>,
    /// Company.
    pub company_name: Option<String>,
    /// Addresses.
    pub email_addresses: Vec<GraphContactEmail>,
    /// Mobile.
    pub mobile_phone: Option<String>,
    /// Work phones.
    pub business_phones: Vec<String>,
    /// Home phones.
    pub home_phones: Vec<String>,
}

impl GraphContact {
    fn path(&self) -> String {
        let id = encode_query_value(&self.id);
        match &self.parent_folder_id {
            Some(folder) => format!(
                "/me/contactFolders/{}/contacts/{id}",
                encode_query_value(folder)
            ),
            None => format!("/me/contacts/{id}"),
        }
    }

    fn write_body(&self, fields: &ContactChanges) -> Value {
        let mut body = json!({});
        // Graph can regenerate displayName on unrelated updates unless it is supplied.
        if let Some(name) = fields.name.as_ref().or(self.display_name.as_ref()) {
            body["displayName"] = json!(name);
        }
        if let Some(company) = &fields.company {
            body["companyName"] = json!(company);
        }
        if let Some(emails) = &fields.emails {
            let existing: Vec<_> = self
                .email_addresses
                .iter()
                .filter_map(|e| e.to_address(self.display_name.as_deref().unwrap_or_default()))
                .collect();
            let addresses: Vec<_> = emails
                .iter()
                .map(|address| {
                    let mut value = json!({"address": address});
                    if let Some(email) = existing
                        .iter()
                        .find(|e| e.addr.eq_ignore_ascii_case(address))
                    {
                        value["name"] = json!(email.name);
                    }
                    value
                })
                .collect();
            body["emailAddresses"] = json!(addresses);
        }
        if let Some(phones) = &fields.phones {
            // Familiar exposes one list. Keep the Outlook type of surviving numbers.
            let mobile = self.mobile_phone.as_ref().filter(|p| phones.contains(p));
            let (home, business): (Vec<_>, Vec<_>) = phones
                .iter()
                .filter(|p| Some(*p) != mobile)
                .partition(|p| self.home_phones.contains(p));
            body["mobilePhone"] = json!(mobile);
            body["homePhones"] = json!(home);
            body["businessPhones"] = json!(business);
        }
        body
    }

    /// The store row.
    pub fn to_new(&self, account: AccountId) -> NewContact {
        let display = self.display_name.clone().unwrap_or_default();
        NewContact {
            account,
            remote_id: self.id.clone(),
            display_name: display.clone(),
            company: self.company_name.clone().unwrap_or_default(),
            emails: self
                .email_addresses
                .iter()
                .filter_map(|e| e.to_address(&display))
                .collect(),
            phones: self
                .mobile_phone
                .iter()
                .chain(&self.business_phones)
                .chain(&self.home_phones)
                .filter(|p| !p.trim().is_empty())
                .cloned()
                .collect(),
        }
    }
}

/// Outlook sometimes keeps a whole mailbox (`"Name" <addr>`) in a
/// contact's address field. Returns the name in it, if any, and the bare
/// address, lowercased.
fn split_mailbox(s: &str) -> (Option<String>, String) {
    let s = s.trim();
    if let (Some(open), true) = (s.rfind('<'), s.ends_with('>')) {
        let addr = s[open + 1..s.len() - 1].trim().to_lowercase();
        let name = s[..open].trim().trim_matches('"').trim();
        return ((!name.is_empty()).then(|| name.to_string()), addr);
    }
    (None, s.trim_matches('"').to_lowercase())
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct GraphContactFolder {
    id: String,
}

/// Contact reads and writes for one mailbox.
#[derive(Clone, Debug)]
pub struct ContactsApi {
    client: GraphClient,
}

impl ContactsApi {
    /// Reads the signed-in user's contacts.
    pub fn new(client: GraphClient) -> Self {
        Self { client }
    }

    async fn get_contact(&self, remote_id: &str) -> Result<GraphContact> {
        let path = format!(
            "/me/contacts/{}?$select={CONTACT_FIELDS},parentFolderId",
            encode_query_value(remote_id)
        );
        Ok(serde_json::from_value(
            self.client.get(&path, Priority::User, &[]).await?,
        )?)
    }

    /// Creates a contact or updates the supplied fields on an existing contact.
    pub async fn save_contact(
        &self,
        account: AccountId,
        remote_id: Option<&str>,
        fields: &ContactChanges,
    ) -> Result<NewContact> {
        let saved = if let Some(id) = remote_id {
            let contact = self.get_contact(id).await?;
            self.client
                .patch(&contact.path(), &contact.write_body(fields), Priority::User)
                .await?
        } else {
            self.client
                .post(
                    "/me/contacts",
                    &GraphContact::default().write_body(fields),
                    Priority::User,
                )
                .await?
        };
        Ok(serde_json::from_value::<GraphContact>(saved)?.to_new(account))
    }

    /// Deletes a contact from its Outlook folder.
    pub async fn delete_contact(&self, remote_id: &str) -> Result<()> {
        let contact = self.get_contact(remote_id).await?;
        self.client.delete(&contact.path(), Priority::User).await
    }

    async fn pages<T: serde::de::DeserializeOwned>(
        &self,
        first: String,
        priority: Priority,
    ) -> Result<Vec<T>> {
        let mut out = Vec::new();
        let mut url = first;
        loop {
            let page: Page<T> = decode_page(self.client.get(&url, priority, &[]).await?)?;
            out.extend(page.items);
            match page.next_link {
                Some(n) => url = n,
                None => return Ok(out),
            }
        }
    }

    /// Every contact: the default folder's, then each contact folder's.
    pub async fn all(&self, priority: Priority) -> Result<Vec<GraphContact>> {
        let mut out: Vec<GraphContact> = self
            .pages(
                format!("/me/contacts?$top=500&$select={CONTACT_FIELDS}"),
                priority,
            )
            .await?;
        let folders: Vec<GraphContactFolder> = self
            .pages("/me/contactFolders?$top=100".into(), priority)
            .await?;
        for f in folders {
            let more: Vec<GraphContact> = self
                .pages(
                    format!(
                        "/me/contactFolders/{}/contacts?$top=500&$select={CONTACT_FIELDS}",
                        f.id
                    ),
                    priority,
                )
                .await?;
            out.extend(more);
        }
        Ok(out)
    }
}

/// Keeps one account's contacts in the store.
#[derive(Clone, Debug)]
pub struct ContactSync {
    api: ContactsApi,
    account: AccountId,
}

impl ContactSync {
    /// Syncs `account`'s contacts through `api`.
    pub fn new(api: ContactsApi, account: AccountId) -> Self {
        Self { api, account }
    }

    /// Lists every contact and replaces the store's copy. Returns (kept, removed).
    pub async fn sync<W: StoreWriter>(
        &self,
        w: &mut W,
        priority: Priority,
    ) -> Result<(usize, usize)> {
        let all = self.api.all(priority).await?;
        let acc = self.account;
        let removed = w.write(|tx| {
            let round = tx.begin_contact_round(acc)?;
            for c in &all {
                tx.upsert_contact(&c.to_new(acc), round)?;
            }
            tx.finish_contact_round(acc, round)
        })?;
        Ok((all.len(), removed))
    }
}

#[cfg(test)]
mod tests {
    use super::split_mailbox;

    #[test]
    fn mailbox_in_the_address_field() {
        assert_eq!(
            split_mailbox("\"michele stevenson\" <Michele.Stevenson@ecan.govt.nz>"),
            (
                Some("michele stevenson".into()),
                "michele.stevenson@ecan.govt.nz".into()
            )
        );
        assert_eq!(
            split_mailbox(" Bob@Example.com "),
            (None, "bob@example.com".into())
        );
    }
}
