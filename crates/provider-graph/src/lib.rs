// SPDX-License-Identifier: MIT OR Apache-2.0
//! Microsoft Graph provider: mail read (WP 1.6) and calendar read (WP 1.11).
//!
//! - [`GraphClient`]: HTTP to Graph with tokens from a [`TokenSource`], always
//!   asking for immutable IDs. Every request goes through the mailbox's
//!   [`Scheduler`]: at most four in flight, user actions first, and the whole
//!   mailbox pauses for `Retry-After` when Graph throttles.
//! - [`MailApi`]: reads that return Graph's data: the folder tree, header
//!   pages, bodies, folder and message delta pages.
//! - [`MailSync`]: applies them to the store through its single [`familiar_store::Writer`]:
//!   folder-tree delta, per-folder message delta with saved links, move
//!   detection by immutable ID and Internet message ID, and body fetch.
//!
//! The sync engine (WP 1.7) decides what to sync when; this crate does one
//! step at a time and never keeps mail anywhere but the store.

mod calendar;
mod client;
mod error;
mod mail;
mod model;
mod scheduler;
mod sync;
mod write;

pub use calendar::{
    CalendarApi, CalendarListReport, CalendarSync, EVENT_PAGE_SIZE, EventSyncReport, GraphAttendee,
    GraphCalendar, GraphDateTime, GraphEvent, GraphLocation, GraphOwner, GraphResponse,
    events_resource,
};
pub use client::{AuthTokens, ClientOptions, GRAPH_V1, GraphClient, StaticToken, TokenSource};
pub use error::{Error, Result};
pub use mail::{DELTA_PAGE_SIZE, MailApi, STATE_FIELDS};
pub use model::{
    DeltaItem, FOLDER_FIELDS, GraphBody, GraphFolder, GraphMessage, GraphRecipient, HEADER_FIELDS,
    Page, WELL_KNOWN_FOLDERS, importance, parse_time,
};
pub use reqwest::Method;
pub use scheduler::{MAILBOX_CONCURRENCY, Permit, Priority, Scheduler, SchedulerStats};
pub use sync::{
    FOLDERS_RESOURCE, FolderTreeReport, MailSync, MessageSyncReport, messages_resource,
};

/// The crate name, used by the workspace smoke tests.
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn name_is_set() {
        assert_eq!(super::NAME, "familiar-provider-graph");
    }
}
