// SPDX-License-Identifier: MIT OR Apache-2.0
//! What every mail provider shares: the error type, the per-mailbox
//! throttling [`Scheduler`] with [`Priority`], and [`TokenSource`] for
//! access tokens.
//!
//! The Microsoft Graph provider (`familiar-provider-graph`) uses these today
//! and re-exports them, and the Gmail provider will use the same ones, so
//! `famd` handles one error type and one scheduler whatever the account.

mod error;
mod scheduler;
mod tokens;

pub use error::{Error, Result};
pub use scheduler::{MAILBOX_CONCURRENCY, Permit, Priority, Scheduler, SchedulerStats};
pub use tokens::{AuthTokens, StaticToken, TokenSource};

/// The crate name, used by the workspace smoke tests.
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn name_is_set() {
        assert_eq!(super::NAME, "familiar-provider");
    }
}
