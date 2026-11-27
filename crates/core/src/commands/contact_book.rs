// SPDX-License-Identifier: MIT OR Apache-2.0
//! Saved contact cards. Browsing reads the local store. Mutations return after
//! the named account's provider has accepted the change.

use familiar_store::ContactChanges;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::daemon::{Client, DaemonPaths};
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};

/// List saved contact records.
pub struct ContactsList;
/// Create a saved contact.
pub struct ContactsCreate;
/// Change a saved contact.
pub struct ContactsEdit;
/// Delete a saved contact.
pub struct ContactsDelete;

/// Inputs for listing the local address book.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContactsListInput {}

/// One independently editable record.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ContactCard {
    /// Local record ID.
    pub id: i64,
    /// Provider record ID.
    pub remote_id: String,
    /// Owning account key.
    pub account: String,
    /// Display name.
    pub name: String,
    /// Organization name.
    pub company: String,
    /// All saved email addresses.
    pub emails: Vec<String>,
    /// All saved phone numbers.
    pub phones: Vec<String>,
    /// Google account email for opening this card on the web.
    pub google_account: Option<String>,
    /// Google favourite status.
    pub starred: bool,
}

/// All synced accounts and their saved contacts.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ContactBook {
    /// Available synced account keys, including empty address books.
    pub accounts: Vec<String>,
    /// Saved contact records.
    pub contacts: Vec<ContactCard>,
}

/// Lists saved contacts from the store at `paths`.
pub fn list_at(paths: &DaemonPaths) -> Result<ContactBook, CommandError> {
    let r = super::cal_write::open(paths)?;
    let accounts = r.accounts().map_err(store_error)?;
    let mut contacts = Vec::new();
    for account in &accounts {
        for c in r.contacts(account.id).map_err(store_error)? {
            contacts.push(ContactCard {
                id: c.id.0,
                remote_id: c.remote_id,
                account: account.key.clone(),
                name: c.display_name,
                company: c.company,
                phones: c.phones,
                google_account: (account.provider == "gmail").then(|| account.email.clone()),
                starred: c.starred,
                emails: c.emails.into_iter().map(|e| e.addr).collect(),
            });
        }
    }
    Ok(ContactBook {
        accounts: accounts.into_iter().map(|a| a.key).collect(),
        contacts,
    })
}

fn store_error(err: familiar_store::Error) -> CommandError {
    CommandError::internal(format!("store: {err}"))
}

impl Command for ContactsList {
    type Input = ContactsListInput;
    type Output = ContactBook;
    const SPEC: CommandSpec = CommandSpec {
        path: &["contacts", "list"],
        summary: "List saved contacts from all accounts",
        description: "Reads each saved Outlook or Google contact from the local store with its owning account, email addresses and phones.",
        access: Access::Read,
        errors: &[ErrorKind::NotFound],
    };
    fn run(&self, _: &Context<'_>, _: Self::Input) -> Result<ContactBook, CommandError> {
        list_at(&DaemonPaths::resolve()?)
    }
    fn render_text(&self, out: &ContactBook) -> String {
        out.contacts
            .iter()
            .map(|c| {
                format!(
                    "{}\t{}\t{}\t{}\t{}\n",
                    c.id,
                    c.account,
                    c.name,
                    c.emails.join(", "),
                    c.phones.join(", ")
                )
            })
            .collect()
    }
}

/// The destination account and fields for a new record.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContactCreateInput {
    /// Account in which to create the contact.
    pub account: String,
    #[serde(flatten)]
    /// Contact fields supplied by the caller.
    pub fields: ContactChanges,
}

/// An existing contact in one account.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ContactTarget {
    /// Account that owns the saved record.
    pub account: String,
    /// Local contact ID from contacts list.
    #[schemars(extend("x-positional" = 0))]
    pub contact: i64,
    /// Provider ID from the selected card, to detect a stale selection.
    pub remote_id: Option<String>,
    /// JSON snapshot of the reviewed name, company, emails, phones and starred flag.
    pub expected_json: Option<String>,
}

