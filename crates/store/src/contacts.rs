// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contacts and past correspondents (WP 2.5).
//!
//! Outlook contacts are refreshed in full rounds: every contact the server
//! lists is stamped with the round number, and the rest are removed. Past
//! correspondents are kept by triggers on `messages` (schema version 6):
//! everyone mail came from, outside Sent Items and Drafts, and everyone
//! mail in Sent Items went to. [`Reader::people`] searches both.

use std::collections::HashMap;

use crate::model::{AccountId, Address, ContactId};
use crate::writer::WriteTx;
use crate::{Reader, Result};
use rusqlite::{Connection, params};
use serde::Serialize;

/// A contact to add or update, matched by account and `remote_id`.
#[derive(Debug, Clone, Default)]
pub struct NewContact {
    /// Owning account.
    pub account: AccountId,
    /// Server ID.
    pub remote_id: String,
    /// Display name.
    pub display_name: String,
    /// Company.
    pub company: String,
    /// Email addresses (with the name Outlook shows for each).
    pub emails: Vec<Address>,
    /// Phone numbers.
    pub phones: Vec<String>,
}

/// A stored contact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Contact {
    /// Local ID.
    pub id: ContactId,
    /// Owning account.
    pub account: AccountId,
    /// Server ID.
    pub remote_id: String,
    /// Display name.
    pub display_name: String,
    /// Company.
    pub company: String,
    /// Email addresses.
    pub emails: Vec<Address>,
    /// Phone numbers.
    pub phones: Vec<String>,
}

/// Someone to write to: a contact's address or a past correspondent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Person {
    /// Name.
    pub name: String,
    /// Address (lower case).
    pub addr: String,
    /// The contact it comes from, if it is one.
    pub contact: Option<ContactId>,
    /// Company, from the contact.
    pub company: String,
    /// Accounts that know them.
    pub accounts: Vec<AccountId>,
    /// Messages received from them.
    pub received: i64,
    /// Messages sent to them.
    pub sent: i64,
    /// Last message either way (Unix seconds), 0 if none.
    pub last_at: i64,
}

impl Person {
    /// How likely they are the one meant: contacts first, then people
    /// written to, then people heard from, then recent.
    pub fn score(&self) -> i64 {
        i64::from(self.contact.is_some()) * 1_000_000 + self.sent * 20 + self.received
    }
}

fn json<T: serde::de::DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_str(s).unwrap_or_default()
}

fn like_escape(q: &str) -> String {
    q.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

impl WriteTx<'_> {
    /// Starts a contacts round for an account and returns its number.
    pub fn begin_contact_round(&self, account: AccountId) -> Result<i64> {
        Ok(self.tx.query_row(
            "SELECT coalesce(max(round), 0) + 1 FROM contacts WHERE account_id = ?1",
            [account],
            |r| r.get(0),
        )?)
    }

    /// Adds or updates a contact, stamped with `round`.
    pub fn upsert_contact(&self, c: &NewContact, round: i64) -> Result<ContactId> {
        let emails = serde_json::to_string(&c.emails).expect("emails serialise");
        let phones = serde_json::to_string(&c.phones).expect("phones serialise");
        let id: ContactId = self.tx.query_row(
            "INSERT INTO contacts (account_id, remote_id, display_name, company, emails_json,
                                   phones_json, round)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (account_id, remote_id) DO UPDATE SET
               display_name = excluded.display_name, company = excluded.company,
               emails_json = excluded.emails_json, phones_json = excluded.phones_json,
               round = excluded.round
             RETURNING id",
            params![
                c.account,
                c.remote_id,
                c.display_name,
                c.company,
                emails,
                phones,
                round
            ],
            |r| r.get(0),
        )?;
        self.tx
            .execute("DELETE FROM contact_addresses WHERE contact_id = ?1", [id])?;
        let mut s = self
            .tx
            .prepare_cached("INSERT INTO contact_addresses (contact_id, addr) VALUES (?1, ?2)")?;
        for e in &c.emails {
            if !e.addr.trim().is_empty() {
                s.execute(params![id, e.addr.trim().to_lowercase()])?;
            }
        }
        Ok(id)
    }

    /// Ends a round: removes the account's contacts it did not see.
    pub fn finish_contact_round(&self, account: AccountId, round: i64) -> Result<usize> {
        Ok(self.tx.execute(
            "DELETE FROM contacts WHERE account_id = ?1 AND round <> ?2",
            params![account, round],
        )?)
    }
}

