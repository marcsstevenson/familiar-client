// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail provider: mail read (WP G.5).
//!
//! - [`GmailClient`]: HTTP to the Gmail API with tokens from a
//!   [`TokenSource`]. Every request goes through the mailbox's [`Scheduler`]
//!   (at most four in flight, user actions first) and a quota budget in
//!   Gmail's units a minute; throttling pauses the mailbox and retries.
//! - [`GmailApi`]: reads that return Gmail's data: profile, labels, message
//!   lists, headers (`metadata`), whole messages (`raw`) and history.
//! - [`GmailSync`]: applies them to the store: labels as folders, the first
//!   listing, `history.list` after it, a resync when history has expired,
//!   and bodies parsed from the raw message.
//!
//! As with the Graph provider, the engine in famd decides what runs when;
//! this crate does one step at a time and keeps mail only in the store.

mod api;
mod calendar;
pub mod calendar_writes;
mod client;
mod compose;
mod contacts;
mod mime;
mod model;
mod sync;

pub use api::{DraftRef, GmailApi, LIST_PAGE_SIZE, METADATA_HEADERS, SendAs};
pub use calendar::{CalendarListReport, EventSyncReport, GoogleCalendarSync};
pub use client::{ClientOptions, DEFAULT_UNITS_PER_MINUTE, GMAIL_V1, GmailClient, units};
pub use compose::{Mailbox, Outgoing, Part, message_id, parts_of};
pub use contacts::GoogleContactSync;
pub use familiar_provider::{
    AuthTokens, Error, Priority, Result, Scheduler, StaticToken, TokenSource,
};
pub use mime::{Body, body_from_raw, to_new_message};
pub use model::{
    GmailLabel, GmailMessage, Header, HistoryMessage, HistoryPage, HistoryRecord, MessageList,
    MessageRef, Payload, Profile,
};
pub use sync::{
    ALL_MAIL, GmailSync, HISTORY_RESOURCE, LABELS_RESOURCE, LISTING_RESOURCE, LabelReport,
    MailReport, SYSTEM_FOLDERS, is_folder_label, is_system_id, primary_label, store_body,
};

/// The crate name, used by the workspace smoke tests.
pub const NAME: &str = env!("CARGO_PKG_NAME");
