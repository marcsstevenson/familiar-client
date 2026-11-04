// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam auth login`, `fam auth status` and `fam auth logout` (WP 1.4).
//!
//! Tokens are kept by `familiar-auth`: in the OS keychain, or in an encrypted file where
//! there is none. Each account's email and tenant come from `[accounts.<name>]` in
//! `config.toml`; `--authority`, `--email` and `--client-id` override it, which also lets
//! an account sign in before it is in the config.

use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use familiar_auth::{
    Auth, AuthError, AuthErrorKind, DEFAULT_CLIENT_ID, Flow, LoginRequest, Prompt, SCOPES,
    TokenRecord,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

impl From<AuthError> for CommandError {
    fn from(e: AuthError) -> Self {
        let kind = match e.kind {
            AuthErrorKind::Usage => ErrorKind::Usage,
            AuthErrorKind::Config => ErrorKind::Config,
            AuthErrorKind::AuthRequired => ErrorKind::AuthRequired,
            AuthErrorKind::Offline => ErrorKind::Offline,
            AuthErrorKind::Server => ErrorKind::Server,
            AuthErrorKind::Internal => ErrorKind::Internal,
        };
        CommandError::new(kind, e.message)
    }
}

fn open() -> Result<Auth, CommandError> {
    Ok(Auth::from_env()?)
}

/// The config, if there is a config file. A file that exists but doesn't load is an error.
pub(crate) fn config() -> Result<Option<familiar_config::Config>, CommandError> {
    let Some((path, _)) = familiar_config::paths::resolve(None) else {
        return Ok(None);
    };
    match familiar_config::load(&path) {
        Ok(c) => Ok(Some(c)),
        Err(familiar_config::LoadError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(e) => Err(CommandError::new(
            ErrorKind::Config,
            format!("{}: {e}", path.display()),
        )),
    }
}

/// An RFC 3339 UTC time from Unix seconds.
pub(crate) fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// How an account's sign-in stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignInState {
    /// Tokens are stored. Not checked against the server (no `--check`).
    SignedIn,
    /// In `config.toml` but not signed in on this machine.
    SignedOut,
    /// Checked: a token was obtained (refreshing if needed) and Graph `/me` answered.
    Ok,
    /// The account must sign in again.
    AuthRequired,
    /// The server could not be reached.
    Offline,
    /// The check failed for another reason; see `detail`.
    Error,
}

/// One account's sign-in. Never contains tokens.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AccountSignIn {
    /// The account name.
    pub account: String,
    /// The state.
    pub state: SignInState,
    /// The sign-in name, usually the email address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// The display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The account's tenant ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    /// The authority used to sign in (for a signed-out account, the tenant in `config.toml`).
    pub authority: String,
    /// The authority refreshes go to: `consumers` or the tenant ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_authority: Option<String>,
    /// Granted scopes.
    pub scopes: Vec<String>,
    /// When the account signed in (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_in_at: Option<String>,
    /// When the tokens were last refreshed (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<String>,
    /// When the current access token expires (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_expires_at: Option<String>,
    /// Why a check failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl AccountSignIn {
    fn from_record(r: &TokenRecord, state: SignInState) -> Self {
        Self {
            account: r.account.clone(),
            state,
            username: r.username.clone(),
            display_name: r.display_name.clone(),
            tenant_id: r.tenant_id.clone(),
            authority: r.authority.clone(),
            refresh_authority: Some(r.refresh_authority()),
            scopes: r.scopes.clone(),
            signed_in_at: Some(rfc3339(r.signed_in_at)),
            refreshed_at: r.refreshed_at.map(rfc3339),
            access_expires_at: r.access_expires_at.map(rfc3339),
            detail: None,
        }
    }

    fn signed_out(name: &str, account: &familiar_config::Account) -> Self {
        Self {
            account: name.to_string(),
            state: SignInState::SignedOut,
            username: Some(account.email.clone()),
            display_name: None,
            tenant_id: None,
            authority: account.tenant.clone(),
            refresh_authority: None,
            scopes: Vec::new(),
            signed_in_at: None,
            refreshed_at: None,
            access_expires_at: None,
            detail: Some(format!("run `fam auth login {name}`")),
        }
    }

    fn line(&self) -> String {
        let who = self.username.as_deref().unwrap_or("?");
        let state = match self.state {
            SignInState::SignedIn => "signed in",
            SignInState::SignedOut => "signed out",
            SignInState::Ok => "ok",
            SignInState::AuthRequired => "needs sign-in",
            SignInState::Offline => "offline",
            SignInState::Error => "error",
        };
        let mut s = format!("{:<12} {state:<13} {who}", self.account);
        if let Some(r) = &self.refresh_authority {
            s.push_str(&format!("  (refreshes via {r})"));
        }
        if let Some(d) = &self.detail {
            s.push_str(&format!("\n             {d}"));
        }
        s
    }
}

