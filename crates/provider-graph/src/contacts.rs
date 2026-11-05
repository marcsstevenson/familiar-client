// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contacts read (WP 2.5): the user's Outlook contacts, refreshed in full.
//!
//! Contacts are few (hundreds, not thousands), so each sync lists them all
//! (the default contacts folder, then each contact folder under it) and
//! replaces the store's copy in one round: contacts the server no longer
//! lists are removed.

use familiar_store::{AccountId, Address, NewContact, StoreWriter};
use serde::Deserialize;

use crate::client::GraphClient;
use crate::error::Result;
use crate::model::{Page, decode_page};
use crate::scheduler::Priority;

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

/// A contact as Graph returns it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct GraphContact {
    /// ID.
    pub id: String,
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
                .filter_map(|e| {
                    let (inner_name, addr) = split_mailbox(e.address.as_deref()?);
                    let name = e
                        .name
                        .clone()
                        .filter(|n| !n.trim().is_empty() && !n.contains('<'))
                        .or(inner_name)
                        .unwrap_or_else(|| display.clone());
                    (!addr.is_empty()).then_some(Address { name, addr })
                })
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

/// Contact reads for one mailbox.
#[derive(Clone, Debug)]
pub struct ContactsApi {
    client: GraphClient,
}

impl ContactsApi {
    /// Reads the signed-in user's contacts.
    pub fn new(client: GraphClient) -> Self {
        Self { client }
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
