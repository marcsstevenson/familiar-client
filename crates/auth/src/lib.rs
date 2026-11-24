// SPDX-License-Identifier: MIT OR Apache-2.0
//! Microsoft and Google sign-in for Familiar (WP 1.4, Gmail plan WP G.3).
//!
//! - **Flows.** Authorisation code with PKCE in the system browser (the default) or device
//!   code (for machines without a browser), against the account's authority.
//! - **Storage.** Refresh and access tokens live in the OS keychain, or in an encrypted
//!   file where there is none; see [`store`].
//! - **Refresh.** Access tokens are refreshed five minutes before they expire. Personal
//!   accounts refresh against `consumers`, work accounts against their own tenant ID
//!   (never `organizations`, which failed in WP 0.2). Rotated refresh tokens are saved.
//!
//! - **Google.** An account whose authority is [`GOOGLE_AUTHORITY`] signs in to Google
//!   with PKCE in the browser (Google has no device code for Gmail scopes) using the
//!   person's own Desktop OAuth client, whose secret is kept in the token record because
//!   Google wants it on every refresh.
//!
//! Endpoints default to Microsoft's and Google's and can be pointed at mocks with
//! `FAMILIAR_LOGIN_URL`, `FAMILIAR_GRAPH_URL`, `FAMILIAR_GOOGLE_AUTH_URL`,
//! `FAMILIAR_GOOGLE_TOKEN_URL` and `FAMILIAR_GMAIL_URL` (see [`Endpoints::from_env`]).

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
                          Calendars.ReadWrite Contacts.ReadWrite MailboxSettings.ReadWrite";

/// Scopes Familiar asks Google for: mail now, calendar and contacts later, so nobody has
/// to consent twice (Gmail plan, WP G.0).
pub const GOOGLE_SCOPES: &str = "openid email profile \
    https://www.googleapis.com/auth/gmail.modify \
    https://www.googleapis.com/auth/gmail.send \
    https://www.googleapis.com/auth/gmail.settings.basic \
    https://www.googleapis.com/auth/calendar \
    https://www.googleapis.com/auth/contacts \
    https://www.googleapis.com/auth/contacts.other.readonly";

/// The authority stored for a Google account, in place of a Microsoft tenant.
pub const GOOGLE_AUTHORITY: &str = "google";
/// Google's authorisation endpoint.
pub const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// Google's token endpoint.
pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// The Gmail API, for the signed-in user.
pub const GMAIL_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

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
    /// Google's authorisation endpoint.
    pub google_auth: String,
    /// Google's token endpoint.
    pub google_token: String,
    /// The Gmail API for the signed-in user, ending in `/users/me`.
    pub gmail: String,
}

