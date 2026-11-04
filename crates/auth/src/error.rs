// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sign-in errors. The kinds map one to one onto `familiar_core::ErrorKind`.

use std::fmt;

/// What went wrong, as a category the command layer maps to an exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthErrorKind {
    /// Bad input, such as an invalid account name or the wrong account signed in.
    Usage,
    /// No usable token store, or the token file can't be decrypted.
    Config,
    /// The account must sign in (again).
    AuthRequired,
    /// The sign-in service or Graph could not be reached.
    Offline,
    /// The sign-in service or Graph returned an error.
    Server,
    /// Anything else.
    Internal,
}

/// A sign-in error. Messages never contain tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthError {
    /// The category.
    pub kind: AuthErrorKind,
    /// A human-readable explanation.
    pub message: String,
}

impl AuthError {
    /// Builds an error.
    pub fn new(kind: AuthErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn usage(m: impl Into<String>) -> Self {
        Self::new(AuthErrorKind::Usage, m)
    }
    pub(crate) fn config(m: impl Into<String>) -> Self {
        Self::new(AuthErrorKind::Config, m)
    }
    pub(crate) fn auth_required(m: impl Into<String>) -> Self {
        Self::new(AuthErrorKind::AuthRequired, m)
    }
    pub(crate) fn server(m: impl Into<String>) -> Self {
        Self::new(AuthErrorKind::Server, m)
    }
    pub(crate) fn internal(m: impl Into<String>) -> Self {
        Self::new(AuthErrorKind::Internal, m)
    }
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AuthError {}

/// Result with [`AuthError`].
pub type Result<T> = std::result::Result<T, AuthError>;
