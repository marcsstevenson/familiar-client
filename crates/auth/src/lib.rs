// SPDX-License-Identifier: MIT OR Apache-2.0
//! Microsoft sign-in for Familiar (WP 1.4).
//!
//! - **Flows.** Authorisation code with PKCE in the system browser (the default) or device
//!   code (for machines without a browser), against the account's authority.
//! - **Storage.** Refresh and access tokens live in the OS keychain, or in an encrypted
//!   file where there is none; see [`store`].
//! - **Refresh.** Access tokens are refreshed five minutes before they expire. Personal
//!   accounts refresh against `consumers`, work accounts against their own tenant ID
//!   (never `organizations`, which failed in WP 0.2). Rotated refresh tokens are saved.
//!
//! Endpoints default to Microsoft's and can be pointed at the mock with
//! `FAMILIAR_LOGIN_URL` and `FAMILIAR_GRAPH_URL` (see [`Endpoints::from_env`]).

pub mod error;
pub mod oauth;
pub mod record;
pub mod store;

pub use error::{AuthError, AuthErrorKind, Result};
pub use record::{CONSUMER_TENANT, TokenRecord, now_secs, validate_account};
pub use store::{FileStore, KeychainStore, MemoryStore, TokenStore, open_store};

use oauth::{OAuthClient, TokenResponse, jwt_claims};
use serde_json::Value;
use std::time::Duration;

/// The Familiar app's Application (client) ID, registered in Marc's own Azure directory
/// (WP 0.1). Public, not a secret.
pub const DEFAULT_CLIENT_ID: &str = "c6057b6a-a692-47dd-b34e-12629372e32a";

/// Scopes Familiar asks for: everything Phases 1 to 4 need, so nobody has to consent twice.
pub const SCOPES: &str = "openid profile offline_access User.Read Mail.ReadWrite Mail.Send \
                          Calendars.ReadWrite Contacts.Read MailboxSettings.ReadWrite";

/// Microsoft's sign-in host.
pub const LOGIN_URL: &str = "https://login.microsoftonline.com";
/// Microsoft Graph v1.0.
pub const GRAPH_URL: &str = "https://graph.microsoft.com/v1.0";

/// Where sign-in and Graph live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// The identity platform host, without a trailing slash.
    pub login: String,
    /// The Graph base URL, ending in `/v1.0`.
    pub graph: String,
}

impl Endpoints {
    /// Microsoft's production endpoints.
    pub fn microsoft() -> Self {
        Self {
            login: LOGIN_URL.into(),
            graph: GRAPH_URL.into(),
        }
    }

    /// Microsoft's, unless `FAMILIAR_LOGIN_URL` or `FAMILIAR_GRAPH_URL` override them
    /// (tests point them at `familiar-mock-graph`).
    pub fn from_env() -> Self {
        let pick = |var: &str, default: &str| {
            std::env::var(var)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| default.to_string())
                .trim_end_matches('/')
                .to_string()
        };
        Self {
            login: pick("FAMILIAR_LOGIN_URL", LOGIN_URL),
            graph: pick("FAMILIAR_GRAPH_URL", GRAPH_URL),
        }
    }
}

/// How to sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Authorisation code with PKCE in the system browser.
    Browser,
    /// Device code: a code to enter at microsoft.com/devicelogin on any device.
    DeviceCode,
}

/// A sign-in request.
#[derive(Debug, Clone)]
pub struct LoginRequest {
    /// The account name tokens are stored under.
    pub account: String,
    /// `consumers` (Hotmail, Outlook.com), `organizations`, `common`, a tenant ID or domain.
    pub authority: String,
    /// The app (client) ID.
    pub client_id: String,
    /// Browser or device code.
    pub flow: Flow,
    /// The address the account should sign in as. Sent as `login_hint`; signing in as
    /// anyone else is refused and nothing is saved.
    pub email: Option<String>,
    /// Scopes to ask for; normally [`SCOPES`].
    pub scopes: String,
}

/// What the person sees and does during sign-in.
pub trait Prompt {
    /// Shows a line, such as the device code message or the sign-in URL.
    fn show(&self, message: &str);
    /// Opens `url` in the system browser; false if it couldn't.
    fn open_browser(&self, url: &str) -> bool {
        webbrowser::open(url).is_ok()
    }
}