impl Endpoints {
    /// Microsoft's production endpoints.
    pub fn microsoft() -> Self {
        Self {
            login: LOGIN_URL.into(),
            graph: GRAPH_URL.into(),
            google_auth: GOOGLE_AUTH_URL.into(),
            google_token: GOOGLE_TOKEN_URL.into(),
            gmail: GMAIL_URL.into(),
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
            google_auth: pick("FAMILIAR_GOOGLE_AUTH_URL", GOOGLE_AUTH_URL),
            google_token: pick("FAMILIAR_GOOGLE_TOKEN_URL", GOOGLE_TOKEN_URL),
            gmail: pick("FAMILIAR_GMAIL_URL", GMAIL_URL),
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
    /// `consumers` (Hotmail, Outlook.com), `organizations`, `common`, a tenant ID or domain,
    /// or [`GOOGLE_AUTHORITY`].
    pub authority: String,
    /// The app (client) ID.
    pub client_id: String,
    /// The OAuth client secret, for Google's Desktop clients (Google says it isn't
    /// treated as secret for installed apps). None for Microsoft.
    pub client_secret: Option<String>,
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

/// The signed-in user, from Graph `/me` or Gmail's profile.
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
        let oauth = OAuthClient::new(endpoints.login.clone())
            .with_google(&endpoints.google_auth, &endpoints.google_token);
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
        let record = self.sign_in(req, prompt)?;
        self.save(&record)?;
        Ok(record)
    }

    /// Saves a sign-in from [`Auth::sign_in`] under `record.account`, replacing any
    /// earlier one.
    pub fn save(&self, record: &TokenRecord) -> Result<()> {
        validate_account(&record.account).map_err(AuthError::usage)?;
        self.store.save(record)
    }

    /// Signs in without saving, for a new account whose name is chosen from who signed
    /// in (`fam account add`). `req.account` is only a placeholder for messages.
    pub fn sign_in(&self, req: &LoginRequest, prompt: &dyn Prompt) -> Result<TokenRecord> {
        validate_account(&req.account).map_err(AuthError::usage)?;
        if req.authority.trim().is_empty() {
            return Err(AuthError::usage("the sign-in authority is empty"));
        }
        let google = req.authority == GOOGLE_AUTHORITY;
        if google && req.flow == Flow::DeviceCode {
            return Err(AuthError::usage(
                "Google has no device code sign-in for Gmail: sign in with the browser, on \
                 this machine or on one with a browser and copy the token file across",
            ));
        }
        if google && req.client_secret.is_none() {
            return Err(AuthError::config(
                "Google sign-in needs your Google client file (docs/gmail-setup.md)",
            ));
        }
        let notify = |m: &str| prompt.show(m);
        let tokens = match req.flow {
            Flow::Browser => self.oauth.pkce(
                &oauth::PkceRequest {
                    authority: &req.authority,
                    client_id: &req.client_id,
                    client_secret: req.client_secret.as_deref(),
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
        // Google's ID token has `email` where Microsoft's has `preferred_username`.
        let username = claim("preferred_username").or_else(|| claim("email"));
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
            client_secret: req.client_secret.clone(),
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
            record.client_secret.as_deref(),
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

    /// An access token for the account (Graph, or Google for a Google account), refreshed first if it expires within five
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

    /// Proves the sign-in works: gets a token (refreshing if needed) and calls Graph `/me`
    /// (Gmail's profile for a Google account). A 401 forces one refresh and a retry.
    pub fn check(&self, account: &str) -> Result<Profile> {
        let google = self
            .record(account)?
            .is_some_and(|r| r.authority == GOOGLE_AUTHORITY);
        let me = |token: &str| {
            if google {
                self.gmail_profile(token)
            } else {
                self.me(token)
            }
        };
        let token = self.access_token(account)?;
        match me(&token)? {
            Some(p) => Ok(p),
            None => {
                let record = self.refresh(account)?;
                let token = record.access_token.unwrap_or_default();
                me(&token)?.ok_or_else(|| {
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

    /// Gmail's profile. None on 401.
    fn gmail_profile(&self, token: &str) -> Result<Option<Profile>> {
        let url = format!("{}/profile", self.endpoints.gmail);
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
                let reason = body["error"]["status"].as_str().unwrap_or("-");
                return Err(AuthError::server(format!(
                    "Gmail profile answered HTTP {s} {reason}"
                )));
            }
            Err(e) => return Err(oauth::transport("Gmail", &e)),
        };
        let body: Value = resp
            .into_json()
            .map_err(|e| AuthError::server(format!("Gmail profile: unreadable body: {e}")))?;
        let mail = body["emailAddress"].as_str().map(str::to_string);
        Ok(Some(Profile {
            display_name: None,
            user_principal_name: mail.clone(),
            mail,
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

/// A Google OAuth client: the ID and the Desktop client's secret.
#[derive(Clone, PartialEq, Eq)]
pub struct GoogleClient {
    /// The client ID, ending in `.apps.googleusercontent.com`.
    pub id: String,
    /// The client secret. Google says a Desktop client's secret isn't treated as secret,
    /// but it is still kept out of logs.
    pub secret: String,
}

impl std::fmt::Debug for GoogleClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleClient")
            .field("id", &self.id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Familiar's own Google client (a Desktop app in Marc's unverified production project),
/// used when there is no `google-client.json` and no `[app.google] client_file`. It is
/// built in from `FAMILIAR_GOOGLE_CLIENT_ID` and `FAMILIAR_GOOGLE_CLIENT_SECRET` at
/// compile time, so it never sits in the repo; a build without them has none. The
/// project is capped at 100 users over its lifetime, and people see Google's "hasn't
/// verified this app" page once when they sign in.
const BUILTIN_GOOGLE_CLIENT: Option<(&str, &str)> = match (
    option_env!("FAMILIAR_GOOGLE_CLIENT_ID"),
    option_env!("FAMILIAR_GOOGLE_CLIENT_SECRET"),
) {
    (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => Some((id, secret)),
    _ => None,
};

impl GoogleClient {
    /// Familiar's own Google client, when this build has one; see
    /// [`BUILTIN_GOOGLE_CLIENT`].
    pub fn builtin() -> Option<Self> {
        BUILTIN_GOOGLE_CLIENT.map(|(id, secret)| Self {
            id: id.into(),
            secret: secret.into(),
        })
    }

    /// Reads the client JSON Google Cloud gives you for a Desktop app client.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            AuthError::config(format!(
                "can't read the Google client file {}: {e} (docs/gmail-setup.md says how to \
                 make one)",
                path.display()
            ))
        })?;
        let json: Value = serde_json::from_str(&text)
            .map_err(|e| AuthError::config(format!("{} isn't JSON: {e}", path.display())))?;
        let installed = json.get("installed").ok_or_else(|| {
            AuthError::config(format!(
                "{} is not a Desktop app client: create the client as a Desktop app",
                path.display()
            ))
        })?;
        let field = |k: &str| {
            installed[k]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| AuthError::config(format!("{} has no {k}", path.display())))
        };
        Ok(Self {
            id: field("client_id")?,
            secret: field("client_secret")?,
        })
    }
}

fn split_scopes(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}
