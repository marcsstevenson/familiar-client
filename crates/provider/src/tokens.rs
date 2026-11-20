// SPDX-License-Identifier: MIT OR Apache-2.0
//! Access tokens for one account.

use std::sync::Arc;

use crate::error::{Error, Result};

/// Gives the client access tokens for one account.
///
/// Both calls may block (they can talk to the identity platform), so the
/// client runs them on Tokio's blocking pool.
pub trait TokenSource: Send + Sync + 'static {
    /// A token that is valid now, refreshed first if it is about to expire.
    fn access_token(&self) -> Result<String>;
    /// A new token, even if the current one looks valid. Called once after a 401.
    fn refresh(&self) -> Result<String>;
}

/// A fixed token, for tests and the mock.
#[derive(Clone, Debug)]
pub struct StaticToken(pub String);

impl TokenSource for StaticToken {
    fn access_token(&self) -> Result<String> {
        Ok(self.0.clone())
    }
    fn refresh(&self) -> Result<String> {
        Ok(self.0.clone())
    }
}

/// Tokens from the WP 1.4 sign-in store for one account.
pub struct AuthTokens {
    auth: Arc<familiar_auth::Auth>,
    account: String,
}

impl AuthTokens {
    /// Tokens for `account` (its key in config.toml).
    pub fn new(auth: Arc<familiar_auth::Auth>, account: impl Into<String>) -> Self {
        Self {
            auth,
            account: account.into(),
        }
    }
}

impl TokenSource for AuthTokens {
    fn access_token(&self) -> Result<String> {
        self.auth.access_token(&self.account).map_err(Error::from)
    }
    fn refresh(&self) -> Result<String> {
        let record = self.auth.refresh(&self.account)?;
        record
            .access_token
            .ok_or_else(|| Error::Auth("refresh gave no access token".into()))
    }
}