/// The signed-in user, from Graph `/me`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Display name.
    pub display_name: Option<String>,
    /// Sign-in name.
    pub user_principal_name: Option<String>,
    /// Primary SMTP address.
    pub mail: Option<String>,
}

/// Sign-in, refresh and sign-out over a token store.
pub struct Auth {
    store: Box<dyn TokenStore>,
    endpoints: Endpoints,
    oauth: OAuthClient,
    agent: ureq::Agent,
    browser_timeout: Duration,
}

impl Auth {
    /// Uses `store` and `endpoints`.
    pub fn new(store: Box<dyn TokenStore>, endpoints: Endpoints) -> Self {
        let oauth = OAuthClient::new(endpoints.login.clone());
        Self {
            store,
            endpoints,
            oauth,
            agent: oauth::agent(),
            browser_timeout: Duration::from_secs(300),
        }
    }

    /// The store `FAMILIAR_TOKEN_STORE` selects and the endpoints from the environment.
    pub fn from_env() -> Result<Self> {
        Ok(Self::new(open_store()?, Endpoints::from_env()))
    }

    /// How long browser sign-in waits for the redirect. Defaults to five minutes.
    pub fn with_browser_timeout(mut self, timeout: Duration) -> Self {
        self.browser_timeout = timeout;
        self
    }

