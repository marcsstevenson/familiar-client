// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar read (WP 1.11): calendars, and each calendar's events by
//! `calendarView` delta for a rolling window.
//!
//! # Window and rounds
//!
//! A calendarView delta is tied to the window it started with, which
//! travels inside its links. The engine asks for a window around today
//! (`[calendar] past` and `future` in config); when the window moves on, the
//! saved link is dropped and a new *full round* runs for the new window.
//! Events the full round didn't list are removed at its end, which also
//! drops those that slid out of the window. Between full rounds the delta
//! link brings only changes.
//!
//! Recurring series come expanded by the server: each occurrence and
//! exception is its own event; series masters are not kept. Graph's delta
//! gives an occurrence only its ID, type, times and series master, so its
//! subject, location, organiser, zone and the rest are filled in from the
//! master (one GET per series per sync). Any other event that arrives that
//! sparse is fetched in full by its own ID. Graph may also report changed
//! events just outside the window; those are not stored.
//!
//! # Time zones
//!
//! Every request asks for times in UTC (`Prefer: outlook.timezone="UTC"`),
//! but a start or end in another zone is still read correctly, with a
//! Windows or IANA zone name (see [`familiar_core::tz`]). The organiser's
//! zone (`originalStartTimeZone`, usually a Windows name) is stored as an
//! IANA name.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, NaiveDate, Utc};
use familiar_core::tz;
use familiar_store::{AccountId, Address, Attendee, Calendar, EventData, NewCalendar, StoreWriter};
use serde::Deserialize;

use crate::client::GraphClient;
use crate::error::{Error, Result};
use crate::model::{DeltaItem, GraphRecipient, Page, decode_delta_page, decode_page};
use crate::scheduler::Priority;

/// Events per calendarView delta page.
pub const EVENT_PAGE_SIZE: usize = 100;

/// Sync bookmark for one calendar's events.
pub fn events_resource(calendar_remote_id: &str) -> String {
    format!("events:{calendar_remote_id}")
}

const PREFER_UTC: &str = "outlook.timezone=\"UTC\"";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

// Graph shapes ---------------------------------------------------------------

/// A calendar as Graph returns it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct GraphCalendar {
    /// ID.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Colour name (`auto`, `lightBlue` ...).
    pub color: Option<String>,
    /// Colour as `#rrggbb`, often empty.
    pub hex_color: Option<String>,
    /// The default calendar.
    pub is_default_calendar: bool,
    /// The user can change it.
    pub can_edit: bool,
    /// Owner.
    pub owner: Option<GraphOwner>,
}

/// A calendar owner.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GraphOwner {
    /// Name.
    pub name: Option<String>,
    /// Address.
    pub address: Option<String>,
}

/// Graph's `dateTimeTimeZone`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct GraphDateTime {
    /// Wall-clock time, `2026-11-05T04:00:00.0000000`.
    pub date_time: String,
    /// Zone name, Windows or IANA.
    pub time_zone: Option<String>,
}

/// A location.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct GraphLocation {
    /// Display name.
    pub display_name: Option<String>,
}

/// A response status.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GraphResponse {
    /// `none`, `organizer`, `accepted`, `tentativelyAccepted`, `declined`, `notResponded`.
    pub response: Option<String>,
}

/// An attendee.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct GraphAttendee {
    /// `required`, `optional` or `resource`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Their response.
    pub status: Option<GraphResponse>,
    /// Their address.
    #[serde(flatten)]
    pub recipient: GraphRecipient,
}

