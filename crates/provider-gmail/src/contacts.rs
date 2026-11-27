// SPDX-License-Identifier: MIT OR Apache-2.0
//! Google contacts (WP G.9): the account's saved contacts from the People
//! API, refreshed in full like Outlook contacts. They are few (hundreds), so
//! each sync lists them all and replaces the store's copy in one round.
//!
//! Gmail's automatic "other contacts" aren't read: they are people the
//! account has mailed, which the store's correspondents list already has.

use familiar_store::{AccountId, Address, ContactChanges, NewContact, StoreWriter};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Map, Value as Json, json};

use crate::{Error, GmailApi, Priority, Result, api::enc};

const CONTACT_FIELDS: &str =
    "personFields=names,emailAddresses,organizations,phoneNumbers,memberships";
const STARRED: &str = "contactGroups/starred";

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
    memberships: Vec<Json>,
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
            starred: self.memberships.iter().any(is_starred),
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

fn person_path(remote: &str) -> Result<String> {
    let id = remote
        .strip_prefix("people/")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::Decode(format!("invalid People resource name: {remote}")))?;
    Ok(format!("/people/{}", enc(id)))
}

fn is_starred(membership: &Json) -> bool {
    membership["contactGroupMembership"]["contactGroupResourceName"] == STARRED
}

fn contact_fields(fields: &ContactChanges, current: &Json) -> Map<String, Json> {
    let mut body = Map::new();
    if let Some(starred) = fields.starred {
        let mut groups = current["memberships"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if starred != groups.iter().any(is_starred) {
            groups.retain(|group| {
                !is_starred(group) && group.get("contactGroupMembership").is_some()
            });
            if groups.is_empty() {
                groups.push(json!({"contactGroupMembership":{"contactGroupResourceName":"contactGroups/myContacts"}}));
            }
            if starred {
                groups.push(json!({"contactGroupMembership":{"contactGroupResourceName":STARRED}}));
            }
            body.insert("memberships".into(), Json::Array(groups));
        }
    }
    if let Some(name) = &fields.name
        && current["names"][0]["displayName"].as_str() != Some(name)
    {
        let names = if name.is_empty() {
            json!([])
        } else {
            json!([{"unstructuredName":name}])
        };
        body.insert("names".into(), names);
    }
    if let Some(company) = &fields.company {
        let mut orgs = current["organizations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if let Some(first) = orgs.first_mut() {
            first["name"] = json!(company);
            if company.is_empty()
                && first.as_object().is_some_and(|org| {
                    org.keys()
                        .all(|k| matches!(k.as_str(), "name" | "metadata"))
                })
            {
                orgs.remove(0);
            }
        } else if !company.is_empty() {
            orgs.push(json!({"name":company}));
        }
        body.insert("organizations".into(), Json::Array(orgs));
    }
    for (key, values) in [
        ("emailAddresses", &fields.emails),
        ("phoneNumbers", &fields.phones),
    ] {
        if let Some(values) = values {
            // A field mask replaces the entire list. Retain labels and other
            // details attached to an address or number the user kept.
            let existing = current[key]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default();
            let entries = values
                .iter()
                .map(|value| {
                    existing
                        .iter()
                        .find(|e| {
                            e["value"]
                                .as_str()
                                .is_some_and(|s| s.trim().eq_ignore_ascii_case(value))
                        })
                        .cloned()
                        .unwrap_or_else(|| json!({"value":value}))
                })
                .collect();
            body.insert(key.into(), Json::Array(entries));
        }
    }
    body
}

impl GmailApi {
    async fn contact_json(&self, path: &str) -> Result<Json> {
        self.client()
            .people_request(
                Method::GET,
                // Google may also return profile data, which belongs to
                // another source and must not be copied into the contact.
                &format!("{path}?{CONTACT_FIELDS},metadata&sources=READ_SOURCE_TYPE_CONTACT"),
                None,
                Priority::User,
            )
            .await
    }

    /// Reads the current contact fields before a reviewed merge step.
    pub async fn get_contact(&self, account: AccountId, remote: &str) -> Result<NewContact> {
        let current = self.contact_json(&person_path(remote)?).await?;
        Ok(serde_json::from_value::<Person>(current)?.to_new(account))
    }

    /// Creates or patches one saved contact, returning the server's copy.
    pub async fn save_contact(
        &self,
        account: AccountId,
        remote_id: Option<&str>,
        fields: &ContactChanges,
    ) -> Result<NewContact> {
        let (method, path, body) = if let Some(remote) = remote_id {
            let path = person_path(remote)?;
            let current = self.contact_json(&path).await?;
            let mut changes = contact_fields(fields, &current);
            if changes.is_empty() {
                return Ok(serde_json::from_value::<Person>(current)?.to_new(account));
            }
            let mask = changes.keys().cloned().collect::<Vec<_>>().join(",");
            changes.insert("metadata".into(), current["metadata"].clone());
            (
                Method::PATCH,
                format!("{path}:updateContact?updatePersonFields={mask}&{CONTACT_FIELDS}"),
                changes,
            )
        } else {
            (
                Method::POST,
                format!("/people:createContact?{CONTACT_FIELDS}"),
                contact_fields(fields, &Json::Null),
            )
        };
        let result = self
            .client()
            .people_request(method, &path, Some(&Json::Object(body)), Priority::User)
            .await?;
        Ok(serde_json::from_value::<Person>(result)?.to_new(account))
    }

    /// Deletes one saved contact from this mailbox.
    pub async fn delete_contact(&self, remote_id: &str) -> Result<()> {
        let path = format!("{}:deleteContact", person_path(remote_id)?);
        self.client()
            .people_request(Method::DELETE, &path, None, Priority::User)
            .await?;
        Ok(())
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
        let connections = format!(
            "/people/me/connections?{CONTACT_FIELDS}&sources=READ_SOURCE_TYPE_CONTACT&pageSize=1000"
        );
        let mut path = connections.clone();
        loop {
            let v = self
                .api
                .client()
                .people_request(reqwest::Method::GET, &path, None, priority)
                .await?;
            let page: Page = serde_json::from_value(v)?;
            all.extend(page.connections);
            match page.next_page_token {
                Some(t) => path = format!("{connections}&pageToken={}", enc(&t)),
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