fn contacts_of(c: &Connection, account: AccountId) -> Result<Vec<Contact>> {
    let mut s = c.prepare_cached(
        "SELECT id, account_id, remote_id, display_name, company, emails_json, phones_json
           FROM contacts WHERE account_id = ?1 ORDER BY display_name COLLATE NOCASE, id",
    )?;
    let rows = s.query_map([account], |r| {
        Ok(Contact {
            id: r.get(0)?,
            account: r.get(1)?,
            remote_id: r.get(2)?,
            display_name: r.get(3)?,
            company: r.get(4)?,
            emails: json(&r.get::<_, String>(5)?),
            phones: json(&r.get::<_, String>(6)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn people(
    c: &Connection,
    accounts: &[AccountId],
    query: &str,
    limit: usize,
) -> Result<Vec<Person>> {
    let q = like_escape(query.trim().to_lowercase().as_str());
    let prefix = format!("{q}%");
    let word = format!("% {q}%");
    let mut by_addr: HashMap<String, Person> = HashMap::new();
    let mut contacts = c.prepare_cached(
        "SELECT c.id, c.display_name, c.company, ca.addr, c.emails_json
           FROM contacts c JOIN contact_addresses ca ON ca.contact_id = c.id
          WHERE c.account_id = ?1
            AND (c.display_name LIKE ?2 ESCAPE '\\' OR c.display_name LIKE ?3 ESCAPE '\\'
                 OR ca.addr LIKE ?2 ESCAPE '\\' OR c.company LIKE ?2 ESCAPE '\\')",
    )?;
    let mut corr = c.prepare_cached(
        "SELECT addr, name, received, sent, last_at FROM correspondents
          WHERE account_id = ?1
            AND (addr LIKE ?2 ESCAPE '\\' OR name LIKE ?2 ESCAPE '\\' OR name LIKE ?3 ESCAPE '\\')
          ORDER BY sent DESC, received DESC, last_at DESC
          LIMIT ?4",
    )?;
    let cap = (limit * 4).max(50) as i64;
    for &acc in accounts {
        let rows = contacts.query_map(params![acc, prefix, word], |r| {
            Ok((
                r.get::<_, ContactId>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (id, display, company, addr, emails) = row?;
            let emails: Vec<Address> = json(&emails);
            let name = emails
                .iter()
                .find(|e| e.addr.eq_ignore_ascii_case(&addr))
                .map(|e| e.name.clone())
                .filter(|n| !n.is_empty() && !n.contains('@'))
                .unwrap_or(display);
            let p = by_addr.entry(addr.clone()).or_insert_with(|| Person {
                name: name.clone(),
                addr: addr.clone(),
                contact: None,
                company: String::new(),
                accounts: Vec::new(),
                received: 0,
                sent: 0,
                last_at: 0,
            });
            p.contact.get_or_insert(id);
            p.name = name;
            if p.company.is_empty() {
                p.company = company;
            }
            if !p.accounts.contains(&acc) {
                p.accounts.push(acc);
            }
        }
        let rows = corr.query_map(params![acc, prefix, word, cap], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        for row in rows {
            let (addr, name, received, sent, last_at) = row?;
            let p = by_addr.entry(addr.clone()).or_insert_with(|| Person {
                name: String::new(),
                addr: addr.clone(),
                contact: None,
                company: String::new(),
                accounts: Vec::new(),
                received: 0,
                sent: 0,
                last_at: 0,
            });
            if p.name.is_empty() && !name.contains('@') {
                p.name = name;
            }
            p.received += received;
            p.sent += sent;
            p.last_at = p.last_at.max(last_at);
            if !p.accounts.contains(&acc) {
                p.accounts.push(acc);
            }
        }
    }
    let mut out: Vec<Person> = by_addr.into_values().collect();
    out.sort_by(|a, b| {
        b.score()
            .cmp(&a.score())
            .then(b.last_at.cmp(&a.last_at))
            .then(a.addr.cmp(&b.addr))
    });
    out.truncate(limit);
    Ok(out)
}

impl Reader {
    /// An account's contacts, by name.
    pub fn contacts(&self, account: AccountId) -> Result<Vec<Contact>> {
        contacts_of(&self.conn, account)
    }

    /// Contacts and past correspondents of `accounts` whose name, address
    /// or company starts with `query` (or has a word that does), best
    /// first: contacts, then people written to most, then heard from. An
    /// empty query gives the most-written-to.
    pub fn people(&self, accounts: &[AccountId], query: &str, limit: usize) -> Result<Vec<Person>> {
        people(&self.conn, accounts, query, limit)
    }
}
