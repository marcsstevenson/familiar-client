// SPDX-License-Identifier: MIT OR Apache-2.0
//! Google contacts (WP G.9): the account's saved contacts from the People
//! API, refreshed in full like Outlook contacts. They are few (hundreds), so
//! each sync lists them all and replaces the store's copy in one round.
//!
//! Gmail's automatic "other contacts" aren't read: they are people the
//! account has mailed, which the store's correspondents list already has.

use familiar_store::{AccountId, Address, NewContact, StoreWriter};
use serde::Deserialize;

use crate::{GmailApi, Priority, Result, api::enc};

/// The fields asked for.
const CONNECTIONS: &str = "/people/me/connections?personFields=names,emailAddresses,organizations,phoneNumbers&pageSize=1000";

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Page {
    connections: Vec<Person>,
    next_page_token: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Person {
    resource_name: String,
    names: Vec<Name>,
    email_addresses: Vec<Value>,
    organizations: Vec<Organization>,
    phone_numbers: Vec<Value>,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Name {
    display_name: String,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Value {
    value: String,
    display_name: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Organization {
    name: String,
}

impl Person {
    fn to_new(&self, account: AccountId) -> NewContact {
        let display = self
            .names
            .first()
            .map(|n| n.display_name.clone())
            .unwrap_or_default();
        NewContact {
            account,
            remote_id: self.resource_name.clone(),
            display_name: display.clone(),
            company: self
                .organizations
                .first()
                .map(|o| o.name.clone())
                .unwrap_or_default(),
            emails: self
                .email_addresses
                .iter()
                .filter(|e| !e.value.trim().is_empty())
                .map(|e| Address {
                    name: if e.display_name.trim().is_empty() {
                        display.clone()
                    } else {
                        e.display_name.clone()
                    },
                    addr: e.value.trim().to_ascii_lowercase(),
                })
                .collect(),
            phones: self
                .phone_numbers
                .iter()
                .map(|p| p.value.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
        }
    }
}

/// Keeps one Gmail account's contacts in the store.
#[derive(Clone, Debug)]
pub struct GoogleContactSync {
    api: GmailApi,
    account: AccountId,
}

impl GoogleContactSync {
    /// Syncs `account`'s contacts through `api`.
    pub fn new(api: GmailApi, account: AccountId) -> Self {
        Self { api, account }
    }

    /// Lists every saved contact and replaces the store's copy. Returns
    /// (kept, removed). Nothing changes unless every page was read.
    pub async fn sync<W: StoreWriter>(
        &self,
        w: &mut W,
        priority: Priority,
    ) -> Result<(usize, usize)> {
        let mut all = Vec::new();
        let mut path = CONNECTIONS.to_string();
        loop {
            let v = self
                .api
                .client()
                .people_request(reqwest::Method::GET, &path, None, priority)
                .await?;
            let page: Page = serde_json::from_value(v)?;
            all.extend(page.connections);
            match page.next_page_token {
                Some(t) => path = format!("{CONNECTIONS}&pageToken={}", enc(&t)),
                None => break,
            }
        }
        let acc = self.account;
        let removed = w.write(|tx| {
            let round = tx.begin_contact_round(acc)?;
            for p in &all {
                tx.upsert_contact(&p.to_new(acc), round)?;
            }
            tx.finish_contact_round(acc, round)
        })?;
        Ok((all.len(), removed))
    }
}
