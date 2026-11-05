// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendars and events (WP 1.11).
//!
//! Events are kept for a rolling window around today. Each calendar's rows
//! are synced by delta in *rounds*: a full round starts with
//! [`WriteTx::begin_event_round`], stamps every event it sees with the
//! round number, and [`WriteTx::finish_event_round`] removes the rest. An
//! incremental round (from a saved delta link) stamps what it sees with the
//! same number and removes nothing.

use crate::model::{AccountId, Address, CalendarId, EventId};
use crate::writer::WriteTx;
use crate::{Reader, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

/// A calendar to add or update, matched by account and `remote_id`.
#[derive(Debug, Clone, Default)]
pub struct NewCalendar {
    /// Owning account.
    pub account: AccountId,
    /// Server ID.
    pub remote_id: String,
    /// Display name.
    pub name: String,
    /// Colour name or hex, if the server gives one.
    pub color: Option<String>,
    /// The account's default calendar.
    pub is_default: bool,
    /// Whether the signed-in user can change it.
    pub can_edit: bool,
    /// Owner.
    pub owner: Address,
}

/// A stored calendar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Calendar {
    /// Local ID.
    pub id: CalendarId,
    /// Owning account.
    pub account: AccountId,
    /// Server ID.
    pub remote_id: String,
    /// Display name.
    pub name: String,
    /// Colour name or hex.
    pub color: Option<String>,
    /// The account's default calendar.
    pub is_default: bool,
    /// Whether the signed-in user can change it.
    pub can_edit: bool,
    /// Owner.
    pub owner: Address,
    /// Start of the window the current delta link covers (Unix seconds).
    pub window_start: Option<i64>,
    /// End of that window.
    pub window_end: Option<i64>,
    /// The current round number.
    pub round: i64,
}

/// One attendee of an event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attendee {
    /// Display name, possibly empty.
    pub name: String,
    /// Email address.
    pub addr: String,
    /// `required`, `optional` or `resource`.
    pub kind: String,
    /// Their response: `none`, `accepted`, `tentativelyAccepted`, `declined` ...
    pub response: String,
}

/// An event's fields, as written by the provider and read back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventData {
    /// Owning account.
    pub account: AccountId,
    /// Owning calendar.
    pub calendar: CalendarId,
    /// Server ID (immutable).
    pub remote_id: String,
    /// iCalendar UID, shared by every occurrence of a series.
    pub ical_uid: Option<String>,
    /// The series master's server ID, for occurrences and exceptions.
    pub series_master_id: Option<String>,
    /// `singleInstance`, `occurrence` or `exception`.
    pub kind: String,
    /// Subject.
    pub subject: String,
    /// Location display name.
    pub location: String,
    /// Start (Unix seconds, UTC). For all-day events, midnight UTC of the first day.
    pub start_at: i64,
    /// End (Unix seconds, UTC). For all-day events, midnight UTC after the last day.
    pub end_at: i64,
    /// All-day event: the times stand for dates, not instants.
    pub is_all_day: bool,
    /// IANA zone the start was set in, if known.
    pub start_tz: Option<String>,
    /// IANA zone the end was set in, if known.
    pub end_tz: Option<String>,
    /// `free`, `tentative`, `busy`, `oof`, `workingElsewhere` or `unknown`.
    pub show_as: String,
    /// Cancelled meeting.
    pub is_cancelled: bool,
    /// The signed-in user organised it.
    pub is_organizer: bool,
    /// Organiser.
    pub organizer: Address,
    /// The signed-in user's response, if Graph gives one.
    pub response: Option<String>,
    /// Attendees.
    pub attendees: Vec<Attendee>,
    /// Outlook categories.
    pub categories: Vec<String>,
    /// Start of the body as plain text.
    pub preview: String,
    /// Link to open it in Outlook on the web.
    pub web_link: Option<String>,
    /// Reminder, in minutes before the start, if one is on.
    pub reminder_minutes: Option<i64>,
    /// Graph's change key.
    pub change_key: Option<String>,
}

/// A stored event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Event {
    /// Local ID.
    pub id: EventId,
    /// Its fields.
    #[serde(flatten)]
    pub data: EventData,
}