// ---------------------------------------------------------------------------

/// `fam auth login`.
pub struct AuthLogin;

/// Input for `fam auth login`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthLoginInput {
    /// The account name, for example `hotmail` (lowercase letters, digits, - and _).
    #[schemars(extend("x-positional" = 0))]
    pub account: String,
    /// Where to sign in: `consumers` for Hotmail and Outlook.com, `organizations` or the
    /// tenant ID or domain for a work account, or `common`. Defaults to the account's
    /// `tenant` in config.toml.
    #[serde(default)]
    pub authority: Option<String>,
    /// The address to sign in as. Defaults to the account's `email` in config.toml. Sent
    /// as a hint to the sign-in page; a sign-in as anyone else is refused.
    #[serde(default)]
    pub email: Option<String>,
    /// Use a device code (enter it at microsoft.com/devicelogin on any device) instead of
    /// opening a browser here.
    #[serde(default)]
    pub device_code: bool,
    /// The app (client) ID. Defaults to `[app] client_id` in config.toml, then Familiar's.
    #[serde(default)]
    pub client_id: Option<String>,
}

struct NotifyPrompt<'a>(&'a dyn Fn(&str));

impl Prompt for NotifyPrompt<'_> {
    fn show(&self, message: &str) {
        (self.0)(message)
    }
}

impl Command for AuthLogin {
    type Input = AuthLoginInput;
    type Output = AccountSignIn;
    const SPEC: CommandSpec = CommandSpec {
        path: &["auth", "login"],
        summary: "Sign in to a Microsoft account",
        description: "Signs in with the system browser (or a device code with --device-code) and keeps the tokens in the OS keychain, or in an encrypted file where there is none (set FAMILIAR_TOKEN_PASSPHRASE). Signing in again replaces the stored tokens.",
        access: Access::Write,
        errors: &[
            ErrorKind::AuthRequired,
            ErrorKind::Config,
            ErrorKind::Offline,
            ErrorKind::Server,
        ],
    };

    fn run(&self, ctx: &Context<'_>, input: AuthLoginInput) -> Result<AccountSignIn, CommandError> {
        familiar_auth::validate_account(&input.account).map_err(CommandError::usage)?;
        let config = config()?;
        let configured = config.as_ref().and_then(|c| c.accounts.get(&input.account));
        let auth = open()?;
        let earlier = auth.record(&input.account)?;
        let authority = input
            .authority
            .or_else(|| configured.map(|a| a.tenant.clone()))
            .or_else(|| earlier.as_ref().map(|r| r.authority.clone()))
            .ok_or_else(|| {
                CommandError::usage(format!(
                    "account `{}` is not in config.toml: add [accounts.{}] with email and \
                     tenant, or pass --authority (consumers for Hotmail and Outlook.com; \
                     organizations, a tenant ID or a domain for work accounts)",
                    input.account, input.account
                ))
            })?;
        let email = input.email.or_else(|| configured.map(|a| a.email.clone()));
        let client_id = input
            .client_id
            .or_else(|| {
                config
                    .as_ref()
                    .map(|c| c.app.client_id_or_default().to_string())
            })
            .or_else(|| earlier.as_ref().map(|r| r.client_id.clone()))
            .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_string());
        let req = LoginRequest {
            account: input.account,
            authority,
            client_id,
            flow: if input.device_code {
                Flow::DeviceCode
            } else {
                Flow::Browser
            },
            email,
            scopes: SCOPES.to_string(),
        };
        let record = auth.login(&req, &NotifyPrompt(ctx.notify))?;
        Ok(AccountSignIn::from_record(&record, SignInState::SignedIn))
    }

    fn render_text(&self, o: &AccountSignIn) -> String {
        format!(
            "Signed in {} as {} (refreshes via {}).",
            o.account,
            o.username.as_deref().unwrap_or("?"),
            o.refresh_authority.as_deref().unwrap_or("?")
        )
    }
}

// ---------------------------------------------------------------------------

/// `fam auth status`.
pub struct AuthStatus;

/// Input for `fam auth status`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthStatusInput {
    /// Only this account. With --check, a failed check is an error (exit code 4 or 7).
    #[serde(default)]
    #[schemars(extend("x-positional" = 0))]
    pub account: Option<String>,
    /// Prove each sign-in works: refresh the token if needed and call Graph /me.
    #[serde(default)]
    pub check: bool,
}

/// Output of `fam auth status`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AuthStatusOutput {
    /// Where tokens are kept: `keychain` or `file`.
    pub store: String,
    /// Signed-in accounts and accounts in config.toml, by name.
    pub accounts: Vec<AccountSignIn>,
}

