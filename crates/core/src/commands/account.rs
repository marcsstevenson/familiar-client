// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam init` and `fam account add` (onboarding WP OB.2).
//!
//! `fam account add` signs in first and writes the account to `config.toml` from who
//! signed in, so a new person never types an email address, tenant or provider. The
//! desktop app's Welcome screen runs the same command.

use crate::agent;
use crate::commands::auth::{NotifyPrompt, config, google_client};
use crate::daemon::{Client, DaemonPaths, METHOD_STOP};
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use familiar_auth::{Auth, CONSUMER_TENANT, Flow, LoginRequest, SCOPES};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn config_path() -> Result<PathBuf, CommandError> {
    familiar_config::paths::resolve(None)
        .map(|(p, _)| p)
        .ok_or_else(|| CommandError::internal("no home folder to keep config.toml in"))
}

fn io_error(path: &std::path::Path, e: std::io::Error) -> CommandError {
    CommandError::internal(format!("could not write {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------

/// `fam init`.
pub struct Init;

/// Input for `fam init`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitInput {}

/// Output of `fam init`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct InitOutput {
    /// The config file.
    pub config: String,
    /// True when this run created it; false when it was already there.
    pub created: bool,
}

impl Command for Init {
    type Input = InitInput;
    type Output = InitOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["init"],
        summary: "Create config.toml for a new install",
        description: "Creates the config folder and a short, commented config.toml if there is none. An existing file is never changed. Add accounts with `fam account add`.",
        access: Access::Write,
        errors: &[],
    };

    fn run(&self, _ctx: &Context<'_>, _input: InitInput) -> Result<InitOutput, CommandError> {
        let path = config_path()?;
        let created = familiar_config::template::create(&path).map_err(|e| io_error(&path, e))?;
        Ok(InitOutput {
            config: path.display().to_string(),
            created,
        })
    }

    fn render_text(&self, o: &InitOutput) -> String {
        if o.created {
            format!(
                "Created {}. Next: `fam account add` (or `fam account add --provider gmail`).",
                o.config
            )
        } else {
            format!("{} already exists; nothing changed.", o.config)
        }
    }
}

// ---------------------------------------------------------------------------

/// Which kind of account to add.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AddProvider {
    /// Outlook.com, Hotmail, Live, or a Microsoft 365 work or school account.
    #[default]
    Microsoft,
    /// A Google account.
    Gmail,
}

/// `fam account add`.
pub struct AccountAdd;

/// Input for `fam account add`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccountAddInput {
    /// `microsoft` (the default) or `gmail`.
    #[serde(default)]
    pub provider: AddProvider,
    /// The account's name in Familiar, used with `--account` and in searches
    /// (`account:hotmail`). Defaults to one made from the address, such as `hotmail`,
    /// `gmail` or the first part of a work domain.
    #[serde(default)]
    pub name: Option<String>,
    /// The address to sign in as, sent as a hint to the sign-in page. A sign-in as anyone
    /// else is refused.
    #[serde(default)]
    pub email: Option<String>,
    /// Use a device code (enter it at microsoft.com/devicelogin on any device) instead of
    /// opening a browser here. Microsoft only.
    #[serde(default)]
    pub device_code: bool,
}

/// Output of `fam account add`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AccountAdded {
    /// The account's name in Familiar.
    pub account: String,
    /// The address that signed in.
    pub email: String,
    /// `microsoft` or `gmail`.
    pub provider: AddProvider,
    /// The sign-in tenant written to config.toml: `consumers` or a tenant ID. Microsoft only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    /// The name on the account, if the sign-in gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The config file the account was written to.
    pub config: String,
    /// True when this command created the config file.
    pub created_config: bool,
    /// True when a running famd was asked to stop so it restarts with the new account
    /// (the desktop app starts it again by itself).
    pub famd_restarted: bool,
}

/// The tenant to write for a Microsoft sign-in: `consumers` for personal accounts, else
/// the work tenant's ID, which is also what token refresh needs.
pub(crate) fn tenant_for(tid: Option<&str>) -> Result<String, CommandError> {
    match tid {
        Some(t) if t.eq_ignore_ascii_case(CONSUMER_TENANT) => Ok("consumers".into()),
        Some(t) if familiar_config::validate::is_guid(t) => Ok(t.to_ascii_lowercase()),
        _ => Err(CommandError::new(
            ErrorKind::Server,
            "the sign-in didn't say which Microsoft tenant the account is in",
        )),
    }
}