/// An event as Graph returns it. Everything is optional because delta
/// pages may carry fewer properties than a plain GET.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct GraphEvent {
    /// Immutable ID.
    pub id: String,
    /// iCalendar UID.
    #[serde(rename = "iCalUId")]
    pub ical_uid: Option<String>,
    /// Series master, for occurrences and exceptions.
    pub series_master_id: Option<String>,
    /// `singleInstance`, `occurrence`, `exception` or `seriesMaster`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Subject.
    pub subject: Option<String>,
    /// Location.
    pub location: Option<GraphLocation>,
    /// Start.
    pub start: Option<GraphDateTime>,
    /// End.
    pub end: Option<GraphDateTime>,
    /// All-day event.
    pub is_all_day: bool,
    /// The organiser's zone for the start.
    pub original_start_time_zone: Option<String>,
    /// The organiser's zone for the end.
    pub original_end_time_zone: Option<String>,
    /// Free/busy status.
    pub show_as: Option<String>,
    /// Cancelled meeting.
    pub is_cancelled: bool,
    /// The user organised it.
    pub is_organizer: bool,
    /// Organiser.
    pub organizer: Option<GraphRecipient>,
    /// The user's response.
    pub response_status: Option<GraphResponse>,
    /// Attendees.
    pub attendees: Vec<GraphAttendee>,
    /// Categories.
    pub categories: Vec<String>,
    /// Body preview.
    pub body_preview: Option<String>,
    /// Outlook on the web link.
    pub web_link: Option<String>,
    /// Reminder on.
    pub is_reminder_on: bool,
    /// Reminder minutes.
    pub reminder_minutes_before_start: Option<i64>,
    /// Change key.
    pub change_key: Option<String>,
}

fn all_day_date(t: &GraphDateTime) -> Option<i64> {
    let date = t.date_time.get(..10)?;
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp())
}

fn instant(t: &GraphDateTime) -> Option<i64> {
    tz::graph_date_time(&t.date_time, t.time_zone.as_deref().unwrap_or("UTC"))
        .map(|d: DateTime<Utc>| d.timestamp())
}

fn zone(name: &Option<String>) -> Option<String> {
    name.as_deref().and_then(tz::to_iana)
}

impl GraphEvent {
    /// Whether Graph sent only the ID, type and times (as calendarView delta
    /// does for occurrences), so the details must come from elsewhere.
    pub fn is_sparse(&self) -> bool {
        self.subject.is_none()
    }

    /// Fills a sparse occurrence's details from its series master. The
    /// occurrence keeps its own ID, type, times and master ID.
    pub fn fill_from_master(&mut self, m: &GraphEvent) {
        let keep = (
            self.id.clone(),
            self.kind.clone(),
            self.start.clone(),
            self.end.clone(),
            self.series_master_id.clone(),
            self.change_key.clone(),
        );
        *self = GraphEvent {
            id: keep.0,
            kind: keep.1.or_else(|| Some("occurrence".into())),
            start: keep.2,
            end: keep.3,
            series_master_id: keep.4,
            change_key: keep.5,
            web_link: None,
            ..m.clone()
        };
    }

    /// Whether this is a series master (not stored: its occurrences are).
    pub fn is_series_master(&self) -> bool {
        self.kind.as_deref() == Some("seriesMaster")
    }

    /// Start and end as Unix seconds. All-day events give midnight UTC of
    /// their dates, whatever zone Graph labelled them with.
    pub fn times(&self) -> Option<(i64, i64)> {
        let (s, e) = (self.start.as_ref()?, self.end.as_ref()?);
        let (s, e) = if self.is_all_day {
            (all_day_date(s)?, all_day_date(e)?)
        } else {
            (instant(s)?, instant(e)?)
        };
        Some((s, e.max(s)))
    }