impl Command for AuthStatus {
    type Input = AuthStatusInput;
    type Output = AuthStatusOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["auth", "status"],
        summary: "Show which accounts are signed in",
        description: "Lists the accounts in config.toml and any others signed in on this machine. With --check, each sign-in is proved by refreshing if needed and calling Microsoft Graph.",
        access: Access::Read,
        errors: &[
            ErrorKind::AuthRequired,
            ErrorKind::Config,
            ErrorKind::Offline,
            ErrorKind::Server,
        ],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: AuthStatusInput,
    ) -> Result<AuthStatusOutput, CommandError> {
        let config = config()?;
        let auth = open()?;
        let names: Vec<String> = match &input.account {
            Some(a) => vec![a.clone()],
            None => {
                let mut n: std::collections::BTreeSet<String> =
                    auth.accounts()?.into_iter().collect();
                if let Some(c) = &config {
                    n.extend(c.accounts.keys().cloned());
                }
                n.into_iter().collect()
            }
        };
        let mut accounts = Vec::new();
        for name in names {
            let Some(record) = auth.record(&name)? else {
                if let Some(a) = config.as_ref().and_then(|c| c.accounts.get(&name)) {
                    if input.account.is_none() || !input.check {
                        accounts.push(AccountSignIn::signed_out(&name, a));
                        continue;
                    }
                }
                if input.account.is_some() {
                    return Err(CommandError::new(
                        ErrorKind::AuthRequired,
                        format!("account `{name}` is not signed in: run `fam auth login {name}`"),
                    ));
                }
                continue;
            };
            if !input.check {
                accounts.push(AccountSignIn::from_record(&record, SignInState::SignedIn));
                continue;
            }
            match auth.check(&name) {
                Ok(_) => {
                    let fresh = auth.record(&name)?.unwrap_or(record);
                    accounts.push(AccountSignIn::from_record(&fresh, SignInState::Ok));
                }
                Err(e) if input.account.is_some() => return Err(e.into()),
                Err(e) => {
                    let state = match e.kind {
                        AuthErrorKind::AuthRequired => SignInState::AuthRequired,
                        AuthErrorKind::Offline => SignInState::Offline,
                        _ => SignInState::Error,
                    };
                    let mut a = AccountSignIn::from_record(&record, state);
                    a.detail = Some(e.message);
                    accounts.push(a);
                }
            }
        }
        Ok(AuthStatusOutput {
            store: auth.store_name().to_string(),
            accounts,
        })
    }

    fn render_text(&self, o: &AuthStatusOutput) -> String {
        if o.accounts.is_empty() {
            return format!("No accounts signed in (token store: {}).", o.store);
        }
        let mut s = format!("Token store: {}\n", o.store);
        for a in &o.accounts {
            s.push_str(&a.line());
            s.push('\n');
        }
        s.trim_end().to_string()
    }
}

// ---------------------------------------------------------------------------

/// `fam auth logout`.
pub struct AuthLogout;

/// Input for `fam auth logout`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthLogoutInput {
    /// The account to sign out.
    #[serde(default)]
    #[schemars(extend("x-positional" = 0))]
    pub account: Option<String>,
    /// Sign out every account.
    #[serde(default)]
    pub all: bool,
}

/// Output of `fam auth logout`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AuthLogoutOutput {
    /// Accounts whose tokens were removed. Empty if none were signed in.
    pub removed: Vec<String>,
}

impl Command for AuthLogout {
    type Input = AuthLogoutInput;
    type Output = AuthLogoutOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["auth", "logout"],
        summary: "Sign out: forget an account's tokens",
        description: "Removes the account's tokens from this machine. Microsoft offers no sign-out for apps like Familiar; to revoke access everywhere, remove Familiar at myaccount.microsoft.com (personal) or myapps.microsoft.com (work).",
        access: Access::Write,
        errors: &[ErrorKind::Config],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: AuthLogoutInput,
    ) -> Result<AuthLogoutOutput, CommandError> {
        let names = match (&input.account, input.all) {
            (Some(_), true) => {
                return Err(CommandError::usage("give an account or --all, not both"));
            }
            (None, false) => return Err(CommandError::usage("give an account, or --all")),
            (Some(a), false) => vec![a.clone()],
            (None, true) => open()?.accounts()?,
        };
        let auth = open()?;
        let mut removed = Vec::new();
        for n in names {
            if auth.logout(&n)? {
                removed.push(n);
            }
        }
        Ok(AuthLogoutOutput { removed })
    }

    fn render_text(&self, o: &AuthLogoutOutput) -> String {
        if o.removed.is_empty() {
            "Nothing to sign out.".into()
        } else {
            format!("Signed out {}.", o.removed.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_793_836_800), "2026-11-05T00:00:00Z");
        assert_eq!(rfc3339(951_782_400 + 3661), "2000-02-29T01:01:01Z");
    }
}