/// A name for a new account from its address: `hotmail`, `outlook`, `gmail` and so on
/// for personal mail domains, otherwise the first part of the domain. A number is added
/// when the name is taken.
pub(crate) fn suggest_name(email: &str, provider: AddProvider, taken: &[&str]) -> String {
    let domain = email
        .rsplit_once('@')
        .map_or("", |(_, d)| d)
        .to_ascii_lowercase();
    let first = domain.split('.').next().unwrap_or("");
    let base = match first {
        "googlemail" => "gmail".to_string(),
        _ => {
            let cleaned: String = first
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(40)
                .collect();
            if !cleaned.is_empty() {
                cleaned
            } else if provider == AddProvider::Gmail {
                "gmail".into()
            } else {
                "mail".into()
            }
        }
    };
    if !taken.contains(&base.as_str()) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|n| !taken.contains(&n.as_str()))
        .expect("an unused number")
}

/// Asks a running famd to stop so the desktop app starts it again with the new config.
fn restart_famd() -> bool {
    let Ok(paths) = DaemonPaths::resolve() else {
        return false;
    };
    let Ok(mut client) =
        Client::connect_with_timeout(&paths.socket, std::time::Duration::from_secs(2))
    else {
        return false;
    };
    client
        .call::<_, serde_json::Value>(METHOD_STOP, &serde_json::json!({}))
        .is_ok()
}

impl Command for AccountAdd {
    type Input = AccountAddInput;
    type Output = AccountAdded;
    const SPEC: CommandSpec = CommandSpec {
        path: &["account", "add"],
        summary: "Add a Microsoft or Gmail account",
        description: "Signs in with the system browser (or a device code with --device-code; Microsoft only), then writes the account to config.toml from who signed in: email, display name and, for Microsoft, the tenant. Creates config.toml if there is none, and restarts a running famd so it starts syncing the account. Needs a person: agents can't add accounts.",
        access: Access::Write,
        errors: &[
            ErrorKind::AuthRequired,
            ErrorKind::Config,
            ErrorKind::Offline,
            ErrorKind::Server,
        ],
    };

