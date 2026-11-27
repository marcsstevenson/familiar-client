// SPDX-License-Identifier: MIT OR Apache-2.0
//! Provider-confirmed address book writes, serialized with the account's
//! contact sync so an older snapshot cannot overwrite the result.

use familiar_core::agent::{self, Denial};
use familiar_core::commands::contact_book::{
    ContactMutation, ContactTarget, ContactWrite, ContactWritten,
};
use familiar_core::registry::Access;
use familiar_core::{CommandError, ErrorKind};
use familiar_provider_graph::{ContactsApi, Error};
use familiar_store::{AccountId, ContactChanges, ContactId, NewAudit, NewContact};

use crate::service::{AccountApi, Service, command_error};

fn validate(fields: &mut ContactChanges, creating: bool) -> Result<(), CommandError> {
    for value in [&mut fields.name, &mut fields.company]
        .into_iter()
        .flatten()
    {
        *value = value.trim().into();
    }
    for values in [&mut fields.emails, &mut fields.phones]
        .into_iter()
        .flatten()
    {
        for value in values.iter_mut() {
            *value = value.trim().into();
        }
        values.retain(|s| !s.is_empty());
    }
    if *fields == ContactChanges::default() {
        return Err(CommandError::usage("Supply at least one contact field."));
    }
    if creating
        && fields.name.as_deref().unwrap_or_default().is_empty()
        && fields.company.as_deref().unwrap_or_default().is_empty()
        && fields.emails.as_ref().is_none_or(Vec::is_empty)
        && fields.phones.as_ref().is_none_or(Vec::is_empty)
    {
        return Err(CommandError::usage(
            "Enter a name, company, email or phone number.",
        ));
    }
    Ok(())
}

fn provider_error(err: Error, account: &str) -> CommandError {
    if matches!(err, Error::Graph { status: 403, .. }) {
        return CommandError::new(
            ErrorKind::PermissionDenied,
            format!(
                "{err}. If this account has read-only contacts access, run `fam auth login {account}` and allow contact changes."
            ),
        );
    }
    command_error(err)
}

impl Service {
    fn contact_remote(
        &self,
        account: AccountId,
        target: &ContactTarget,
    ) -> Result<String, CommandError> {
        let remote = self
            .w
            .clone()
            .write(|tx| tx.contact_remote_id(account, ContactId(target.contact)))
            .map_err(crate::service::store_error)?
            .ok_or_else(|| {
                CommandError::new(
                    ErrorKind::NotFound,
                    "This account no longer has that contact.",
                )
            })?;
        if target
            .remote_id
            .as_ref()
            .is_some_and(|expected| *expected != remote)
        {
            return Err(CommandError::new(
                ErrorKind::Conflict,
                "The selected contact has changed. Refresh the address book.",
            ));
        }
        Ok(remote)
    }