/// A time range to list events in.
///
/// Timed events overlap `[start, end)` as instants. All-day events are
/// matched by date instead: `floating_start` and `floating_end` are the
/// same wall-clock times as `start` and `end` in the viewer's zone, written
/// as if they were UTC, so an all-day event shows on its own dates in any
/// zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventRange {
    /// Start (Unix seconds).
    pub start: i64,
    /// End (Unix seconds), exclusive.
    pub end: i64,
    /// `start` as local wall-clock time read as UTC.
    pub floating_start: i64,
    /// `end` as local wall-clock time read as UTC.
    pub floating_end: i64,
}

impl EventRange {
    /// A range in UTC, where instants and dates agree.
    pub fn utc(start: i64, end: i64) -> Self {
        Self {
            start,
            end,
            floating_start: start,
            floating_end: end,
        }
    }
}

const CAL_COLS: &str = "id, account_id, remote_id, name, color, is_default, can_edit,
    owner_name, owner_addr, window_start, window_end, round";

fn calendar(r: &Row<'_>) -> rusqlite::Result<Calendar> {
    Ok(Calendar {
        id: r.get(0)?,
        account: r.get(1)?,
        remote_id: r.get(2)?,
        name: r.get(3)?,
        color: r.get(4)?,
        is_default: r.get(5)?,
        can_edit: r.get(6)?,
        owner: Address {
            name: r.get(7)?,
            addr: r.get(8)?,
        },
        window_start: r.get(9)?,
        window_end: r.get(10)?,
        round: r.get(11)?,
    })
}

const EVENT_COLS: &str = "id, account_id, calendar_id, remote_id, ical_uid, series_master_id,
    kind, subject, location, start_at, end_at, is_all_day, start_tz, end_tz, show_as,
    is_cancelled, is_organizer, organizer_name, organizer_addr, response, attendees_json,
    categories_json, preview, web_link, reminder_minutes, change_key";

fn json<T: serde::de::DeserializeOwned>(r: &Row<'_>, i: usize) -> rusqlite::Result<T> {
    let s: String = r.get(i)?;
    serde_json::from_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn event(r: &Row<'_>) -> rusqlite::Result<Event> {
    Ok(Event {
        id: r.get(0)?,
        data: EventData {
            account: r.get(1)?,
            calendar: r.get(2)?,
            remote_id: r.get(3)?,
            ical_uid: r.get(4)?,
            series_master_id: r.get(5)?,
            kind: r.get(6)?,
            subject: r.get(7)?,
            location: r.get(8)?,
            start_at: r.get(9)?,
            end_at: r.get(10)?,
            is_all_day: r.get(11)?,
            start_tz: r.get(12)?,
            end_tz: r.get(13)?,
            show_as: r.get(14)?,
            is_cancelled: r.get(15)?,
            is_organizer: r.get(16)?,
            organizer: Address {
                name: r.get(17)?,
                addr: r.get(18)?,
            },
            response: r.get(19)?,
            attendees: json(r, 20)?,
            categories: json(r, 21)?,
            preview: r.get(22)?,
            web_link: r.get(23)?,
            reminder_minutes: r.get(24)?,
            change_key: r.get(25)?,
        },
    })
}

fn calendars_of(c: &Connection, account: AccountId) -> Result<Vec<Calendar>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {CAL_COLS} FROM calendars WHERE account_id = ?1
         ORDER BY is_default DESC, name COLLATE NOCASE, id"
    ))?;
    Ok(s.query_map([account], calendar)?
        .collect::<Result<_, _>>()?)
}

fn calendar_by_remote(
    c: &Connection,
    account: AccountId,
    remote_id: &str,
) -> Result<Option<Calendar>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {CAL_COLS} FROM calendars WHERE account_id = ?1 AND remote_id = ?2"
    ))?;
    Ok(s.query_row(params![account, remote_id], calendar)
        .optional()?)
}

fn calendar_by_id(c: &Connection, id: CalendarId) -> Result<Option<Calendar>> {
    let mut s = c.prepare_cached(&format!("SELECT {CAL_COLS} FROM calendars WHERE id = ?1"))?;
    Ok(s.query_row([id], calendar).optional()?)
}