    /// The store row, or `None` if the event has no usable times.
    pub fn to_event_data(
        &self,
        account: AccountId,
        calendar: familiar_store::CalendarId,
    ) -> Option<EventData> {
        let (start_at, end_at) = self.times()?;
        let organizer = self
            .organizer
            .as_ref()
            .map(GraphRecipient::to_address)
            .unwrap_or_default();
        Some(EventData {
            account,
            calendar,
            remote_id: self.id.clone(),
            ical_uid: self.ical_uid.clone(),
            series_master_id: self.series_master_id.clone(),
            kind: self.kind.clone().unwrap_or_else(|| "singleInstance".into()),
            subject: self.subject.clone().unwrap_or_default(),
            location: self
                .location
                .as_ref()
                .and_then(|l| l.display_name.clone())
                .unwrap_or_default(),
            start_at,
            end_at,
            is_all_day: self.is_all_day,
            start_tz: zone(&self.original_start_time_zone),
            end_tz: zone(&self.original_end_time_zone),
            show_as: self.show_as.clone().unwrap_or_else(|| "busy".into()),
            is_cancelled: self.is_cancelled,
            is_organizer: self.is_organizer,
            organizer,
            response: self
                .response_status
                .as_ref()
                .and_then(|r| r.response.clone()),
            attendees: self
                .attendees
                .iter()
                .map(|a| {
                    let addr = a.recipient.to_address();
                    Attendee {
                        name: addr.name,
                        addr: addr.addr,
                        kind: a.kind.clone().unwrap_or_else(|| "required".into()),
                        response: a
                            .status
                            .as_ref()
                            .and_then(|s| s.response.clone())
                            .unwrap_or_else(|| "none".into()),
                    }
                })
                .collect(),
            categories: self.categories.clone(),
            preview: self.body_preview.clone().unwrap_or_default(),
            web_link: self.web_link.clone(),
            reminder_minutes: self
                .is_reminder_on
                .then_some(self.reminder_minutes_before_start)
                .flatten(),
            change_key: self.change_key.clone(),
        })
    }
}

fn overlaps(e: &EventData, start: i64, end: i64) -> bool {
    // All-day dates are in no particular zone: allow a day either side.
    let slack = if e.is_all_day { 86_400 } else { 0 };
    e.start_at < end + slack && (e.end_at > start - slack || e.start_at >= start - slack)
}