    /// The store's name: `keychain`, `file` or `memory`.
    pub fn store_name(&self) -> &'static str {
        self.store.name()
    }

    /// The endpoints in use.
    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// The signed-in accounts.
    pub fn accounts(&self) -> Result<Vec<String>> {
        self.store.accounts()
    }

    /// An account's stored sign-in.
    pub fn record(&self, account: &str) -> Result<Option<TokenRecord>> {
        self.store.load(account)
    }

    /// Signs in and saves the tokens. Replaces any earlier sign-in for the account.
    pub fn login(&self, req: &LoginRequest, prompt: &dyn Prompt) -> Result<TokenRecord> {
        validate_account(&req.account).map_err(AuthError::usage)?;
        if req.authority.trim().is_empty() {
            return Err(AuthError::usage("the sign-in authority is empty"));
        }
        let notify = |m: &str| prompt.show(m);
        let tokens = match req.flow {
            Flow::Browser => self.oauth.pkce(
                &oauth::PkceRequest {
                    authority: &req.authority,
                    client_id: &req.client_id,
                    scopes: &req.scopes,
                    login_hint: req.email.as_deref(),
                    timeout: self.browser_timeout,
                },
                &|url| prompt.open_browser(url),
                &notify,
            )?,
            Flow::DeviceCode => {
                self.oauth
                    .device_code(&req.authority, &req.client_id, &req.scopes, &notify)?
            }
        };
        let now = now_secs();
        let claims = tokens.id_token.as_deref().and_then(jwt_claims);
        let claim = |k: &str| {
            claims
                .as_ref()
                .and_then(|c| c[k].as_str())
                .map(str::to_string)
        };
        let username = claim("preferred_username");
        if let (Some(want), Some(got)) = (&req.email, &username) {
            if !want.eq_ignore_ascii_case(got) {
                return Err(AuthError::usage(format!(
                    "signed in as {got}, but account `{}` is {want}. Nothing was saved. Sign in \
                     again with {want} (a private browser window avoids the wrong account)",
                    req.account
                )));
            }
        }
        let Some(refresh_token) = tokens.refresh_token.clone() else {
            return Err(AuthError::server(
                "sign-in returned no refresh token (was offline_access granted?)",
            ));
        };
        let record = TokenRecord {
            version: TokenRecord::VERSION,
            account: req.account.clone(),
            client_id: req.client_id.clone(),
            authority: req.authority.clone(),
            tenant_id: claim("tid"),
            username,
            display_name: claim("name"),
            scopes: split_scopes(&tokens.scope),
            requested_scopes: req.scopes.clone(),
            refresh_token,
            access_expires_at: Some(now + tokens.expires_in),
            access_token: Some(tokens.access_token),
            signed_in_at: now,
            refreshed_at: None,
        };
        self.store.save(&record)?;
        Ok(record)
    }

    /// Refreshes an account's tokens now, whether or not the access token has expired,
    /// and saves the result.
    pub fn refresh(&self, account: &str) -> Result<TokenRecord> {
        let record = self.load_required(account)?;
        self.refresh_record(record)
    }

    fn load_required(&self, account: &str) -> Result<TokenRecord> {
        self.store.load(account)?.ok_or_else(|| {
            AuthError::auth_required(format!(
                "account `{account}` is not signed in: run `fam auth login {account}`"
            ))
        })
    }

    fn refresh_record(&self, mut record: TokenRecord) -> Result<TokenRecord> {
        let authority = record.refresh_authority();
        let answer = self.oauth.refresh(
            &authority,
            &record.client_id,
            &record.refresh_token,
            &record.requested_scopes,
        )?;
        let tokens: TokenResponse = match answer {
            Ok(t) => t,
            Err(e) if e.needs_sign_in() => {
                return Err(AuthError::auth_required(format!(
                    "account `{}` must sign in again ({e}): run `fam auth login {}`",
                    record.account, record.account
                )));
            }
            Err(e) => {
                return Err(AuthError::server(format!(
                    "refreshing `{}` against {authority} failed: {e}",
                    record.account
                )));
            }
        };
        let now = now_secs();
        if let Some(rt) = tokens.refresh_token {
            record.refresh_token = rt;
        }
        if let Some(c) = tokens.id_token.as_deref().and_then(jwt_claims) {
            if let Some(tid) = c["tid"].as_str() {
                record.tenant_id = Some(tid.to_string());
            }
        }
        if !tokens.scope.is_empty() {
            record.scopes = split_scopes(&tokens.scope);
        }
        record.access_token = Some(tokens.access_token);
        record.access_expires_at = Some(now + tokens.expires_in);
        record.refreshed_at = Some(now);
        self.store.save(&record)?;
        Ok(record)
    }

    /// A Graph access token for the account, refreshed first if it expires within five
    /// minutes.
    pub fn access_token(&self, account: &str) -> Result<String> {
        let record = self.load_required(account)?;
        if let Some(t) = record.usable_access_token(now_secs()) {
            return Ok(t.to_string());
        }
        let record = self.refresh_record(record)?;
        record
            .access_token
            .ok_or_else(|| AuthError::internal("refresh gave no access token"))
    }

    /// Proves the sign-in works: gets a token (refreshing if needed) and calls Graph `/me`.
    /// A 401 forces one refresh and a retry.
    pub fn check(&self, account: &str) -> Result<Profile> {
        let token = self.access_token(account)?;
        match self.me(&token)? {
            Some(p) => Ok(p),
            None => {
                let record = self.refresh(account)?;
                let token = record.access_token.unwrap_or_default();
                self.me(&token)?.ok_or_else(|| {
                    AuthError::auth_required(format!(
                        "Graph refused `{account}`'s fresh token: run `fam auth login {account}`"
                    ))
                })
            }
        }
    }

    /// GET /me. None on 401.
    fn me(&self, token: &str) -> Result<Option<Profile>> {
        let url = format!(
            "{}/me?$select=displayName,userPrincipalName,mail",
            self.endpoints.graph
        );
        let resp = match self
            .agent
            .get(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .call()
        {
            Ok(r) => r,
            Err(ureq::Error::Status(401, _)) => return Ok(None),
            Err(ureq::Error::Status(s, r)) => {
                let body: Value = r.into_json().unwrap_or(Value::Null);
                let code = body["error"]["code"].as_str().unwrap_or("-");
                return Err(AuthError::server(format!(
                    "Graph /me answered HTTP {s} {code}"
                )));
            }
            Err(e) => return Err(oauth::transport("Microsoft Graph", &e)),
        };
        let body: Value = resp
            .into_json()
            .map_err(|e| AuthError::server(format!("Graph /me: unreadable body: {e}")))?;
        let s = |k: &str| body[k].as_str().map(str::to_string);
        Ok(Some(Profile {
            display_name: s("displayName"),
            user_principal_name: s("userPrincipalName"),
            mail: s("mail"),
        }))
    }

    /// Forgets an account's tokens. False if it was not signed in. Microsoft has no token
    /// revocation for public clients, so this is local: the refresh token simply stops
    /// being used. To revoke on the server, remove the app at myaccount.microsoft.com
    /// (personal) or myapps.microsoft.com (work).
    pub fn logout(&self, account: &str) -> Result<bool> {
        self.store.delete(account)
    }
}

fn split_scopes(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}