fn events_in(c: &Connection, calendars: &[CalendarId], range: EventRange) -> Result<Vec<Event>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT {EVENT_COLS} FROM events WHERE calendar_id = ?1 AND (
           (is_all_day = 0 AND start_at < ?3 AND (end_at > ?2 OR (end_at = start_at AND start_at >= ?2)))
           OR (is_all_day = 1 AND start_at < ?5 AND end_at > ?4))"
    ))?;
    let mut out = Vec::new();
    for cal in calendars {
        let rows = s.query_map(
            params![
                cal,
                range.start,
                range.end,
                range.floating_start,
                range.floating_end
            ],
            event,
        )?;
        for e in rows {
            out.push(e?);
        }
    }
    out.sort_by_key(|e| (e.data.start_at, e.id));
    Ok(out)
}

fn event_by_id(c: &Connection, id: EventId) -> Result<Option<Event>> {
    let mut s = c.prepare_cached(&format!("SELECT {EVENT_COLS} FROM events WHERE id = ?1"))?;
    Ok(s.query_row([id], event).optional()?)
}

fn event_count(c: &Connection, calendar: CalendarId) -> Result<i64> {
    Ok(c.query_row(
        "SELECT COUNT(*) FROM events WHERE calendar_id = ?1",
        [calendar],
        |r| r.get(0),
    )?)
}