    pub(crate) async fn contact_write(
        &self,
        p: ContactWrite,
    ) -> Result<ContactWritten, CommandError> {
        let key = p.mutation.account();
        let (&account, handle) = self
            .accounts
            .iter()
            .find(|(_, h)| h.key == key)
            .ok_or_else(|| {
                CommandError::new(
                    ErrorKind::NotFound,
                    format!("No active account named {key}."),
                )
            })?;
        let (command, access) = match &p.mutation {
            ContactMutation::Create(_) => ("contacts.create", Access::Write),
            ContactMutation::Edit(_) => ("contacts.edit", Access::Write),
            ContactMutation::Delete(_) => ("contacts.delete", Access::Delete),
        };
        let audit = |outcome: &str, detail: String| NewAudit {
            account: Some(account),
            source: p.source.clone(),
            command: command.into(),
            outcome: outcome.into(),
            op: None,
            detail,
        };
        if agent::is_agent(&p.source) && !self.agent.get().allows(key, access) {
            let err = Denial {
                account,
                account_key: key.into(),
                access,
                summary: command.into(),
            }
            .error();
            self.w
                .clone()
                .write(|tx| tx.add_audit(&audit("denied", err.message.clone())))
                .map_err(crate::service::store_error)?;
            return Err(err);
        }

        let _guard = handle.shared.contacts_lock.lock().await;
        let (target, change) = match p.mutation {
            ContactMutation::Create(i) => (None, Some(i.fields)),
            ContactMutation::Edit(i) => (Some(i.target), Some(i.fields)),
            ContactMutation::Delete(i) if i.yes => (Some(i.target), None),
            ContactMutation::Delete(_) => {
                return Err(CommandError::new(
                    ErrorKind::ConfirmationRequired,
                    "Confirm deletion with --yes.",
                ));
            }
        };
        let remote = target
            .as_ref()
            .map(|t| self.contact_remote(account, t))
            .transpose()?;
        if change.as_ref().is_some_and(|c| c.starred.is_some())
            && matches!(handle.api, AccountApi::Graph(_))
        {
            return Err(CommandError::usage(
                "Favourites are supported for Google contacts.",
            ));
        }
        if let Some(expected) = target.as_ref().and_then(|t| t.expected_json.as_ref()) {
            let expected: ContactChanges = serde_json::from_str(expected)
                .map_err(|_| CommandError::usage("Invalid contact snapshot JSON."))?;
            let id = remote.as_deref().expect("a reviewed contact has a target");
            let current = match &handle.api {
                AccountApi::Graph(api) => ContactsApi::new(api.client().clone())
                    .get_contact(id)
                    .await
                    .map(|c| c.to_new(account)),
                AccountApi::Gmail(api) => api.get_contact(account, id).await,
            }
            .map_err(|err| provider_error(err, &handle.key))?;
            let snapshot = ContactChanges {
                name: Some(current.display_name),
                company: Some(current.company),
                emails: Some(current.emails.into_iter().map(|e| e.addr).collect()),
                phones: Some(current.phones),
                starred: Some(current.starred),
            };
            if expected != snapshot {
                return Err(CommandError::new(
                    ErrorKind::Conflict,
                    "The provider's contact has changed since this preview. Sync contacts and review the merge again.",
                ));
            }
        }
        let old = target.as_ref().map(|t| ContactId(t.contact));
        let outcome = match change {
            Some(mut fields) => {
                validate(&mut fields, target.is_none())?;
                handle
                    .api
                    .save_contact(account, remote.as_deref(), &fields)
                    .await
                    .map(Some)
            }
            None => {
                let remote = remote.as_deref().expect("delete has a target");
                match &handle.api {
                    AccountApi::Graph(api) => {
                        ContactsApi::new(api.client().clone())
                            .delete_contact(remote)
                            .await
                    }
                    AccountApi::Gmail(api) => api.delete_contact(remote).await,
                }
                .map(|()| None)
            }
        };
        let saved = match outcome {
            Ok(contact) => contact,
            Err(e) => {
                let err = provider_error(e, &handle.key);
                self.w
                    .clone()
                    .write(|tx| tx.add_audit(&audit("failed", err.message.clone())))
                    .map_err(crate::service::store_error)?;
                return Err(err);
            }
        };
        self.w
            .clone()
            .write(|tx| {
                let contact = if let Some(c) = saved {
                    let id = tx.upsert_contact(&c, 0)?;
                    if let Some(old) = old.filter(|old| *old != id) {
                        tx.delete_contact(account, old)?;
                    }
                    id
                } else {
                    let id = old.expect("delete has a target");
                    tx.delete_contact(account, id)?;
                    id
                };
                tx.add_audit(&audit("done", format!("contact {}", contact.0)))?;
                Ok(ContactWritten { contact: contact.0 })
            })
            .map_err(crate::service::store_error)
    }
}

impl AccountApi {
    async fn save_contact(
        &self,
        account: AccountId,
        remote: Option<&str>,
        fields: &ContactChanges,
    ) -> Result<NewContact, Error> {
        match self {
            Self::Graph(api) => {
                ContactsApi::new(api.client().clone())
                    .save_contact(account, remote, fields)
                    .await
            }
            Self::Gmail(api) => api.save_contact(account, remote, fields).await,
        }
    }
}