fn rfc3339(t: i64) -> String {
    DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

// Reads ----------------------------------------------------------------------

/// Calendar reads for one mailbox.
#[derive(Clone, Debug)]
pub struct CalendarApi {
    client: GraphClient,
    user: String,
}

impl CalendarApi {
    /// Reads the signed-in user's calendars.
    pub fn new(client: GraphClient) -> Self {
        Self {
            client,
            user: "/me".into(),
        }
    }

    /// The client underneath.
    pub fn client(&self) -> &GraphClient {
        &self.client
    }

    /// Every calendar the user has, including shared ones they added.
    pub async fn calendars(&self, priority: Priority) -> Result<Vec<GraphCalendar>> {
        let mut out = Vec::new();
        let mut url = format!("{}/calendars?$top=100", self.user);
        loop {
            let page: Page<GraphCalendar> =
                decode_page(self.client.get(&url, priority, &[]).await?)?;
            out.extend(page.items);
            match page.next_link {
                Some(n) => url = n,
                None => return Ok(out),
            }
        }
    }

    /// One event by ID, in full, with times in UTC. `None` if it's gone.
    pub async fn event(&self, id: &str, priority: Priority) -> Result<Option<GraphEvent>> {
        match self
            .client
            .get(
                &format!("{}/events/{id}", self.user),
                priority,
                &[PREFER_UTC],
            )
            .await
        {
            Ok(v) => Ok(Some(serde_json::from_value(v)?)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// One page of a calendar's calendarView delta for `[start, end)`.
    /// Start with `link = None`; follow `next_link` until a page carries
    /// the `delta_link`. The window travels inside the links.
    pub async fn event_delta_page(
        &self,
        calendar: &str,
        (start, end): (i64, i64),
        link: Option<&str>,
        page_size: usize,
        priority: Priority,
    ) -> Result<Page<DeltaItem<GraphEvent>>> {
        let url = match link {
            Some(l) => l.to_string(),
            None => format!(
                "{}/calendars/{calendar}/calendarView/delta?startDateTime={}&endDateTime={}",
                self.user,
                rfc3339(start),
                rfc3339(end)
            ),
        };
        let size = format!("odata.maxpagesize={}", page_size.max(1));
        decode_delta_page(
            self.client
                .get(&url, priority, &[&size, PREFER_UTC])
                .await?,
        )
    }
}

// Writes ---------------------------------------------------------------------

impl CalendarApi {
    async fn send_utc(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
        priority: Priority,
    ) -> Result<serde_json::Value> {
        self.client
            .request(method, path, body, priority, &[PREFER_UTC])
            .await
    }

    /// Creates an event in a calendar (a series master when `body` has a
    /// recurrence). Give `body` a `transactionId`: Graph then answers a
    /// repeated POST with the event the first one made.
    pub async fn create_event(
        &self,
        calendar: &str,
        body: &serde_json::Value,
        priority: Priority,
    ) -> Result<GraphEvent> {
        let v = self
            .send_utc(
                reqwest::Method::POST,
                &format!("{}/calendars/{calendar}/events", self.user),
                Some(body),
                priority,
            )
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    /// Changes an event (an occurrence becomes an exception; a series
    /// master changes the whole series).
    pub async fn update_event(
        &self,
        id: &str,
        body: &serde_json::Value,
        priority: Priority,
    ) -> Result<GraphEvent> {
        let v = self
            .send_utc(
                reqwest::Method::PATCH,
                &format!("{}/events/{id}", self.user),
                Some(body),
                priority,
            )
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    /// Deletes an event (a series master takes its series with it). On the
    /// organiser's calendar, Graph sends attendees a cancellation.
    pub async fn delete_event(&self, id: &str, priority: Priority) -> Result<()> {
        self.client
            .delete(&format!("{}/events/{id}", self.user), priority)
            .await
    }

    /// Answers a meeting invitation: `accept`, `tentativelyAccept` or
    /// `decline`, telling the organiser when `send_response`.
    pub async fn respond(
        &self,
        id: &str,
        action: &str,
        comment: Option<&str>,
        send_response: bool,
        priority: Priority,
    ) -> Result<()> {
        let body = serde_json::json!({
            "comment": comment.unwrap_or(""),
            "sendResponse": send_response,
        });
        self.client
            .post(
                &format!("{}/events/{id}/{action}", self.user),
                &body,
                priority,
            )
            .await
            .map(|_| ())
    }

    /// Cancels a meeting the user organised, with a message to attendees.
    pub async fn cancel_event(&self, id: &str, comment: &str, priority: Priority) -> Result<()> {
        self.client
            .post(
                &format!("{}/events/{id}/cancel", self.user),
                &serde_json::json!({ "comment": comment }),
                priority,
            )
            .await
            .map(|_| ())
    }

    /// A series master's occurrences and exceptions in `[start, end)`.
    pub async fn instances(
        &self,
        master: &str,
        (start, end): (i64, i64),
        priority: Priority,
    ) -> Result<Vec<GraphEvent>> {
        let mut out = Vec::new();
        let mut url = format!(
            "{}/events/{master}/instances?startDateTime={}&endDateTime={}&$top=100",
            self.user,
            rfc3339(start),
            rfc3339(end)
        );
        loop {
            let page: Page<GraphEvent> =
                decode_page(self.client.get(&url, priority, &[PREFER_UTC]).await?)?;
            out.extend(page.items);
            match page.next_link {
                Some(n) => url = n,
                None => return Ok(out),
            }
        }
    }
}

// Sync -----------------------------------------------------------------------

/// What a calendar-list sync changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarListReport {
    /// Calendars added or updated.
    pub upserted: usize,
    /// Calendars removed locally (with their events).
    pub removed: usize,
}

/// What an event sync of one calendar changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventSyncReport {
    /// Events added or updated.
    pub upserted: usize,
    /// Events removed (deleted on the server, or out of the window).
    pub removed: usize,
    /// Delta pages read.
    pub pages: usize,
    /// Whether this was a full round (first sync, new window, or after a 410).
    pub full_round: bool,
    /// Whether the saved link had expired (410) and the round started over.
    pub restarted: bool,
}

/// Syncs one account's calendars into the store.
#[derive(Debug, Clone)]
pub struct CalendarSync {
    api: CalendarApi,
    account: AccountId,
    page_size: usize,
}

impl CalendarSync {
    /// Syncs `account` (a store account ID) through `api`.
    pub fn new(api: CalendarApi, account: AccountId) -> Self {
        Self {
            api,
            account,
            page_size: EVENT_PAGE_SIZE,
        }
    }

    /// Events per delta page.
    pub fn with_page_size(mut self, n: usize) -> Self {
        self.page_size = n.max(1);
        self
    }

    /// The Graph reads underneath.
    pub fn api(&self) -> &CalendarApi {
        &self.api
    }

    /// Brings the list of calendars up to date. Calendars gone from the
    /// server are removed with their events and bookmarks.
    pub async fn sync_calendars<W: StoreWriter>(
        &self,
        w: &mut W,
        priority: Priority,
    ) -> Result<CalendarListReport> {
        let remote = self.api.calendars(priority).await?;
        let acc = self.account;
        Ok(w.write(|tx| {
            let mut report = CalendarListReport::default();
            let seen: HashSet<&str> = remote.iter().map(|c| c.id.as_str()).collect();
            for c in &remote {
                let owner = c.owner.clone().unwrap_or_default();
                let color = c
                    .hex_color
                    .clone()
                    .filter(|h| !h.is_empty())
                    .or_else(|| c.color.clone());
                tx.upsert_calendar(&NewCalendar {
                    account: acc,
                    remote_id: c.id.clone(),
                    name: c.name.clone(),
                    color,
                    is_default: c.is_default_calendar,
                    can_edit: c.can_edit,
                    owner: Address {
                        name: owner.name.unwrap_or_default(),
                        addr: owner.address.unwrap_or_default(),
                    },
                })?;
                report.upserted += 1;
            }
            for c in tx.calendars(acc)? {
                if !seen.contains(c.remote_id.as_str()) {
                    tx.reset_sync_state(acc, &events_resource(&c.remote_id))?;
                    tx.delete_calendar(c.id)?;
                    report.removed += 1;
                }
            }
            Ok(report)
        })?)
    }

    /// Brings one calendar's events for the window `[start, end)` up to
    /// date. A different window from the saved link's starts a full round.
    pub async fn sync_events<W: StoreWriter>(
        &self,
        w: &mut W,
        calendar_remote_id: &str,
        window: (i64, i64),
        priority: Priority,
    ) -> Result<EventSyncReport> {
        let acc = self.account;
        let resource = events_resource(calendar_remote_id);
        let mut report = EventSyncReport::default();
        let mut masters = HashMap::new();
        loop {
            match self
                .round(
                    w,
                    calendar_remote_id,
                    &resource,
                    window,
                    priority,
                    &mut report,
                    &mut masters,
                )
                .await
            {
                Err(Error::SyncStateGone(_)) if !report.restarted => {
                    report.restarted = true;
                    w.write(|tx| tx.reset_sync_state(acc, &resource))?;
                }
                r => return r.map(|()| report),
            }
        }
    }

    /// Fills in sparse events: occurrences from their series master (each
    /// master fetched once per sync), anything else by its own ID.
    async fn complete(
        &self,
        items: &mut [DeltaItem<GraphEvent>],
        priority: Priority,
        masters: &mut HashMap<String, Option<GraphEvent>>,
    ) -> Result<()> {
        for item in items.iter_mut() {
            let DeltaItem::Upsert(ev) = item else {
                continue;
            };
            if !ev.is_sparse() || ev.is_series_master() {
                continue;
            }
            match ev.series_master_id.clone() {
                Some(mid) if ev.kind.as_deref() != Some("exception") => {
                    if !masters.contains_key(&mid) {
                        let m = self.api.event(&mid, priority).await?;
                        masters.insert(mid.clone(), m);
                    }
                    if let Some(Some(m)) = masters.get(&mid) {
                        ev.fill_from_master(m);
                    }
                }
                _ => {
                    if let Some(full) = self.api.event(&ev.id, priority).await? {
                        *ev = full;
                    }
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn round<W: StoreWriter>(
        &self,
        w: &mut W,
        remote: &str,
        resource: &str,
        (start, end): (i64, i64),
        priority: Priority,
        report: &mut EventSyncReport,
        masters: &mut HashMap<String, Option<GraphEvent>>,
    ) -> Result<()> {
        let acc = self.account;
        let (cal, saved): (Calendar, Option<String>) = w
            .write(|tx| {
                let cal = tx.calendar_by_remote(acc, remote)?;
                let saved = tx.sync_state(acc, resource)?.and_then(|s| s.delta_link);
                Ok((cal, saved))
            })
            .map_err(Error::from)
            .and_then(|(cal, saved)| {
                cal.map(|c| (c, saved)).ok_or_else(|| {
                    Error::NotFound(format!("calendar {remote} is not in the store"))
                })
            })?;

        let same_window = cal.window_start == Some(start) && cal.window_end == Some(end);
        let mut link = saved.filter(|_| same_window);
        // A saved nextLink means a full round was interrupted; finish it.
        let mut full = link.as_deref().is_none_or(|l| !l.contains("deltatoken"));
        let round = if link.is_none() {
            w.write(|tx| {
                tx.reset_sync_state(acc, resource)?;
                tx.begin_event_round(cal.id, start, end)
            })?
        } else {
            cal.round
        };
        if link.is_none() {
            full = true;
        }
        report.full_round |= full;

        loop {
            let mut page = self
                .api
                .event_delta_page(
                    remote,
                    (start, end),
                    link.as_deref(),
                    self.page_size,
                    priority,
                )
                .await?;
            self.complete(&mut page.items, priority, masters).await?;
            report.pages += 1;
            let done = page.delta_link.is_some();
            let next = page.delta_link.clone().or(page.next_link.clone());
            let at = now();
            let (up, rm) = w.write(|tx| {
                let (mut up, mut rm) = (0, 0);
                // Events with changes still queued keep their local state.
                let busy = tx.open_op_targets(acc)?;
                let held = |id: &str, master: Option<&str>| -> familiar_store::Result<bool> {
                    if master.is_some_and(|m| busy.contains(&familiar_store::series_target(m))) {
                        return Ok(true);
                    }
                    Ok(match tx.event_by_remote(cal.id, id)? {
                        Some(e) => {
                            busy.contains(&familiar_store::event_target(e.id))
                                || e.data.series_master_id.as_deref().is_some_and(|m| {
                                    busy.contains(&familiar_store::series_target(m))
                                })
                        }
                        None => false,
                    })
                };
                for item in &page.items {
                    match item {
                        DeltaItem::Removed(id) => {
                            if !held(id, None)? {
                                rm += usize::from(tx.delete_event_by_remote(cal.id, id)?);
                            }
                        }
                        DeltaItem::Upsert(ev) if ev.is_series_master() => {}
                        DeltaItem::Upsert(ev) if held(&ev.id, ev.series_master_id.as_deref())? => {}
                        DeltaItem::Upsert(ev) => match ev.to_event_data(acc, cal.id) {
                            Some(d) if overlaps(&d, start, end) => {
                                tx.upsert_event(&d, round)?;
                                up += 1;
                            }
                            _ => {
                                rm += usize::from(tx.delete_event_by_remote(cal.id, &ev.id)?);
                            }
                        },
                    }
                }
                if done && full {
                    rm += tx.finish_event_round(cal.id, round)?;
                }
                if done {
                    rm += tx.prune_events_outside(cal.id, start - 86_400, end + 86_400)?;
                }
                match &next {
                    Some(l) => tx.set_sync_state(acc, resource, None, Some(l), at)?,
                    None => {
                        tx.set_sync_error(acc, resource, "delta page without a next or delta link")?
                    }
                }
                Ok((up, rm))
            })?;
            report.upserted += up;
            report.removed += rm;
            if done {
                return Ok(());
            }
            match next {
                Some(n) => link = Some(n),
                None => {
                    return Err(Error::Decode(
                        "calendarView delta page without a next or delta link".into(),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_in_a_windows_zone() {
        let e: GraphEvent = serde_json::from_value(json!({
            "id": "e1", "subject": "Swim", "type": "occurrence", "seriesMasterId": "m",
            "start": {"dateTime": "2026-11-05T17:00:00.0000000", "timeZone": "New Zealand Standard Time"},
            "end": {"dateTime": "2026-11-05T18:00:00.0000000", "timeZone": "New Zealand Standard Time"},
            "originalStartTimeZone": "New Zealand Standard Time",
            "organizer": {"emailAddress": {"name": "Sam", "address": "sam@example.com"}},
            "attendees": [{"type": "optional", "status": {"response": "accepted"},
                           "emailAddress": {"name": "Pat", "address": "pat@example.com"}}],
            "isReminderOn": true, "reminderMinutesBeforeStart": 30
        }))
        .unwrap();
        let d = e
            .to_event_data(AccountId(1), familiar_store::CalendarId(2))
            .unwrap();
        // 17:00 NZDT is 04:00 UTC.
        assert_eq!(rfc3339(d.start_at), "2026-11-05T04:00:00Z");
        assert_eq!(d.end_at - d.start_at, 3600);
        assert_eq!(d.start_tz.as_deref(), Some("Pacific/Auckland"));
        assert_eq!(d.kind, "occurrence");
        assert_eq!(d.organizer.addr, "sam@example.com");
        assert_eq!(d.attendees[0].kind, "optional");
        assert_eq!(d.attendees[0].response, "accepted");
        assert_eq!(d.reminder_minutes, Some(30));
    }

    #[test]
    fn all_day_keeps_its_dates() {
        let e: GraphEvent = serde_json::from_value(json!({
            "id": "e2", "isAllDay": true,
            "start": {"dateTime": "2026-12-20T00:00:00.0000000", "timeZone": "New Zealand Standard Time"},
            "end": {"dateTime": "2026-12-21T00:00:00.0000000", "timeZone": "New Zealand Standard Time"}
        }))
        .unwrap();
        let (s, en) = e.times().unwrap();
        assert_eq!(rfc3339(s), "2026-12-20T00:00:00Z");
        assert_eq!(en - s, 86_400);
    }

    #[test]
    fn sparse_all_day_occurrence_takes_the_master() {
        let mut occ: GraphEvent = serde_json::from_value(json!({
            "id": "occ1", "type": "occurrence", "seriesMasterId": "m1",
            "start": {"dateTime": "2026-11-10T00:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-11T00:00:00.0000000", "timeZone": "UTC"}
        }))
        .unwrap();
        assert!(occ.is_sparse());
        let master: GraphEvent = serde_json::from_value(json!({
            "id": "m1", "type": "seriesMaster", "subject": "Bin day", "isAllDay": true,
            "location": {"displayName": "Kerb"}, "showAs": "free",
            "originalStartTimeZone": "New Zealand Standard Time",
            "start": {"dateTime": "2026-01-06T00:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-01-07T00:00:00.0000000", "timeZone": "UTC"}
        }))
        .unwrap();
        occ.fill_from_master(&master);
        let d = occ
            .to_event_data(AccountId(1), familiar_store::CalendarId(1))
            .unwrap();
        assert_eq!(d.remote_id, "occ1");
        assert_eq!(d.kind, "occurrence");
        assert_eq!(d.subject, "Bin day");
        assert_eq!(d.location, "Kerb");
        assert!(d.is_all_day);
        assert_eq!(rfc3339(d.start_at), "2026-11-10T00:00:00Z");
        assert_eq!(d.end_at - d.start_at, 86_400);
        assert_eq!(d.start_tz.as_deref(), Some("Pacific/Auckland"));
        assert_eq!(d.series_master_id.as_deref(), Some("m1"));
    }

    #[test]
    fn sparse_event_without_times_is_skipped() {
        let e: GraphEvent = serde_json::from_value(json!({"id": "x"})).unwrap();
        assert!(
            e.to_event_data(AccountId(1), familiar_store::CalendarId(1))
                .is_none()
        );
    }
}