/// Selected record and changed fields.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContactEditInput {
    #[serde(flatten)]
    /// The contact to change.
    pub target: ContactTarget,
    #[serde(flatten)]
    /// Contact fields supplied by the caller.
    pub fields: ContactChanges,
}

/// Selected record and deletion confirmation.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContactDeleteInput {
    #[serde(flatten)]
    /// The contact to change.
    pub target: ContactTarget,
    /// Confirm deletion from the provider account.
    #[serde(default)]
    pub yes: bool,
}

/// The local ID of the affected contact.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ContactWritten {
    /// Local ID of the affected contact.
    pub contact: i64,
}

/// One change to the provider address book.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContactMutation {
    /// Create a new record.
    Create(ContactCreateInput),
    /// Modify an existing record.
    Edit(ContactEditInput),
    /// Remove a record.
    Delete(ContactDeleteInput),
}

impl ContactMutation {
    /// The account whose permission and address book apply.
    pub fn account(&self) -> &str {
        match self {
            Self::Create(i) => &i.account,
            Self::Edit(i) => &i.target.account,
            Self::Delete(i) => &i.target.account,
        }
    }
}

/// A sourced request to change the address book.
#[derive(Debug, Serialize, Deserialize)]
pub struct ContactWrite {
    /// Identity used for grants and auditing.
    pub source: String,
    /// Requested change.
    pub mutation: ContactMutation,
}

/// Daemon RPC for a provider-confirmed contact mutation.
pub const METHOD_CONTACT_WRITE: &str = "contacts.write";

fn submit(mutation: ContactMutation) -> Result<ContactWritten, CommandError> {
    Client::connect(&DaemonPaths::resolve()?.socket)?.call(
        METHOD_CONTACT_WRITE,
        &ContactWrite {
            source: crate::agent::session_source(),
            mutation,
        },
    )
}

const ERRORS: &[ErrorKind] = &[
    ErrorKind::Usage,
    ErrorKind::NotFound,
    ErrorKind::AuthRequired,
    ErrorKind::PermissionDenied,
    ErrorKind::ConfirmationRequired,
    ErrorKind::Conflict,
    ErrorKind::DaemonUnavailable,
    ErrorKind::Offline,
    ErrorKind::Throttled,
    ErrorKind::Server,
];

impl Command for ContactsCreate {
    type Input = ContactCreateInput;
    type Output = ContactWritten;
    const SPEC: CommandSpec = CommandSpec {
        path: &["contacts", "create"],
        summary: "Create a contact in one account",
        description: "Saves a contact in the specified Outlook or Google account. Requires a connection to the provider.",
        access: Access::Write,
        errors: ERRORS,
    };
    fn run(&self, _: &Context<'_>, input: Self::Input) -> Result<Self::Output, CommandError> {
        submit(ContactMutation::Create(input))
    }
}

impl Command for ContactsEdit {
    type Input = ContactEditInput;
    type Output = ContactWritten;
    const SPEC: CommandSpec = CommandSpec {
        path: &["contacts", "edit"],
        summary: "Edit a saved contact",
        description: "Updates the supplied fields on one contact in its owning account. Omit fields to keep them.",
        access: Access::Write,
        errors: ERRORS,
    };
    fn run(&self, _: &Context<'_>, input: Self::Input) -> Result<Self::Output, CommandError> {
        submit(ContactMutation::Edit(input))
    }
}

impl Command for ContactsDelete {
    type Input = ContactDeleteInput;
    type Output = ContactWritten;
    const SPEC: CommandSpec = CommandSpec {
        path: &["contacts", "delete"],
        summary: "Delete a saved contact",
        description: "Deletes one contact from its owning Outlook or Google account. Requires --yes.",
        access: Access::Delete,
        errors: ERRORS,
    };
    fn run(&self, _: &Context<'_>, input: Self::Input) -> Result<Self::Output, CommandError> {
        submit(ContactMutation::Delete(input))
    }
}
