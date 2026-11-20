// SPDX-License-Identifier: MIT OR Apache-2.0
//! A local mock of the Gmail API and Google sign-in, so the Gmail provider and sync tests
//! run offline (WP G.4), in the way `familiar-mock-graph` does for Microsoft Graph.
//!
//! It serves what the Gmail provider uses under `/gmail/v1/users/{me or address}`:
//! the profile, labels (with counts), `messages.list` with `q`, `labelIds` and paging,
//! `messages.get` in `minimal`, `metadata`, `raw` and `full` formats, attachments,
//! modify, batchModify, trash, untrash, delete, threads, drafts (create, update, send,
//! delete), `messages.send` (JSON `raw` and the `/upload` endpoint), `history.list`
//! and `settings.sendAs`, plus `POST /batch/gmail/v1`.
//!
//! It behaves like Gmail where the provider must cope with Gmail's behaviour:
//!
//! - **History.** Every change writes history records (`messageAdded`, `messageDeleted`,
//!   `labelAdded`, `labelRemoved`). [`MailboxHandle::expire_history`] makes
//!   `history.list` from an older history ID answer 404, as after a week offline.
//! - **Labels.** A message has any number of labels; `UNREAD` and `STARRED` are labels.
//! - **Drafts.** A draft keeps its ID but gets a new message ID on every update.
//! - **Sending.** A `From` that is not the account or a verified send-as alias is
//!   rewritten to the account's address.
//! - **Quota.** 429 `rateLimitExceeded` and 403 `userRateLimitExceeded` from injected
//!   [`Fault`]s, or from a per-mailbox quota-unit budget ([`Builder::quota`]).
//! - **Auth.** Missing, unknown, expired or revoked tokens get 401 with
//!   `WWW-Authenticate`. The same port serves the authorisation and token endpoints,
//!   with PKCE, refresh and revocation.
//!
//! ```no_run
//! # async fn demo() -> std::io::Result<()> {
//! use familiar_mock_gmail::{Fault, MockGmail};
//! let mock = MockGmail::start().await?; // mailbox "default", token "test-token"
//! mock.push_fault(Fault::rate_limited().on_path("/history").times(1));
//! let _base = mock.api_url(); // http://127.0.0.1:<port>/gmail/v1
//! # Ok(()) }
//! ```

mod api;
mod auth;
mod faults;
mod fixture;
mod mime;
mod model;
mod server;

pub use faults::Fault;
pub use fixture::SMALL_EMAIL;
pub use model::{
    Label, LabelColor, Mailbox, Message, MessageInfo, MutationError, SYSTEM_LABELS, SendAs,
};
pub use server::{Builder, LoggedRequest, MailboxHandle, MockGmail};
