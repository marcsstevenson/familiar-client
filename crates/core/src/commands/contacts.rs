// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam contacts search` (WP 2.5): Outlook contacts and past correspondents,
//! for address autocomplete.
//!
//! famd keeps each account's Outlook contacts (refreshed hourly) and the
//! store keeps everyone mail came from or went to (from Sent Items). This
//! reads the store directly, so it works without famd.

use familiar_store::{Account, Reader};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::daemon::DaemonPaths;
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};

/// `fam contacts search`.
pub struct ContactsSearch;

/// Input for `fam contacts search`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContactsSearchInput {
    /// The start of a name, address or company (or of any word in a name).
    /// Empty lists the people written to most.
    #[serde(default)]
    #[schemars(extend("x-positional" = 0))]
    pub query: String,
    /// Only this account (its name in config.toml).
    #[serde(default)]
    pub account: Option<String>,
    /// How many to show, 1 to 200. Default 20.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// One person found.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PersonView {
    /// Name, possibly empty.
    pub name: String,
    /// Address.
    pub address: String,
    /// `Name <address>`, ready for --to.
    pub mailbox: String,
    /// In Outlook contacts.
    pub contact: bool,
    /// Company, from the contact.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub company: String,
    /// Accounts that know them.
    pub accounts: Vec<String>,
    /// Messages sent to them.
    pub sent: i64,
    /// Messages received from them.
    pub received: i64,
    /// Last message either way (RFC 3339), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
}

/// Output of `fam contacts search`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContactsSearchOutput {
    /// Best first: contacts, then people written to most, then heard from.
    pub people: Vec<PersonView>,
}

fn store_err(e: familiar_store::Error) -> CommandError {
    CommandError::internal(format!("store: {e}"))
}

/// The `Name <addr>` form, quoting a name that needs it.
pub fn mailbox(name: &str, addr: &str) -> String {
    if name.is_empty() {
        return addr.to_string();
    }
    if name.contains([',', ';', '<', '>', '"']) {
        format!("\"{}\" <{addr}>", name.replace('"', "'"))
    } else {
        format!("{name} <{addr}>")
    }
}

/// Searches the store at `paths`, as `fam contacts search` does.
pub fn search_at(
    paths: &DaemonPaths,
    input: &ContactsSearchInput,
) -> Result<ContactsSearchOutput, CommandError> {
    let limit = input.limit.unwrap_or(20);
    if !(1..=200).contains(&limit) {
        return Err(CommandError::usage("--limit must be between 1 and 200"));
    }
    let r = match Reader::open(&paths.store) {
        Ok(r) => r,
        Err(familiar_store::Error::NoStore(_)) => {
            return Err(CommandError::new(
                ErrorKind::NotFound,
                "nothing has synced on this device yet; start famd first",
            ));
        }
        Err(e) => return Err(store_err(e)),
    };
    let all = r.accounts().map_err(store_err)?;
    let picked: Vec<&Account> = all
        .iter()
        .filter(|a| input.account.as_deref().is_none_or(|k| a.key == k))
        .collect();
    if let (Some(k), true) = (&input.account, picked.is_empty()) {
        return Err(CommandError::new(
            ErrorKind::NotFound,
            format!("account {k:?} has not synced on this device"),
        ));
    }
    let ids: Vec<_> = picked.iter().map(|a| a.id).collect();
    let found = r
        .people(&ids, &input.query, limit as usize)
        .map_err(store_err)?;
    let key = |id| {
        all.iter()
            .find(|a| a.id == id)
            .map(|a| a.key.clone())
            .unwrap_or_default()
    };
    Ok(ContactsSearchOutput {
        people: found
            .into_iter()
            .map(|p| PersonView {
                mailbox: mailbox(&p.name, &p.addr),
                name: p.name,
                address: p.addr,
                contact: p.contact.is_some(),
                company: p.company,
                accounts: p.accounts.into_iter().map(key).collect(),
                sent: p.sent,
                received: p.received,
                last: (p.last_at > 0).then(|| crate::commands::auth::rfc3339(p.last_at as u64)),
            })
            .collect(),
    })
}

impl Command for ContactsSearch {
    type Input = ContactsSearchInput;
    type Output = ContactsSearchOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["contacts", "search"],
        summary: "Find people to write to",
        description: "Searches Outlook contacts and everyone mail came from or went to, by the start of a name, address or company, best first: contacts, then people written to most, then heard from. Reads the store, so it works without famd. Each result's mailbox field is ready for --to.",
        access: Access::Read,
        errors: &[ErrorKind::Usage, ErrorKind::NotFound],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: ContactsSearchInput,
    ) -> Result<ContactsSearchOutput, CommandError> {
        search_at(&DaemonPaths::resolve()?, &input)
    }

    fn render_text(&self, o: &ContactsSearchOutput) -> String {
        if o.people.is_empty() {
            return "nobody found\n".into();
        }
        let mut out = String::new();
        for p in &o.people {
            let mut notes = Vec::new();
            if p.contact {
                notes.push(if p.company.is_empty() {
                    "contact".to_string()
                } else {
                    format!("contact, {}", p.company)
                });
            }
            if p.sent > 0 {
                notes.push(format!("sent {}", p.sent));
            }
            if p.received > 0 {
                notes.push(format!("received {}", p.received));
            }
            out.push_str(&p.mailbox);
            if !notes.is_empty() {
                out.push_str(&format!("  ({})", notes.join(", ")));
            }
            out.push('\n');
        }
        out
    }
}
