// SPDX-License-Identifier: MIT OR Apache-2.0
//! What is stored per account: the refresh token, the current access token and the
//! facts needed to refresh against the right tenant.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// The tenant ID Entra gives every personal Microsoft account (Hotmail, Outlook.com).
pub const CONSUMER_TENANT: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";

/// An access token this close to expiry (seconds) is refreshed before use.
pub const EXPIRY_MARGIN_SECS: u64 = 300;

/// The stored sign-in for one account. `Debug` never prints tokens.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRecord {
    /// Record format version.
    pub version: u32,
    /// The account name from `config.toml`, for example `hotmail`.
    pub account: String,
    /// The app (client) ID the tokens were issued to.
    pub client_id: String,
    /// The authority used to sign in: `consumers`, `organizations`, `common`, a tenant ID or domain.
    pub authority: String,
    /// The account's tenant ID (`tid` claim of the ID token).
    pub tenant_id: Option<String>,
    /// The sign-in name (`preferred_username`), usually the email address.
    pub username: Option<String>,
    /// The display name (`name` claim).
    pub display_name: Option<String>,
    /// Scopes Entra granted, as reported on the last token response.
    pub scopes: Vec<String>,
    /// Scopes asked for on sign-in, asked for again on refresh.
    pub requested_scopes: String,
    /// The refresh token. Entra rotates it on each refresh.
    pub refresh_token: String,
    /// The current access token, if one has been issued.
    pub access_token: Option<String>,
    /// When the access token expires, in Unix seconds.
    pub access_expires_at: Option<u64>,
    /// When the account signed in, in Unix seconds.
    pub signed_in_at: u64,
    /// When the tokens were last refreshed, in Unix seconds.
    pub refreshed_at: Option<u64>,
}

impl fmt::Debug for TokenRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenRecord")
            .field("account", &self.account)
            .field("authority", &self.authority)
            .field("tenant_id", &self.tenant_id)
            .field("username", &self.username)
            .field("scopes", &self.scopes)
            .field("refresh_token", &"<redacted>")
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "<redacted>"),
            )
            .field("access_expires_at", &self.access_expires_at)
            .finish()
    }
}

impl TokenRecord {
    /// The current record format.
    pub const VERSION: u32 = 1;

    /// True for a personal Microsoft account.
    pub fn is_personal(&self) -> bool {
        self.tenant_id.as_deref() == Some(CONSUMER_TENANT)
            || self.authority.eq_ignore_ascii_case("consumers")
    }

    /// Where refreshes go. Personal accounts use `consumers`. Work accounts use their own
    /// tenant ID: refreshing a work account against `organizations` failed with AADSTS65001
    /// in WP 0.2, so the multi-tenant aliases are never used for refresh.
    pub fn refresh_authority(&self) -> String {
        if self.is_personal() {
            return "consumers".into();
        }
        match &self.tenant_id {
            Some(tid) => tid.clone(),
            None => self.authority.clone(),
        }
    }

    /// The access token if it is still good for at least [`EXPIRY_MARGIN_SECS`].
    pub fn usable_access_token(&self, now: u64) -> Option<&str> {
        match (&self.access_token, self.access_expires_at) {
            (Some(t), Some(exp)) if exp > now + EXPIRY_MARGIN_SECS => Some(t),
            _ => None,
        }
    }
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Account names are the keys tokens are stored under: lowercase letters, digits, `-`
/// and `_`, as `config.toml` requires.
pub fn validate_account(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid account name `{name}`: use lowercase letters, digits, - and _ (at most 64)"
        ))
    }
}

#[cfg(test)]
pub(crate) fn sample(account: &str) -> TokenRecord {
    TokenRecord {
        version: TokenRecord::VERSION,
        account: account.into(),
        client_id: "client".into(),
        authority: "organizations".into(),
        tenant_id: Some("tid-1".into()),
        username: Some("a@example.com".into()),
        display_name: Some("A".into()),
        scopes: vec!["Mail.Read".into()],
        requested_scopes: "offline_access Mail.Read".into(),
        refresh_token: "rt".repeat(1500),
        access_token: Some("at".repeat(1200)),
        access_expires_at: Some(10_000),
        signed_in_at: 1,
        refreshed_at: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_accounts_refresh_against_their_tenant() {
        let r = sample("parklands");
        assert_eq!(r.refresh_authority(), "tid-1");
        let mut p = sample("hotmail");
        p.authority = "common".into();
        p.tenant_id = Some(CONSUMER_TENANT.into());
        assert_eq!(p.refresh_authority(), "consumers");
        let mut c = sample("x");
        c.authority = "consumers".into();
        c.tenant_id = None;
        assert_eq!(c.refresh_authority(), "consumers");
    }

    #[test]
    fn debug_hides_tokens() {
        let s = format!("{:?}", sample("a"));
        assert!(!s.contains("rtrt") && !s.contains("atat"), "{s}");
    }

    #[test]
    fn expiry_margin() {
        let r = sample("a");
        assert!(
            r.usable_access_token(10_000 - EXPIRY_MARGIN_SECS - 1)
                .is_some()
        );
        assert!(r.usable_access_token(10_000 - EXPIRY_MARGIN_SECS).is_none());
    }

    #[test]
    fn account_names() {
        assert!(validate_account("hotmail").is_ok());
        assert!(validate_account("park-lands_2").is_ok());
        for bad in ["", "Hotmail", "a b", "a#1", "a/b"] {
            assert!(validate_account(bad).is_err(), "{bad}");
        }
    }
}
