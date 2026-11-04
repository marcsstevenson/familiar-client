// SPDX-License-Identifier: MIT OR Apache-2.0
//! A local mock of Microsoft Graph, so provider and sync tests run offline (WP 1.1).
//!
//! The mock serves the read API Familiar uses: the user profile and `mailboxSettings`,
//! the folder tree and folder delta, message lists with `$top`/`$skip`/`$select`/`$filter`/
//! `$orderby`, per-folder message delta with `@odata.nextLink` and `@odata.deltaLink`,
//! `$search` (capped at 1,000 results and refused with `$filter` or `$orderby`, as in
//! Graph), calendars, events, `calendarView` and `calendarView/delta`.
//!
//! It behaves like Graph where the provider must cope with Graph's behaviour:
//!
//! - **IDs.** A message's default ID changes when it moves; with
//!   `Prefer: IdType="ImmutableId"` the immutable ID is returned instead.
//! - **Moves in delta** show as `@removed` in the old folder and a new item in the new one.
//! - **Throttling.** 429 with `Retry-After` from injected [`Fault`]s, more than four
//!   concurrent requests per mailbox (`MailboxConcurrency`), or an optional request budget.
//! - **Auth.** Missing, unknown or expired bearer tokens get 401
//!   `InvalidAuthenticationToken` with `WWW-Authenticate`.
//! - **Sync state loss.** [`MailboxHandle::expire_delta_tokens`] makes old delta links
//!   answer 410 `SyncStateNotFound`.
//!
//! The same port also mocks the Microsoft identity platform (`/{tenant}/oauth2/v2.0/...`):
//! authorisation code with PKCE, device code and refresh tokens, with Entra's tenant rules.
//! Point the sign-in code at [`MockGraph::login_url`]; tokens it issues work on the Graph
//! routes. See [`MockGraph::sign_in_as`].
//!
//! Data comes from JSON fixtures ([`Mailbox::from_json`], [`Mailbox::small`]), generated
//! mailboxes of any size ([`SyntheticSpec`]) and recorded exchanges replayed verbatim
//! ([`Recording`]). Tests change a mailbox while the server runs through
//! [`MockGraph::mailbox`]. Writes over HTTP are not served yet; they come with Phase 2.
//!
//! ```no_run
//! # async fn demo() -> std::io::Result<()> {
//! use familiar_mock_graph::{Fault, MockGraph};
//! let mock = MockGraph::start().await?; // mailbox "default", token "test-token"
//! mock.push_fault(Fault::throttle(2).on_path("/delta").times(1));
//! let _base = mock.base_url(); // http://127.0.0.1:<port>/v1.0
//! # Ok(()) }
//! ```

mod faults;
mod identity;
mod model;
mod recorded;
mod render;
mod server;
mod synthetic;

pub use faults::Fault;
pub use identity::{CONSUMER_TENANT, DEFAULT_TENANT};
pub use model::{Calendar, EmailAddress, Event, Folder, Mailbox, Message, MutationError, User};
pub use recorded::{RecordedExchange, Recording};
pub use server::{Builder, LoggedRequest, MailboxHandle, MockGraph};
pub use synthetic::SyntheticSpec;