    fn run(&self, ctx: &Context<'_>, input: AccountAddInput) -> Result<AccountAdded, CommandError> {
        if agent::is_agent(&agent::session_source()) {
            return Err(CommandError::usage(
                "adding an account needs a person at a browser; ask them to run `fam account add`",
            ));
        }
        if input.device_code && input.provider == AddProvider::Gmail {
            return Err(CommandError::usage(
                "Google doesn't allow Gmail sign-in with a device code; sign in with the browser",
            ));
        }
        let existing = config()?;
        let taken: Vec<String> = existing
            .as_ref()
            .map(|c| c.accounts.keys().cloned().collect())
            .unwrap_or_default();
        if let Some(n) = &input.name {
            familiar_auth::validate_account(n).map_err(CommandError::usage)?;
            if taken.contains(n) {
                return Err(CommandError::usage(format!(
                    "there is already an account called `{n}`; choose another --name"
                )));
            }
        }

        let placeholder = input.name.clone().unwrap_or_else(|| "new".into());
        let flow = if input.device_code {
            Flow::DeviceCode
        } else {
            Flow::Browser
        };
        let req = match input.provider {
            AddProvider::Microsoft => LoginRequest {
                account: placeholder,
                // Personal and work accounts both sign in at `common`; the tenant is
                // read from the ID token afterwards.
                authority: "common".into(),
                client_id: existing
                    .as_ref()
                    .map(|c| c.app.client_id_or_default().to_string())
                    .unwrap_or_else(|| familiar_auth::DEFAULT_CLIENT_ID.to_string()),
                client_secret: None,
                flow,
                email: input.email.clone(),
                scopes: SCOPES.to_string(),
            },
            AddProvider::Gmail => {
                let client = google_client(existing.as_ref())?;
                LoginRequest {
                    account: placeholder,
                    authority: familiar_auth::GOOGLE_AUTHORITY.into(),
                    client_id: client.id,
                    client_secret: Some(client.secret),
                    flow,
                    email: input.email.clone(),
                    scopes: familiar_auth::GOOGLE_SCOPES.to_string(),
                }
            }
        };
        let auth = Auth::from_env()?;
        let mut record = auth.sign_in(&req, &NotifyPrompt(ctx.notify))?;

        let email = record.username.clone().ok_or_else(|| {
            CommandError::new(
                ErrorKind::Server,
                "the sign-in didn't say which address signed in",
            )
        })?;
        if let Some((name, _)) = existing.as_ref().and_then(|c| {
            c.accounts
                .iter()
                .find(|(_, a)| a.email.eq_ignore_ascii_case(&email))
        }) {
            return Err(CommandError::usage(format!(
                "{email} is already in Familiar as `{name}`; to sign it in again, run \
                 `fam auth login {name}`"
            )));
        }
        let tenant = match input.provider {
            AddProvider::Microsoft => Some(tenant_for(record.tenant_id.as_deref())?),
            AddProvider::Gmail => None,
        };
        let taken_refs: Vec<&str> = taken.iter().map(String::as_str).collect();
        let name = input
            .name
            .clone()
            .unwrap_or_else(|| suggest_name(&email, input.provider, &taken_refs));

        // Build and check the config change before saving anything.
        let path = config_path()?;
        let created_config =
            familiar_config::template::create(&path).map_err(|e| io_error(&path, e))?;
        let mut doc = familiar_config::ConfigDocument::open(&path)
            .map_err(|e| CommandError::new(ErrorKind::Config, e.0))?;
        let set = |doc: &mut familiar_config::ConfigDocument, key: &str, value: &str| {
            doc.set(&["accounts", &name, key], value)
                .map_err(|e| CommandError::new(ErrorKind::Config, e.0))
        };
        if input.provider == AddProvider::Gmail {
            set(&mut doc, "provider", "gmail")?;
        }
        set(&mut doc, "email", &email)?;
        if let Some(t) = &tenant {
            set(&mut doc, "tenant", t)?;
        }
        if let Some(d) = record
            .display_name
            .as_deref()
            .filter(|d| !d.trim().is_empty())
        {
            set(&mut doc, "display_name", d)?;
        }
        if let Some(e) = doc.check().errors().next() {
            return Err(CommandError::new(
                ErrorKind::Config,
                format!("not added, the config would be invalid: {e}"),
            ));
        }

        record.account = name.clone();
        auth.save(&record)?;
        doc.save(&path)
            .map_err(|e| CommandError::new(ErrorKind::Config, e.0))?;
        let famd_restarted = restart_famd();
        Ok(AccountAdded {
            account: name,
            email,
            provider: input.provider,
            tenant,
            display_name: record.display_name,
            config: path.display().to_string(),
            created_config,
            famd_restarted,
        })
    }

    fn render_text(&self, o: &AccountAdded) -> String {
        let mut s = format!("Added {} as `{}` to {}.", o.email, o.account, o.config);
        if o.famd_restarted {
            s.push_str(" famd is restarting to sync it (the desktop app starts it again; from a terminal, run `famd`).");
        } else {
            s.push_str(" Start `famd` (or open Familiar) to sync it.");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_the_address() {
        let m = AddProvider::Microsoft;
        assert_eq!(suggest_name("a@hotmail.com", m, &[]), "hotmail");
        assert_eq!(suggest_name("a@outlook.co.nz", m, &[]), "outlook");
        assert_eq!(
            suggest_name("a@parklandscricket.co.nz", m, &[]),
            "parklandscricket"
        );
        assert_eq!(
            suggest_name("a@googlemail.com", AddProvider::Gmail, &[]),
            "gmail"
        );
        assert_eq!(
            suggest_name("a@gmail.com", AddProvider::Gmail, &["gmail"]),
            "gmail2"
        );
        assert_eq!(
            suggest_name("a@gmail.com", AddProvider::Gmail, &["gmail", "gmail2"]),
            "gmail3"
        );
        assert_eq!(suggest_name("a@Ex.Ample.com", m, &[]), "ex");
        assert_eq!(suggest_name("odd", m, &[]), "mail");
        for n in ["hotmail", "parklandscricket", "gmail2", "mail"] {
            familiar_auth::validate_account(n).unwrap();
        }
    }

    #[test]
    fn tenants_come_from_the_tid_claim() {
        assert_eq!(tenant_for(Some(CONSUMER_TENANT)).unwrap(), "consumers");
        assert_eq!(
            tenant_for(Some("2112CE74-B8B2-4EDC-876D-508A2CCC4DDB")).unwrap(),
            "2112ce74-b8b2-4edc-876d-508a2ccc4ddb"
        );
        assert!(tenant_for(None).is_err());
        assert!(tenant_for(Some("organizations")).is_err());
    }
}
