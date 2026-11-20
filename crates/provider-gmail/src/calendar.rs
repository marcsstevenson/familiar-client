// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read-only Google Calendar sync. Fetch every page before replacing a window
//! so an interrupted request leaves the previous snapshot intact.

use std::collections::HashSet;

use chrono::{DateTime, NaiveDate, SecondsFormat};
use familiar_core::tz;
use familiar_store::{
    AccountId, Address, Attendee, CalendarId, EventData, NewCalendar, StoreWriter,
};
use serde::{Deserialize, de::DeserializeOwned};

use crate::{Error, GmailApi, Priority, Result, api::enc};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    items: Vec<T>,
    next_page_token: Option<String>,
    #[serde(default)]
    default_reminders: Vec<Reminder>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: String,
    summary: String,
    summary_override: Option<String>,
    access_role: Option<String>,
    background_color: Option<String>,
    #[serde(default)]
    primary: bool,
}

fn writable(role: Option<&str>) -> bool {
    matches!(
        role,
        Some("owner" | "writer" | "writerWithoutPrivateAccess")
    )
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Person {
    display_name: String,
    email: String,
    #[serde(rename = "self")]
    own: bool,
    optional: bool,
    resource: bool,
    response_status: Option<String>,
}

impl Person {
    fn response(&self) -> Option<String> {
        self.response_status.as_deref().map(|s| {
            match s {
                "tentative" => "tentativelyAccepted",
                "needsAction" => "notResponded",
                other => other,
            }
            .to_owned()
        })
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct When {
    date: Option<NaiveDate>,
    date_time: Option<String>,
    time_zone: Option<String>,
}

impl When {
    fn timestamp(&self) -> Option<i64> {
        if let Some(date) = self.date {
            return Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp());
        }
        tz::graph_date_time(
            self.date_time.as_deref()?,
            self.time_zone.as_deref().unwrap_or("UTC"),
        )
        .map(|instant| instant.timestamp())
    }
}

#[derive(Deserialize)]
struct Reminder {
    minutes: i64,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Reminders {
    use_default: bool,
    overrides: Vec<Reminder>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    id: String,
    #[serde(rename = "iCalUID")]
    ical_uid: Option<String>,
    recurring_event_id: Option<String>,
    original_start_time: Option<When>,
    status: Option<String>,
    transparency: Option<String>,
    summary: Option<String>,
    location: Option<String>,
    description: Option<String>,
    #[serde(default)]
    reminders: Reminders,
    html_link: Option<String>,
    etag: Option<String>,
    #[serde(default)]
    start: When,
    #[serde(default)]
    end: When,
    #[serde(default)]
    organizer: Person,
    #[serde(default)]
    attendees: Vec<Person>,
}

impl Event {
    fn row(
        self,
        account: AccountId,
        calendar: CalendarId,
        defaults: &[Reminder],
    ) -> Result<Option<EventData>> {
        if self.status.as_deref() == Some("cancelled") {
            return Ok(None);
        }
        let times = self
            .start
            .timestamp()
            .zip(self.end.timestamp())
            .ok_or_else(|| Error::Decode(format!("invalid times for event {}", self.id)))?;
        let kind = if self.recurring_event_id.is_none() {
            "singleInstance"
        } else if self
            .original_start_time
            .and_then(|t| t.timestamp())
            .is_some_and(|t| t != times.0)
        {
            "exception"
        } else {
            "occurrence"
        };
        let show_as = if self.transparency.as_deref() == Some("transparent") {
            "free"
        } else if self.status.as_deref() == Some("tentative") {
            "tentative"
        } else {
            "busy"
        };
        Ok(Some(EventData {
            account,
            calendar,
            remote_id: self.id,
            ical_uid: self.ical_uid,
            series_master_id: self.recurring_event_id,
            kind: kind.into(),
            subject: self.summary.unwrap_or_default(),
            location: self.location.unwrap_or_default(),
            start_at: times.0,
            end_at: times.1,
            is_all_day: self.start.date.is_some(),
            start_tz: self.start.time_zone,
            end_tz: self.end.time_zone,
            show_as: show_as.into(),
            is_organizer: self.organizer.own,
            organizer: Address {
                name: self.organizer.display_name,
                addr: self.organizer.email,
            },
            response: self
                .attendees
                .iter()
                .find(|a| a.own)
                .and_then(Person::response),
            attendees: self
                .attendees
                .into_iter()
                .map(|a| Attendee {
                    response: a.response().unwrap_or_else(|| "none".into()),
                    kind: if a.resource {
                        "resource"
                    } else if a.optional {
                        "optional"
                    } else {
                        "required"
                    }
                    .into(),
                    name: a.display_name,
                    addr: a.email,
                })
                .collect(),
            preview: familiar_core::compose::preview_of(
                self.description.as_deref().unwrap_or_default(),
            ),
            // Familiar displays one reminder. Use the one closest to the event.
            reminder_minutes: (if self.reminders.use_default {
                defaults
            } else {
                &self.reminders.overrides
            })
            .iter()
            .map(|r| r.minutes)
            .min(),
            web_link: self.html_link,
            change_key: self.etag,
            ..Default::default()
        }))
    }
}

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
    /// Event pages read.
    pub pages: usize,
    /// Whether this was a full round (always true for window reads).
    pub full_round: bool,
    /// Whether an incremental round restarted (always false for window reads).
    pub restarted: bool,
}

/// Reads one account's calendars and expanded event instances.
#[derive(Clone, Debug)]
pub struct GoogleCalendarSync {
    api: GmailApi,
    account: AccountId,
}

impl GoogleCalendarSync {
    /// Syncs the given store account through its Google client.
    pub fn new(api: GmailApi, account: AccountId) -> Self {
        Self { api, account }
    }

    async fn pages<T: DeserializeOwned>(
        &self,
        base: &str,
        priority: Priority,
    ) -> Result<Vec<Page<T>>> {
        let mut path = base.to_owned();
        let mut pages = Vec::new();
        loop {
            let value = self
                .api
                .client()
                .calendar_request(reqwest::Method::GET, &path, None, priority)
                .await?;
            let mut page: Page<T> = serde_json::from_value(value)?;
            let next = page.next_page_token.take();
            pages.push(page);
            match next {
                Some(token) => path = format!("{base}&pageToken={}", enc(&token)),
                None => return Ok(pages),
            }
        }
    }

    /// Reconciles the complete calendar list for this account.
    pub async fn sync_calendars<W: StoreWriter>(
        &self,
        w: &mut W,
        priority: Priority,
    ) -> Result<CalendarListReport> {
        let pages = self
            .pages::<Entry>(
                "/users/me/calendarList?maxResults=250&showHidden=true",
                priority,
            )
            .await?;
        Ok(w.write(|tx| {
            let mut seen = HashSet::new();
            let mut report = CalendarListReport::default();
            for entry in pages.into_iter().flat_map(|page| page.items) {
                seen.insert(entry.id.clone());
                tx.upsert_calendar(&NewCalendar {
                    account: self.account,
                    remote_id: entry.id,
                    name: entry.summary_override.unwrap_or(entry.summary),
                    color: entry.background_color,
                    is_default: entry.primary,
                    can_edit: writable(entry.access_role.as_deref()),
                    owner: Address::default(),
                })?;
                report.upserted += 1;
            }
            for cal in tx.calendars(self.account)? {
                if !seen.contains(&cal.remote_id) {
                    tx.reset_sync_state(self.account, &format!("events:{}", cal.remote_id))?;
                    tx.delete_calendar(cal.id)?;
                    report.removed += 1;
                }
            }
            Ok(report)
        })?)
    }

    /// Replaces the calendar's window with the server's expanded instances.
    pub async fn sync_events<W: StoreWriter>(
        &self,
        w: &mut W,
        calendar_remote_id: &str,
        window: (i64, i64),
        priority: Priority,
    ) -> Result<EventSyncReport> {
        let cal = w
            .write(|tx| tx.calendar_by_remote(self.account, calendar_remote_id))?
            .ok_or_else(|| {
                Error::NotFound(format!("calendar {calendar_remote_id} is not in the store"))
            })?;
        let bound = |t| {
            DateTime::from_timestamp(t, 0)
                .map(|d| enc(&d.to_rfc3339_opts(SecondsFormat::Secs, true)))
                .ok_or_else(|| Error::Decode(format!("invalid calendar window bound {t}")))
        };
        let path = format!(
            "/calendars/{}/events?singleEvents=true&showDeleted=false&timeMin={}&timeMax={}&maxResults=2500",
            enc(calendar_remote_id),
            bound(window.0)?,
            bound(window.1)?
        );
        let pages = self.pages::<Event>(&path, priority).await?;
        let page_count = pages.len();
        let mut rows = Vec::new();
        for page in pages {
            for event in page.items {
                if let Some(row) = event.row(self.account, cal.id, &page.default_reminders)? {
                    rows.push(row);
                }
            }
        }
        Ok(w.write(|tx| {
            let round = tx.begin_event_round(cal.id, window.0, window.1)?;
            let busy = tx.open_op_targets(self.account)?;
            let series_busy = |master: Option<&str>| {
                master.is_some_and(|m| busy.contains(&familiar_store::series_target(m)))
            };
            let mut upserted = 0;
            for row in &rows {
                if !busy.is_empty()
                    && (series_busy(row.series_master_id.as_deref())
                        || tx
                            .event_by_remote(cal.id, &row.remote_id)?
                            .is_some_and(|old| {
                                busy.contains(&familiar_store::event_target(old.id))
                                    || series_busy(old.data.series_master_id.as_deref())
                            }))
                {
                    continue;
                }
                upserted += 1;
                tx.upsert_event(row, round)?;
            }
            Ok(EventSyncReport {
                upserted,
                removed: tx.finish_event_round(cal.id, round)?,
                pages: page_count,
                full_round: true,
                restarted: false,
            })
        })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn maps_calendar_write_roles() {
        for (role, expected) in [
            ("owner", true),
            ("writer", true),
            ("writerWithoutPrivateAccess", true),
            ("reader", false),
            ("freeBusyReader", false),
        ] {
            let entry: Entry = serde_json::from_value(
                json!({"id":"cal", "summary":"Calendar", "accessRole":role}),
            )
            .unwrap();
            assert_eq!(writable(entry.access_role.as_deref()), expected);
        }
        assert!(!writable(None));
    }

    fn map(value: Value) -> Result<Option<EventData>> {
        serde_json::from_value::<Event>(value)?.row(AccountId(1), CalendarId(2), &[])
    }

    #[test]
    fn all_day_exception_keeps_exclusive_end_and_instance_identity() {
        let row = map(
            json!({"id":"instance", "recurringEventId":"master", "iCalUID":"uid",
            "originalStartTime":{"date":"2026-11-01"}, "start":{"date":"2026-11-02"},
            "end":{"date":"2026-11-04"}, "attendees":[{"self":true,"responseStatus":"tentative"}]}),
        )
        .unwrap()
        .unwrap();
        assert!(row.is_all_day);
        assert_eq!(row.start_at % 86400, 0);
        assert_eq!(row.end_at - row.start_at, 2 * 86400);
        assert_eq!(row.kind, "exception");
        assert_eq!(row.remote_id, "instance");
        assert_eq!(row.series_master_id.as_deref(), Some("master"));
        assert_eq!(row.ical_uid.as_deref(), Some("uid"));
        assert_eq!(row.response.as_deref(), Some("tentativelyAccepted"));
    }

    #[test]
    fn descriptions_become_bounded_plain_text() {
        let row = map(
            json!({"id":"notes", "start":{"date":"2026-11-01"}, "end":{"date":"2026-11-02"},
            "description": format!("<b>Kea &amp; tūī</b>  {}", "ā".repeat(300))}),
        )
        .unwrap()
        .unwrap();
        assert!(row.preview.starts_with("Kea & tūī ā"));
        assert!(row.preview.chars().count() <= 255);
    }

    #[test]
    fn explicit_reminders_override_calendar_defaults_even_when_empty() {
        for (overrides, expected) in [
            (json!([{"minutes":30},{"minutes":0}]), Some(0)),
            (json!([]), None),
        ] {
            let event: Event = serde_json::from_value(json!({"id":"override", "start":{"date":"2026-11-01"},
                "end":{"date":"2026-11-02"}, "reminders":{"useDefault":false,"overrides":overrides}})).unwrap();
            let row = event
                .row(AccountId(1), CalendarId(2), &[Reminder { minutes: 10 }])
                .unwrap()
                .unwrap();
            assert_eq!(row.reminder_minutes, expected);
        }
    }

    #[test]
    fn sparse_cancellation_needs_no_times() {
        assert!(
            map(json!({"id":"gone","status":"cancelled"}))
                .unwrap()
                .is_none()
        );
        assert!(map(json!({"id":"broken"})).is_err());
    }

    #[tokio::test]
    async fn reconciles_only_after_the_last_page() {
        use crate::{ClientOptions, GmailClient, StaticToken};
        use axum::{Json, Router, http::Uri};
        use familiar_store::{NewAccount, Writer};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let phase = Arc::new(AtomicUsize::new(0));
        let state = phase.clone();
        let app = Router::new().fallback(move |uri: Uri| {
            let phase = state.load(Ordering::SeqCst);
            async move {
                let second = uri.query().unwrap().contains("pageToken=a%2Bb");
                let event = |id| json!({"id":id,"reminders":{"useDefault":true},"start":{"dateTime":"2026-11-01T00:00:00Z"},
                    "end":{"dateTime":"2026-11-01T01:00:00Z"}});
                Json(match (uri.path(), second) {
                    ("/calendar/v3/users/me/calendarList", false) => json!({"items":[
                        {"id":"one/primary","summary":"One","accessRole":"owner"}],"nextPageToken":"a+b"}),
                    ("/calendar/v3/users/me/calendarList", true) => json!({"items":[
                        {"id":"two","summary":"Two","accessRole":"reader"},
                        {"id":"three","summary":"Three","accessRole":"writer"}]}),
                    ("/calendar/v3/calendars/one%2Fprimary/events", false) => {
                        assert!(uri.query().unwrap().contains("singleEvents=true"));
                        json!({"items":[event(if phase == 0 {"old"} else {"new"})],"nextPageToken":"a+b"})
                    }
                    ("/calendar/v3/calendars/one%2Fprimary/events", true) => match phase {
                        0 => json!({"defaultReminders":[{"method":"popup","minutes":15}],"items":[event("keep")]}),
                        1 => json!({"items":[{"id":"bad"}]}),
                        _ => json!({"items":[event("keep"),{"id":"old","status":"cancelled"}]}),
                    },
                    path => panic!("unexpected route {path:?}"),
                })
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let opts = ClientOptions::with_base(format!("http://{addr}/gmail/v1/users/me"));
        let api =
            GmailApi::new(GmailClient::new(opts, Arc::new(StaticToken("token".into()))).unwrap());
        let dir = tempfile::tempdir().unwrap();
        let mut w = Writer::open(dir.path()).unwrap();
        let acc = w
            .write(|tx| {
                tx.upsert_account(&NewAccount {
                    key: "google".into(),
                    ..Default::default()
                })
            })
            .unwrap();
        let sync = GoogleCalendarSync::new(api, acc);
        assert_eq!(
            sync.sync_calendars(&mut w, Priority::User)
                .await
                .unwrap()
                .upserted,
            3
        );
        let calendar = |w: &mut Writer, id| {
            w.write(|tx| tx.calendar_by_remote(acc, id))
                .unwrap()
                .unwrap()
        };
        let primary = calendar(&mut w, "one/primary");
        assert!(primary.can_edit && calendar(&mut w, "three").can_edit);
        assert!(!calendar(&mut w, "two").can_edit);
        let window = (1793491200, 1793577600);
        let first = sync
            .sync_events(&mut w, "one/primary", window, Priority::User)
            .await
            .unwrap();
        assert_eq!((first.pages, first.upserted, first.removed), (2, 2, 0));
        let keep = w
            .write(|tx| tx.event_by_remote(primary.id, "keep"))
            .unwrap()
            .unwrap();
        assert_eq!(keep.data.reminder_minutes, Some(15));
        phase.store(1, Ordering::SeqCst);
        assert!(
            sync.sync_events(&mut w, "one/primary", window, Priority::User)
                .await
                .is_err()
        );
        let has = |w: &mut Writer, id| {
            w.write(|tx| tx.event_by_remote(primary.id, id))
                .unwrap()
                .is_some()
        };
        assert!(has(&mut w, "old") && !has(&mut w, "new"));
        phase.store(2, Ordering::SeqCst);
        let next = sync
            .sync_events(&mut w, "one/primary", window, Priority::User)
            .await
            .unwrap();
        assert_eq!((next.pages, next.upserted, next.removed), (2, 2, 1));
        assert!(!has(&mut w, "old") && has(&mut w, "keep") && has(&mut w, "new"));
        server.abort();
    }
}