impl WriteTx<'_> {
    /// Adds a calendar, or updates the one with the same account and server ID.
    pub fn upsert_calendar(&self, c: &NewCalendar) -> Result<CalendarId> {
        Ok(self.tx.query_row(
            "INSERT INTO calendars (account_id, remote_id, name, color, is_default, can_edit,
                                    owner_name, owner_addr)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (account_id, remote_id) DO UPDATE SET
               name = excluded.name, color = excluded.color, is_default = excluded.is_default,
               can_edit = excluded.can_edit, owner_name = excluded.owner_name,
               owner_addr = excluded.owner_addr
             RETURNING id",
            params![
                c.account,
                c.remote_id,
                c.name,
                c.color,
                c.is_default,
                c.can_edit,
                c.owner.name,
                c.owner.addr
            ],
            |r| r.get(0),
        )?)
    }

    /// Deletes a calendar with its events.
    pub fn delete_calendar(&self, id: CalendarId) -> Result<()> {
        self.tx
            .execute("DELETE FROM calendars WHERE id = ?1", [id])?;
        Ok(())
    }

    /// An account's calendars, the default first.
    pub fn calendars(&self, account: AccountId) -> Result<Vec<Calendar>> {
        calendars_of(self.c(), account)
    }

    /// A calendar by server ID.
    pub fn calendar_by_remote(
        &self,
        account: AccountId,
        remote_id: &str,
    ) -> Result<Option<Calendar>> {
        calendar_by_remote(self.c(), account, remote_id)
    }

    /// Starts a full round of `calendar` for the window `[start, end)` and
    /// returns its round number.
    pub fn begin_event_round(&self, calendar: CalendarId, start: i64, end: i64) -> Result<i64> {
        Ok(self.tx.query_row(
            "UPDATE calendars SET round = round + 1, window_start = ?2, window_end = ?3
             WHERE id = ?1 RETURNING round",
            params![calendar, start, end],
            |r| r.get(0),
        )?)
    }

    /// Ends a full round: removes the calendar's events it did not see.
    /// Returns how many were removed.
    pub fn finish_event_round(&self, calendar: CalendarId, round: i64) -> Result<usize> {
        Ok(self.tx.execute(
            "DELETE FROM events WHERE calendar_id = ?1 AND round <> ?2",
            params![calendar, round],
        )?)
    }

    /// Adds or updates an event, stamped with `round`.
    pub fn upsert_event(&self, e: &EventData, round: i64) -> Result<EventId> {
        let attendees = serde_json::to_string(&e.attendees).expect("attendees serialise");
        let categories = serde_json::to_string(&e.categories).expect("categories serialise");
        Ok(self.tx.query_row(
            "INSERT INTO events (account_id, calendar_id, remote_id, ical_uid, series_master_id,
                kind, subject, location, start_at, end_at, is_all_day, start_tz, end_tz, show_as,
                is_cancelled, is_organizer, organizer_name, organizer_addr, response,
                attendees_json, categories_json, preview, web_link, reminder_minutes,
                change_key, round)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)
             ON CONFLICT (calendar_id, remote_id) DO UPDATE SET
               ical_uid = excluded.ical_uid, series_master_id = excluded.series_master_id,
               kind = excluded.kind, subject = excluded.subject, location = excluded.location,
               start_at = excluded.start_at, end_at = excluded.end_at,
               is_all_day = excluded.is_all_day, start_tz = excluded.start_tz,
               end_tz = excluded.end_tz, show_as = excluded.show_as,
               is_cancelled = excluded.is_cancelled, is_organizer = excluded.is_organizer,
               organizer_name = excluded.organizer_name, organizer_addr = excluded.organizer_addr,
               response = excluded.response, attendees_json = excluded.attendees_json,
               categories_json = excluded.categories_json, preview = excluded.preview,
               web_link = excluded.web_link, reminder_minutes = excluded.reminder_minutes,
               change_key = excluded.change_key, round = excluded.round
             RETURNING id",
            params![
                e.account,
                e.calendar,
                e.remote_id,
                e.ical_uid,
                e.series_master_id,
                e.kind,
                e.subject,
                e.location,
                e.start_at,
                e.end_at,
                e.is_all_day,
                e.start_tz,
                e.end_tz,
                e.show_as,
                e.is_cancelled,
                e.is_organizer,
                e.organizer.name,
                e.organizer.addr,
                e.response,
                attendees,
                categories,
                e.preview,
                e.web_link,
                e.reminder_minutes,
                e.change_key,
                round
            ],
            |r| r.get(0),
        )?)
    }

    /// Deletes an event by server ID. Returns whether there was one.
    pub fn delete_event_by_remote(&self, calendar: CalendarId, remote_id: &str) -> Result<bool> {
        Ok(self.tx.execute(
            "DELETE FROM events WHERE calendar_id = ?1 AND remote_id = ?2",
            params![calendar, remote_id],
        )? > 0)
    }

    /// Deletes a calendar's events that end before `start` or begin at or after `end`.
    pub fn prune_events_outside(
        &self,
        calendar: CalendarId,
        start: i64,
        end: i64,
    ) -> Result<usize> {
        Ok(self.tx.execute(
            "DELETE FROM events WHERE calendar_id = ?1 AND (end_at < ?2 OR start_at >= ?3)",
            params![calendar, start, end],
        )?)
    }

    /// An event by local ID.
    pub fn event(&self, id: EventId) -> Result<Option<Event>> {
        event_by_id(self.c(), id)
    }

    /// Events of the given calendars in a range, by start time.
    pub fn events_in(&self, calendars: &[CalendarId], range: EventRange) -> Result<Vec<Event>> {
        events_in(self.c(), calendars, range)
    }
}

impl Reader {
    /// An account's calendars, the default first.
    pub fn calendars(&self, account: AccountId) -> Result<Vec<Calendar>> {
        calendars_of(&self.conn, account)
    }

    /// A calendar by local ID.
    pub fn calendar(&self, id: CalendarId) -> Result<Option<Calendar>> {
        calendar_by_id(&self.conn, id)
    }

    /// A calendar by server ID.
    pub fn calendar_by_remote(
        &self,
        account: AccountId,
        remote_id: &str,
    ) -> Result<Option<Calendar>> {
        calendar_by_remote(&self.conn, account, remote_id)
    }

    /// Events of the given calendars in a range, by start time.
    pub fn events_in(&self, calendars: &[CalendarId], range: EventRange) -> Result<Vec<Event>> {
        events_in(&self.conn, calendars, range)
    }

    /// An event by local ID.
    pub fn event(&self, id: EventId) -> Result<Option<Event>> {
        event_by_id(&self.conn, id)
    }

    /// How many events a calendar has on this device.
    pub fn event_count(&self, calendar: CalendarId) -> Result<i64> {
        event_count(&self.conn, calendar)
    }
}
