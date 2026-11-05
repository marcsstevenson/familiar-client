// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar writes (WP 2.4): what an event op carries, its local effects
//! and the Graph JSON it turns into.
//!
//! Like mail writes, an event change shows in the store at once and is
//! queued for the server (see [`crate::ops`]). A new event gets a local row
//! with a placeholder server ID; when the server has made it, the row takes
//! the server's ID and fields. A recurring event is expanded by the server:
//! its first occurrence shows locally until then, and the server's
//! occurrences replace it once the series is made.
//!
//! Times travel to Graph as wall-clock times in the event's zone (with its
//! Windows name), so a weekly event keeps its local time across daylight
//! saving changes. All-day events are dates.

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Utc};
use chrono_tz::Tz;
use familiar_store::{
    AccountId, Address, Attendee, CalendarId, EventData, EventId, LOCAL_REMOTE_PREFIX, WriteTx,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::compose::{Recipient, preview_of};
use crate::error::CommandError;
use crate::tz;

/// How often a recurring event repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepeatKind {
    /// Every `interval` days.
    Daily,
    /// Every `interval` weeks, on `days`.
    Weekly,
    /// Every `interval` months, on the start's day of the month.
    Monthly,
    /// Every `interval` years, on the start's date.
    Yearly,
}

/// A recurrence, expanded by the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Recurrence {
    /// Daily, weekly, monthly or yearly.
    pub every: RepeatKind,
    /// Every this many days, weeks, months or years (1 or more).
    pub interval: u32,
    /// Weekly: the days (`monday` ...). Empty means the start's day.
    #[serde(default)]
    pub days: Vec<String>,
    /// Last date it can fall on (YYYY-MM-DD, in the event's zone).
    #[serde(default)]
    pub until: Option<String>,
    /// Or: how many times.
    #[serde(default)]
    pub count: Option<u32>,
}

/// A new event's fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventFields {
    /// Subject.
    pub subject: String,
    /// Start (Unix seconds). All-day: midnight UTC of the first day.
    pub start_at: i64,
    /// End (Unix seconds). All-day: midnight UTC after the last day.
    pub end_at: i64,
    /// All-day event.
    #[serde(default)]
    pub all_day: bool,
    /// The event's zone (IANA).
    pub time_zone: String,
    /// Location.
    #[serde(default)]
    pub location: String,
    /// Body (HTML fragment).
    #[serde(default)]
    pub body_html: String,
    /// Required attendees (they get an invitation).
    #[serde(default)]
    pub attendees: Vec<Recipient>,
    /// Optional attendees.
    #[serde(default)]
    pub optional: Vec<Recipient>,
    /// `free`, `tentative`, `busy`, `oof` or `workingElsewhere`.
    #[serde(default)]
    pub show_as: Option<String>,
    /// Reminder minutes before the start; `None` is the server's default.
    #[serde(default)]
    pub reminder_minutes: Option<i64>,
    /// No reminder.
    #[serde(default)]
    pub no_reminder: bool,
    /// Outlook categories.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Repeats.
    #[serde(default)]
    pub recurrence: Option<Recurrence>,
}

/// Changes to an event; `None` leaves a field as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventChanges {
    /// New subject.
    #[serde(default)]
    pub subject: Option<String>,
    /// New start (Unix seconds; all-day: midnight UTC).
    #[serde(default)]
    pub start_at: Option<i64>,
    /// New end.
    #[serde(default)]
    pub end_at: Option<i64>,
    /// All-day or not.
    #[serde(default)]
    pub all_day: Option<bool>,
    /// The zone new times are in (IANA).
    #[serde(default)]
    pub time_zone: Option<String>,
    /// New location.
    #[serde(default)]
    pub location: Option<String>,
    /// New body (HTML fragment).
    #[serde(default)]
    pub body_html: Option<String>,
    /// New required attendees (replaces them, and the optional ones).
    #[serde(default)]
    pub attendees: Option<Vec<Recipient>>,
    /// New free/busy status.
    #[serde(default)]
    pub show_as: Option<String>,
    /// New reminder, in minutes.
    #[serde(default)]
    pub reminder_minutes: Option<i64>,
    /// Turn the reminder off.
    #[serde(default)]
    pub no_reminder: bool,
    /// New categories (replaces them).
    #[serde(default)]
    pub categories: Option<Vec<String>>,
}

impl EventChanges {
    /// Whether nothing would change.
    pub fn is_empty(&self) -> bool {
        *self == EventChanges::default()
    }

    /// Whether the times change.
    pub fn retimes(&self) -> bool {
        self.start_at.is_some() || self.end_at.is_some() || self.all_day.is_some()
    }
}

/// An answer to an invitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rsvp {
    /// Accept.
    Accept,
    /// Accept tentatively.
    Tentative,
    /// Decline (the meeting leaves the calendar).
    Decline,
}

impl Rsvp {
    /// Graph's action, e.g. `tentativelyAccept`.
    pub fn action(self) -> &'static str {
        match self {
            Rsvp::Accept => "accept",
            Rsvp::Tentative => "tentativelyAccept",
            Rsvp::Decline => "decline",
        }
    }

    /// The response Graph then shows, e.g. `tentativelyAccepted`.
    pub fn response(self) -> &'static str {
        match self {
            Rsvp::Accept => "accepted",
            Rsvp::Tentative => "tentativelyAccepted",
            Rsvp::Decline => "declined",
        }
    }

    /// In words.
    pub fn verb(self) -> &'static str {
        match self {
            Rsvp::Accept => "accept",
            Rsvp::Tentative => "tentatively accept",
            Rsvp::Decline => "decline",
        }
    }
}

/// A stored event as it was, to undo a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventSnapshot {
    /// Local event ID.
    pub id: i64,
    /// Its fields (`familiar_store::EventData`).
    pub data: Value,
}

/// Create an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventOp {
    /// Local ID of the new event (its first occurrence, if recurring).
    pub event: i64,
    /// Local calendar ID.
    pub calendar: i64,
    /// What it is.
    pub fields: EventFields,
    /// Sent as Graph's `transactionId`, so a repeated POST makes one event.
    pub transaction_id: String,
    /// The server's ID once made (a series master's, if recurring).
    #[serde(default)]
    pub remote_id: Option<String>,
    /// The email it was made from (`fam cal add --from-mail`), linked in its body.
    #[serde(default)]
    pub from_mail: Option<i64>,
}

/// Change an event, or a whole series.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventUpdateOp {
    /// Local event ID.
    pub event: i64,
    /// The series master's server ID, to change the whole series.
    #[serde(default)]
    pub series: Option<String>,
    /// What changes.
    pub changes: EventChanges,
    /// The rows as they were.
    pub prev: Vec<EventSnapshot>,
}

/// Answer an invitation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventRespondOp {
    /// Local event ID.
    pub event: i64,
    /// The series master's server ID, to answer for the whole series.
    #[serde(default)]
    pub series: Option<String>,
    /// The answer.
    pub response: Rsvp,
    /// A note to the organiser.
    #[serde(default)]
    pub comment: Option<String>,
    /// Tell the organiser.
    pub send_response: bool,
    /// The rows as they were.
    pub prev: Vec<EventSnapshot>,
}

/// Delete an event or a series (a meeting you organised is cancelled).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventDeleteOp {
    /// Local event ID.
    pub event: i64,
    /// The series master's server ID, to delete the whole series.
    #[serde(default)]
    pub series: Option<String>,
    /// A cancellation note to attendees (a meeting you organised).
    #[serde(default)]
    pub comment: Option<String>,
}

fn zone(name: &str) -> Tz {
    tz::parse_zone(name).unwrap_or(Tz::UTC)
}

/// The zone name Graph is given: Windows when there is one.
pub fn graph_zone(iana: &str) -> String {
    tz::iana_to_windows(iana)
        .map(str::to_string)
        .unwrap_or_else(|| iana.to_string())
}

fn wall(t: i64, z: Tz) -> NaiveDateTime {
    DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .with_timezone(&z)
        .naive_local()
}

fn date_of(t: i64) -> NaiveDate {
    DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .date_naive()
}

/// Graph's `dateTimeTimeZone` for a time.
pub fn graph_time(t: i64, all_day: bool, iana: &str) -> Value {
    let local = if all_day {
        date_of(t).and_hms_opt(0, 0, 0).expect("midnight")
    } else {
        wall(t, zone(iana))
    };
    json!({
        "dateTime": local.format("%Y-%m-%dT%H:%M:%S").to_string(),
        "timeZone": graph_zone(iana),
    })
}

fn attendees_json(req: &[Recipient], opt: &[Recipient]) -> Value {
    let one = |r: &Recipient, kind: &str| json!({"emailAddress": {"name": r.name, "address": r.address}, "type": kind});
    Value::Array(
        req.iter()
            .map(|r| one(r, "required"))
            .chain(opt.iter().map(|r| one(r, "optional")))
            .collect(),
    )
}

fn recurrence_json(r: &Recurrence, start_at: i64, all_day: bool, iana: &str) -> Value {
    let first = if all_day {
        date_of(start_at)
    } else {
        wall(start_at, zone(iana)).date()
    };
    let mut pattern = json!({
        "type": match r.every {
            RepeatKind::Daily => "daily",
            RepeatKind::Weekly => "weekly",
            RepeatKind::Monthly => "absoluteMonthly",
            RepeatKind::Yearly => "absoluteYearly",
        },
        "interval": r.interval.max(1),
    });
    match r.every {
        RepeatKind::Weekly => {
            let days: Vec<String> = if r.days.is_empty() {
                vec![weekday_name(first.weekday()).to_string()]
            } else {
                r.days.clone()
            };
            pattern["daysOfWeek"] = json!(days);
            pattern["firstDayOfWeek"] = json!("sunday");
        }
        RepeatKind::Monthly => pattern["dayOfMonth"] = json!(first.day()),
        RepeatKind::Yearly => {
            pattern["dayOfMonth"] = json!(first.day());
            pattern["month"] = json!(first.month());
        }
        RepeatKind::Daily => {}
    }
    let mut range = json!({
        "startDate": first.format("%Y-%m-%d").to_string(),
        "recurrenceTimeZone": graph_zone(iana),
    });
    match (&r.until, r.count) {
        (Some(u), _) => {
            range["type"] = json!("endDate");
            range["endDate"] = json!(u);
        }
        (None, Some(n)) => {
            range["type"] = json!("numbered");
            range["numberOfOccurrences"] = json!(n);
        }
        (None, None) => range["type"] = json!("noEnd"),
    }
    json!({ "pattern": pattern, "range": range })
}

/// The `POST /events` body for a new event.
pub fn event_json(f: &EventFields, transaction_id: &str, link_html: Option<&str>) -> Value {
    let mut body = f.body_html.clone();
    if let Some(l) = link_html {
        body.push_str(l);
    }
    let mut v = json!({
        "subject": f.subject,
        "body": {"contentType": "HTML", "content": format!("<html><body>{body}</body></html>")},
        "start": graph_time(f.start_at, f.all_day, &f.time_zone),
        "end": graph_time(f.end_at, f.all_day, &f.time_zone),
        "isAllDay": f.all_day,
        "location": {"displayName": f.location},
        "attendees": attendees_json(&f.attendees, &f.optional),
        "categories": f.categories,
        "transactionId": transaction_id,
    });
    if let Some(s) = &f.show_as {
        v["showAs"] = json!(s);
    }
    if f.no_reminder {
        v["isReminderOn"] = json!(false);
    } else if let Some(m) = f.reminder_minutes {
        v["isReminderOn"] = json!(true);
        v["reminderMinutesBeforeStart"] = json!(m);
    }
    if let Some(r) = &f.recurrence {
        v["recurrence"] = recurrence_json(r, f.start_at, f.all_day, &f.time_zone);
    }
    v
}

/// The `PATCH /events/{id}` body for `c`, against the event as `cur` is.
pub fn changes_json(c: &EventChanges, cur: &EventData) -> Value {
    let mut v = serde_json::Map::new();
    if let Some(s) = &c.subject {
        v.insert("subject".into(), json!(s));
    }
    if let Some(b) = &c.body_html {
        v.insert(
            "body".into(),
            json!({"contentType": "HTML", "content": format!("<html><body>{b}</body></html>")}),
        );
    }
    if c.retimes() {
        let all_day = c.all_day.unwrap_or(cur.is_all_day);
        let iana = c
            .time_zone
            .clone()
            .or_else(|| cur.start_tz.clone())
            .unwrap_or_else(|| "UTC".into());
        let start = c.start_at.unwrap_or(cur.start_at);
        let end = c.end_at.unwrap_or(cur.end_at);
        v.insert("isAllDay".into(), json!(all_day));
        v.insert("start".into(), graph_time(start, all_day, &iana));
        v.insert("end".into(), graph_time(end, all_day, &iana));
    }
    if let Some(l) = &c.location {
        v.insert("location".into(), json!({"displayName": l}));
    }
    if let Some(a) = &c.attendees {
        v.insert("attendees".into(), attendees_json(a, &[]));
    }
    if let Some(s) = &c.show_as {
        v.insert("showAs".into(), json!(s));
    }
    if c.no_reminder {
        v.insert("isReminderOn".into(), json!(false));
    } else if let Some(m) = c.reminder_minutes {
        v.insert("isReminderOn".into(), json!(true));
        v.insert("reminderMinutesBeforeStart".into(), json!(m));
    }
    if let Some(cats) = &c.categories {
        v.insert("categories".into(), json!(cats));
    }
    Value::Object(v)
}

fn weekday_name(d: chrono::Weekday) -> &'static str {
    use chrono::Weekday::*;
    match d {
        Mon => "monday",
        Tue => "tuesday",
        Wed => "wednesday",
        Thu => "thursday",
        Fri => "friday",
        Sat => "saturday",
        Sun => "sunday",
    }
}

/// Reads `mon`, `Tuesday` and so on as Graph's day names.
pub fn parse_weekday(s: &str) -> Option<&'static str> {
    let s = s.trim().to_ascii_lowercase();
    if s.len() < 2 {
        return None;
    }
    [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ]
    .into_iter()
    .find(|d| d.starts_with(&s[..s.len().min(3)]) && d.starts_with(s.as_str()))
}

/// How a time reads in the event's own zone, for summaries.
pub fn when_text(start: i64, end: i64, all_day: bool, iana: &str) -> String {
    if all_day {
        let first = date_of(start);
        let last = date_of(end - 86_400).max(first);
        if last == first {
            return format!("{} (all day)", first.format("%a %-d %b %Y"));
        }
        return format!(
            "{} to {} (all day)",
            first.format("%a %-d %b"),
            last.format("%a %-d %b %Y")
        );
    }
    let z = zone(iana);
    let (s, e) = (wall(start, z), wall(end, z));
    if s.date() == e.date() {
        format!(
            "{} {}–{}",
            s.format("%a %-d %b %Y"),
            s.format("%H:%M"),
            e.format("%H:%M")
        )
    } else {
        format!(
            "{} to {}",
            s.format("%a %-d %b %Y %H:%M"),
            e.format("%a %-d %b %H:%M")
        )
    }
}

/// Checks a new event's times.
pub fn check_times(start: i64, end: i64, all_day: bool) -> Result<(), CommandError> {
    if end <= start {
        return Err(CommandError::usage("the end must come after the start"));
    }
    if all_day && (start % 86_400 != 0 || end % 86_400 != 0) {
        return Err(CommandError::internal("all-day times must be whole dates"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Local effects

fn to_attendees(req: &[Recipient], opt: &[Recipient]) -> Vec<Attendee> {
    let one = |r: &Recipient, kind: &str| Attendee {
        name: r.name.clone(),
        addr: r.address.clone(),
        kind: kind.into(),
        response: "none".into(),
    };
    req.iter()
        .map(|r| one(r, "required"))
        .chain(opt.iter().map(|r| one(r, "optional")))
        .collect()
}

/// The local row for a new event, before the server has it.
pub fn new_event_data(
    f: &EventFields,
    account: AccountId,
    calendar: CalendarId,
    me: Address,
    remote_id: String,
) -> EventData {
    EventData {
        account,
        calendar,
        remote_id,
        ical_uid: None,
        series_master_id: None,
        kind: "singleInstance".into(),
        subject: f.subject.clone(),
        location: f.location.clone(),
        start_at: f.start_at,
        end_at: f.end_at,
        is_all_day: f.all_day,
        start_tz: Some(f.time_zone.clone()),
        end_tz: Some(f.time_zone.clone()),
        show_as: f.show_as.clone().unwrap_or_else(|| "busy".into()),
        is_cancelled: false,
        is_organizer: true,
        organizer: me,
        response: Some("organizer".into()),
        attendees: to_attendees(&f.attendees, &f.optional),
        categories: f.categories.clone(),
        preview: preview_of(&f.body_html),
        web_link: None,
        reminder_minutes: if f.no_reminder {
            None
        } else {
            Some(f.reminder_minutes.unwrap_or(15))
        },
        change_key: None,
    }
}

/// Inserts a new event's local row and records its ID in the op.
pub fn apply_create(tx: &WriteTx<'_>, op: &mut EventOp) -> familiar_store::Result<()> {
    let cal = CalendarId(op.calendar);
    let account = tx
        .accounts()?
        .into_iter()
        .find(|a| {
            tx.calendars(a.id)
                .map(|cs| cs.iter().any(|c| c.id == cal))
                .unwrap_or(false)
        })
        .ok_or_else(|| familiar_store::Error::NotFound(format!("calendar {}", op.calendar)))?;
    let round = tx
        .calendars(account.id)?
        .into_iter()
        .find(|c| c.id == cal)
        .map_or(0, |c| c.round);
    let me = Address {
        name: account.display_name.clone(),
        addr: account.email.clone(),
    };
    let d = new_event_data(
        &op.fields,
        account.id,
        cal,
        me,
        crate::ops::placeholder_remote_id(),
    );
    op.event = tx.upsert_event(&d, round)?.0;
    Ok(())
}

/// Removes a new event's row if the server never made it.
pub fn undo_create(tx: &WriteTx<'_>, op: &EventOp) -> familiar_store::Result<()> {
    if let Some(e) = tx.event(EventId(op.event))? {
        if e.data.remote_id.starts_with(LOCAL_REMOTE_PREFIX) {
            tx.delete_event(e.id)?;
        }
    }
    Ok(())
}

/// The rows an op on `event` (or its whole series) touches.
pub fn rows(
    tx: &WriteTx<'_>,
    event: i64,
    series: Option<&str>,
) -> familiar_store::Result<Vec<familiar_store::Event>> {
    let Some(e) = tx.event(EventId(event))? else {
        return Ok(Vec::new());
    };
    match series {
        Some(m) => tx.events_of_series(e.data.account, m),
        None => Ok(vec![e]),
    }
}

/// Snapshots rows to undo a change.
pub fn snapshot(rows: &[familiar_store::Event]) -> Vec<EventSnapshot> {
    rows.iter()
        .map(|e| EventSnapshot {
            id: e.id.0,
            data: serde_json::to_value(&e.data).unwrap_or(Value::Null),
        })
        .collect()
}

/// Puts snapshotted rows back.
pub fn restore(tx: &WriteTx<'_>, prev: &[EventSnapshot]) -> familiar_store::Result<()> {
    for s in prev {
        let Ok(d) = serde_json::from_value::<EventData>(s.data.clone()) else {
            continue;
        };
        match tx.replace_event(EventId(s.id), &d) {
            Err(familiar_store::Error::NotFound(_)) => {}
            r => r?,
        }
    }
    Ok(())
}

/// Applies changes to one row's fields. Times only change on a single
/// event; a series is re-read from the server once it has the change.
pub fn change_data(d: &mut EventData, c: &EventChanges, whole_series: bool) {
    if let Some(s) = &c.subject {
        d.subject.clone_from(s);
    }
    if let Some(l) = &c.location {
        d.location.clone_from(l);
    }
    if let Some(b) = &c.body_html {
        d.preview = preview_of(b);
    }
    if !whole_series {
        if let Some(a) = c.all_day {
            d.is_all_day = a;
        }
        if let Some(s) = c.start_at {
            d.start_at = s;
        }
        if let Some(e) = c.end_at {
            d.end_at = e;
        }
        if let Some(z) = &c.time_zone {
            d.start_tz = Some(z.clone());
            d.end_tz = Some(z.clone());
        }
        if c.retimes() && d.kind == "occurrence" {
            d.kind = "exception".into();
        }
    }
    if let Some(a) = &c.attendees {
        d.attendees = to_attendees(a, &[]);
    }
    if let Some(s) = &c.show_as {
        d.show_as.clone_from(s);
    }
    if c.no_reminder {
        d.reminder_minutes = None;
    } else if let Some(m) = c.reminder_minutes {
        d.reminder_minutes = Some(m);
    }
    if let Some(cats) = &c.categories {
        d.categories.clone_from(cats);
    }
}

/// Makes an update's change locally.
pub fn apply_update(tx: &WriteTx<'_>, op: &EventUpdateOp) -> familiar_store::Result<()> {
    for e in rows(tx, op.event, op.series.as_deref())? {
        let mut d = e.data.clone();
        change_data(&mut d, &op.changes, op.series.is_some());
        tx.replace_event(e.id, &d)?;
    }
    Ok(())
}

/// Records an answer locally.
pub fn apply_respond(tx: &WriteTx<'_>, op: &EventRespondOp) -> familiar_store::Result<()> {
    for e in rows(tx, op.event, op.series.as_deref())? {
        let mut d = e.data.clone();
        d.response = Some(op.response.response().into());
        tx.replace_event(e.id, &d)?;
    }
    Ok(())
}

/// Removes deleted (or declined) rows once the server has the change.
pub fn finish_delete(
    tx: &WriteTx<'_>,
    event: i64,
    series: Option<&str>,
) -> familiar_store::Result<()> {
    for e in rows(tx, event, series)? {
        tx.delete_event(e.id)?;
    }
    tx.delete_event(EventId(event))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Reading times

/// A date and maybe a time, as typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct When {
    /// The date.
    pub date: NaiveDate,
    /// The time, if one was given.
    pub time: Option<chrono::NaiveTime>,
}

/// Reads a time of day: `18:00`, `6pm`, `6:30 pm`, `noon`.
pub fn parse_time(s: &str) -> Option<chrono::NaiveTime> {
    let s = s
        .trim()
        .to_ascii_lowercase()
        .replace(' ', "")
        .replace('.', ":");
    match s.as_str() {
        "noon" | "midday" => return chrono::NaiveTime::from_hms_opt(12, 0, 0),
        "midnight" => return chrono::NaiveTime::from_hms_opt(0, 0, 0),
        _ => {}
    }
    let (num, pm) = if let Some(n) = s.strip_suffix("pm") {
        (n, Some(true))
    } else if let Some(n) = s.strip_suffix("am") {
        (n, Some(false))
    } else {
        (s.as_str(), None)
    };
    let (h, m) = match num.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None if pm.is_some() => (num.parse::<u32>().ok()?, 0),
        None if num.len() == 4 && num.is_ascii() => {
            (num[..2].parse().ok()?, num[2..].parse().ok()?)
        }
        None => return None,
    };
    let h = match pm {
        Some(true) if (1..12).contains(&h) => h + 12,
        Some(false) if h == 12 => 0,
        Some(_) if !(1..=12).contains(&h) => return None,
        _ => h,
    };
    chrono::NaiveTime::from_hms_opt(h, m, 0)
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn month_of(s: &str) -> Option<u32> {
    let s = s.to_ascii_lowercase();
    let s = s.trim_end_matches('.');
    if s.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|m| s.starts_with(m))
        .filter(|&i| {
            let full = [
                "january",
                "february",
                "march",
                "april",
                "may",
                "june",
                "july",
                "august",
                "september",
                "october",
                "november",
                "december",
            ][i];
            full.starts_with(s) || (s == "sept" && i == 8)
        })
        .map(|i| i as u32 + 1)
}

fn day_number(s: &str) -> Option<u32> {
    let s = s.to_ascii_lowercase();
    let s = s
        .trim_end_matches(',')
        .trim_end_matches("st")
        .trim_end_matches("nd")
        .trim_end_matches("rd")
        .trim_end_matches("th");
    s.parse().ok().filter(|d| (1..=31).contains(d))
}

/// The date `today` or later with month `m` and day `d`, or in `year`.
fn date_from(today: NaiveDate, year: Option<i32>, m: u32, d: u32) -> Option<NaiveDate> {
    match year {
        Some(y) => NaiveDate::from_ymd_opt(y, m, d),
        None => {
            let this = NaiveDate::from_ymd_opt(today.year(), m, d)?;
            if this >= today - chrono::Duration::days(1) {
                Some(this)
            } else {
                NaiveDate::from_ymd_opt(today.year() + 1, m, d)
            }
        }
    }
}

/// Reads a date: `today`, `tomorrow`, a weekday (`sat`: the next one,
/// today included), `2026-11-21`, `21/11/2026` (day first), `21 Nov`,
/// `Nov 21`, with an optional year.
pub fn parse_date(words: &[&str], today: NaiveDate) -> Option<(NaiveDate, usize)> {
    let w0 = words.first()?.trim_end_matches(',').to_ascii_lowercase();
    match w0.as_str() {
        "today" | "tonight" | "tonite" => return Some((today, 1)),
        "tomorrow" => return Some((today + chrono::Duration::days(1), 1)),
        _ => {}
    }
    if let Ok(d) = NaiveDate::parse_from_str(&w0, "%Y-%m-%d") {
        return Some((d, 1));
    }
    let parts: Vec<&str> = w0.split('/').collect();
    if parts.len() == 3 || parts.len() == 2 {
        let d = parts[0].parse().ok()?;
        let m = parts[1].parse().ok()?;
        let y = parts
            .get(2)
            .and_then(|y| y.parse::<i32>().ok())
            .map(|y| if y < 100 { 2000 + y } else { y });
        return date_from(today, y, m, d).map(|x| (x, 1));
    }
    let year_at = |i: usize| {
        words
            .get(i)
            .and_then(|y| y.trim_end_matches([',', '.']).parse::<i32>().ok())
            .filter(|y| (2000..=2100).contains(y))
    };
    // A weekday, maybe followed by a date (`Saturday 21 November`).
    if let Some(day) = parse_weekday(&w0) {
        if let Some((d, n)) = parse_date(&words[1..], today) {
            return Some((d, n + 1));
        }
        let want = [
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
        ]
        .iter()
        .position(|d| *d == day)? as i64;
        let ahead = (want - i64::from(today.weekday().num_days_from_monday())).rem_euclid(7);
        return Some((today + chrono::Duration::days(ahead), 1));
    }
    if let (Some(d), Some(m)) = (day_number(&w0), words.get(1).and_then(|w| month_of(w))) {
        let y = year_at(2);
        return date_from(today, y, m, d).map(|x| (x, 2 + usize::from(y.is_some())));
    }
    if let (Some(m), Some(d)) = (month_of(&w0), words.get(1).and_then(|w| day_number(w))) {
        let y = year_at(2);
        return date_from(today, y, m, d).map(|x| (x, 2 + usize::from(y.is_some())));
    }
    None
}

/// Reads `--start`/`--end`: a date, a time, or both (`2026-11-21 18:00`,
/// `sat 6pm`, `tomorrow 9:30am`, `21/11 18:00`). A time alone is today
/// (or `on`, when given).
pub fn parse_when(s: &str, today: NaiveDate, on: Option<NaiveDate>) -> Option<When> {
    let s = s.trim().replace('T', " ");
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    if let Some((date, n)) = parse_date(&words, today) {
        let rest = words[n..].join(" ");
        let rest = rest.trim().trim_start_matches("at ").trim();
        if rest.is_empty() {
            return Some(When { date, time: None });
        }
        return parse_time(rest).map(|t| When {
            date,
            time: Some(t),
        });
    }
    parse_time(&words.join(" ")).map(|t| When {
        date: on.unwrap_or(today),
        time: Some(t),
    })
}

/// What an email suggests for an event made from it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Suggestion {
    /// The subject without `RE:`/`FW:`.
    pub subject: String,
    /// The first date found.
    pub date: Option<NaiveDate>,
    /// A time found next to it.
    pub time: Option<chrono::NaiveTime>,
    /// A `Where:`/`Location:`/`Venue:` line, or the place after the time
    /// on a ticket-style `•`-separated line.
    pub location: Option<String>,
}

/// Strips reply and forward prefixes.
pub fn bare_subject(s: &str) -> String {
    let mut s = s.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        match [
            "re:",
            "fw:",
            "fwd:",
            "event reminder:",
            "reminder:",
            "invitation:",
            "updated invitation:",
            "save the date:",
        ]
        .iter()
        .find(|p| lower.starts_with(*p))
        {
            Some(p) => s = s[p.len()..].trim_start(),
            None => return s.to_string(),
        }
    }
}

/// Looks through an email's subject and text for a date, time and place.
pub fn suggest(subject: &str, text: &str, today: NaiveDate) -> Suggestion {
    let mut out = Suggestion {
        subject: bare_subject(subject),
        ..Suggestion::default()
    };
    // The new part of the message first; quoted history (reply headers
    // carry their own dates, old mail its own venues) only when it has
    // nothing.
    let (own, quoted) = split_quoted(text);
    out.location = keyed_location(&own)
        .or_else(|| listed_location(&own))
        // The whole text, so `> Location: ...` quotes count too.
        .or_else(|| keyed_location(text));
    if let Some((date, time)) =
        find_when(&format!("{subject}\n{own}"), today).or_else(|| find_when(&quoted, today))
    {
        out.date = Some(date);
        out.time = time;
    }
    out
}

/// A place given by a label (`Where: ...`, `Venue: ...`) or a phrase
/// (`held at ...`), up to the end of its line or sentence.
fn keyed_location(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    for key in [
        "where:",
        "location:",
        "venue:",
        "place:",
        "held at ",
        "take place at ",
        "takes place at ",
    ] {
        let Some(i) = lower
            .match_indices(key)
            .map(|(i, _)| i)
            .find(|&i| i == 0 || lower[..i].ends_with(|c: char| c.is_whitespace() || c == '>'))
        else {
            continue;
        };
        let rest = &text[i + key.len()..];
        let line = rest.lines().next().unwrap_or("");
        let end = line
            .find(". ")
            .or_else(|| line.find("  "))
            .unwrap_or(line.len());
        let v = line[..end].trim().trim_end_matches('.').trim();
        // "held at 6pm" is a time, not a place; a labelled field such as
        // `Location: 1200 Main Street` is taken as written.
        let prose = key.ends_with(' ');
        if !v.is_empty() && !(prose && starts_with_time(v)) {
            return Some(v.to_string());
        }
    }
    None
}

/// True when `s` opens with a time of day (`6pm on Friday`, `18:00`).
fn starts_with_time(s: &str) -> bool {
    let words: Vec<&str> = s.split_whitespace().collect();
    (1..=words.len().min(2)).any(|n| parse_time(&words[..n].join(" ")).is_some())
}

/// A place from a ticket-style line that lists the details between
/// separators, with the place after the time:
/// `Fri 6 Nov • 20:00 Wolfbrook Arena • Christchurch` gives
/// `Wolfbrook Arena, Christchurch`.
fn listed_location(text: &str) -> Option<String> {
    const SEPS: [char; 3] = ['•', '|', '·'];
    for line in text.lines() {
        let parts: Vec<&str> = line
            .split(|c| SEPS.contains(&c))
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() < 2 {
            continue;
        }
        for (i, part) in parts.iter().enumerate() {
            let words: Vec<&str> = part.split_whitespace().collect();
            // The longest leading run of words that reads as a time.
            let Some(n) = (1..=words.len().min(2))
                .rev()
                .find(|&n| parse_time(&words[..n].join(" ")).is_some())
            else {
                continue;
            };
            let mut place: Vec<String> = Vec::new();
            // `20:00 - 22:00`: the end time is part of the time too.
            let mut n = n;
            if let Some(sep) = words.get(n) {
                if ["-", "–", "—", "to", "until"].contains(&sep.to_ascii_lowercase().as_str())
                    && starts_with_time(&words[n + 1..].join(" "))
                {
                    let rest = &words[n + 1..];
                    n += 1
                        + (1..=rest.len().min(2))
                            .rev()
                            .find(|&k| parse_time(&rest[..k].join(" ")).is_some())
                            .unwrap_or(0);
                }
            }
            let here = words[n..].join(" ");
            let here = here.trim_start_matches("at ").trim();
            // Whatever is left must read as a name, not more times.
            if !here.is_empty()
                && here.starts_with(char::is_alphanumeric)
                && !starts_with_time(here)
            {
                place.push(here.to_string());
            }
            // A short follow-on part without digits is the town.
            for next in &parts[i + 1..] {
                let short = next.split_whitespace().count() <= 4;
                if !short || next.chars().any(|c| c.is_ascii_digit()) || place.len() >= 2 {
                    break;
                }
                place.push((*next).to_string());
            }
            if !place.is_empty() {
                return Some(place.join(", "));
            }
        }
    }
    None
}

/// Splits an email's text into what the sender wrote and the quoted
/// history below it, dropping the reply-header lines (`From:`/`Sent:`,
/// `On ... wrote:`, `-----Original Message-----`) and `>` quotes.
fn split_quoted(text: &str) -> (String, String) {
    let lines: Vec<&str> = text.lines().collect();
    let header = |l: &str| {
        let t = l.trim().to_ascii_lowercase();
        ["from:", "sent:", "date:", "to:", "cc:", "subject:"]
            .iter()
            .any(|k| t.starts_with(k))
    };
    let starts_history = |i: usize| {
        let t = lines[i].trim().to_ascii_lowercase();
        if t.starts_with("-----original message")
            || t.starts_with("-------- forwarded")
            || t.starts_with("---------- forwarded")
        {
            return true;
        }
        if t.starts_with("on ") {
            let next = lines
                .get(i + 1)
                .map(|l| l.trim().to_ascii_lowercase())
                .unwrap_or_default();
            return t.ends_with("wrote:") || next.ends_with("wrote:");
        }
        // Outlook: From: then Sent: (or Date:) within a few lines.
        t.starts_with("from:")
            && lines[i + 1..].iter().take(4).any(|l| {
                let l = l.trim().to_ascii_lowercase();
                l.starts_with("sent:") || l.starts_with("date:")
            })
    };
    let cut = (0..lines.len())
        .find(|&i| starts_history(i))
        .unwrap_or(lines.len());
    let keep = |ls: &[&str]| {
        ls.iter()
            .filter(|l| !l.trim_start().starts_with('>'))
            .filter(|l| !header(l))
            .filter(|l| {
                let t = l.trim().to_ascii_lowercase();
                !(t.starts_with("on ") && t.ends_with("wrote:")) && !t.ends_with("wrote:")
            })
            .copied()
            .collect::<Vec<_>>()
            .join("\n")
    };
    (keep(&lines[..cut]), keep(&lines[cut..]))
}

/// The first date in `all` that comes with a time or is clearly a date,
/// and the time near it.
fn find_when(all: &str, today: NaiveDate) -> Option<(NaiveDate, Option<chrono::NaiveTime>)> {
    let chars: Vec<char> = all.chars().collect();
    let clean: String = chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            // `Tomorrow/Friday` is two words; `21/11` is a date.
            let letter = |j: Option<usize>| {
                j.and_then(|j| chars.get(j))
                    .is_some_and(|c| c.is_alphabetic())
            };
            if c == '/' && (letter(i.checked_sub(1)) || letter(Some(i + 1))) {
                ' '
            } else if c.is_alphanumeric() || "/:-.".contains(c) {
                c
            } else {
                ' '
            }
        })
        .collect();
    let words: Vec<&str> = clean.split_whitespace().collect();
    for i in 0..words.len() {
        // A bare weekday or day number is too weak alone: needs a month,
        // a full date form, or a time right after.
        let Some((date, n)) = parse_date(&words[i..], today) else {
            continue;
        };
        let w = words[i].to_ascii_lowercase();
        let strong = n > 1
            || w.contains('/')
            || w.contains('-')
            || matches!(w.as_str(), "today" | "tonight" | "tonite" | "tomorrow");
        let word = |k: usize| {
            words
                .get(k)
                .map(|w| w.trim_end_matches(['.', ',']).to_ascii_lowercase())
        };
        let time_at = |k: usize| {
            let a = word(k)?;
            let b = word(k + 1).unwrap_or_default();
            if b == "am" || b == "pm" {
                return parse_time(&format!("{a}{b}"));
            }
            if a.contains(':') || a.ends_with("am") || a.ends_with("pm") {
                return parse_time(&a);
            }
            None
        };
        // A few words after the date, else a few before (`6pm tonight`).
        let time = (0..6)
            .find_map(|k| time_at(i + n + k))
            .or_else(|| (1..=3).filter_map(|k| i.checked_sub(k)).find_map(time_at))
            // Else the next sentence or so (`tonight ... The session will
            // begin at 6:00pm`), unless another date comes first.
            .or_else(|| {
                (6..24)
                    .take_while(|k| {
                        words
                            .get(i + n + k)
                            .is_some_and(|_| parse_date(&words[i + n + k..], today).is_none())
                    })
                    .find_map(|k| time_at(i + n + k))
            });
        if !strong && time.is_none() {
            continue;
        }
        return Some((date, time));
    }
    None
}

/// The instant a local date and time in `iana` stands for.
pub fn instant(date: NaiveDate, time: chrono::NaiveTime, iana: &str) -> i64 {
    tz::local_to_utc(date.and_time(time), zone(iana)).timestamp()
}

/// Midnight UTC of a date (how all-day times are kept).
pub fn date_secs(d: NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc()
        .timestamp()
}

/// Today in a zone.
pub fn today_in(now: i64, iana: &str) -> NaiveDate {
    DateTime::<Utc>::from_timestamp(now, 0)
        .unwrap_or_default()
        .with_timezone(&zone(iana))
        .date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn times_and_dates() {
        let t = |h, m| chrono::NaiveTime::from_hms_opt(h, m, 0);
        assert_eq!(parse_time("6pm"), t(18, 0));
        assert_eq!(parse_time("6:30 PM"), t(18, 30));
        assert_eq!(parse_time("12am"), t(0, 0));
        assert_eq!(parse_time("18:05"), t(18, 5));
        assert_eq!(parse_time("noon"), t(12, 0));
        assert_eq!(parse_time("13pm"), None);
        let today = d(2026, 11, 5); // a Thursday
        let w = |s| parse_when(s, today, None);
        assert_eq!(w("2026-11-21 18:00").unwrap().date, d(2026, 11, 21));
        assert_eq!(w("2026-11-21T18:00").unwrap().time, t(18, 0));
        assert_eq!(w("sat 6pm").unwrap().date, d(2026, 11, 7));
        assert_eq!(w("thursday").unwrap().date, today);
        assert_eq!(w("21/11 9am").unwrap().date, d(2026, 11, 21));
        assert_eq!(w("2/1/27").unwrap().date, d(2027, 1, 2));
        assert_eq!(w("21 Nov 2026 at 6pm").unwrap().time, t(18, 0));
        assert_eq!(w("Nov 21").unwrap().date, d(2026, 11, 21));
        assert_eq!(w("3 Feb").unwrap().date, d(2027, 2, 3));
        assert_eq!(
            w("7:15pm").unwrap(),
            When {
                date: today,
                time: t(19, 15)
            }
        );
        assert!(w("whenever").is_none());
    }

    #[test]
    fn suggestions_from_an_email() {
        let today = d(2026, 11, 5);
        let s = suggest(
            "RE: Club prizegiving",
            "Hi all,\nThe prizegiving is on Saturday 21 November at 6:30 pm.\nWhere: Parklands hall\nThanks",
            today,
        );
        assert_eq!(s.subject, "Club prizegiving");
        assert_eq!(
            bare_subject("Event Reminder: Flight of the Conchords"),
            "Flight of the Conchords"
        );
        assert_eq!(s.date, Some(d(2026, 11, 21)));
        assert_eq!(s.time, chrono::NaiveTime::from_hms_opt(18, 30, 0));
        assert_eq!(s.location.as_deref(), Some("Parklands hall"));
        // Ticket-style lines put the place after the time.
        let s = suggest(
            "Event Reminder: Flight of the Conchords",
            "Your event is coming up\nFri 6 Nov • 20:00 Wolfbrook Arena • Christchurch\nDoors open 7pm",
            today,
        );
        assert_eq!(s.location.as_deref(), Some("Wolfbrook Arena, Christchurch"));
        assert_eq!(s.date, Some(d(2026, 11, 6)));
        assert_eq!(
            suggest("Hi", "Sat 7 Nov | 7:30 pm | Town Hall", today)
                .location
                .as_deref(),
            Some("Town Hall")
        );
        assert_eq!(
            suggest(
                "Masterclass",
                "Hi all\nThe session will be held at the St Andrew's covered nets. Bring your kit.",
                today
            )
            .location
            .as_deref(),
            Some("the St Andrew's covered nets")
        );
        assert_eq!(
            suggest(
                "Concert",
                "Thu 12 Nov\n20:00 Wolfbrook Arena • Christchurch",
                today
            )
            .location
            .as_deref(),
            Some("Wolfbrook Arena, Christchurch")
        );
        // The current message's venue wins over quoted history.
        assert_eq!(
            suggest(
                "RE: Event",
                "The event takes place at New Venue. See you.\n\nFrom: Club\nSent: Monday, 2 November 2026 9:00 am\nSubject: Event\n\nThe event was held at Old Venue. Thanks",
                today
            )
            .location
            .as_deref(),
            Some("New Venue")
        );
        // A venue only in a `>` quote still counts when nothing new has one.
        assert_eq!(
            suggest(
                "RE: Quiz",
                "Count me in\n\n> Quiz night Sat 7 Nov 7pm\n> Location: Town Hall",
                today
            )
            .location
            .as_deref(),
            Some("Town Hall")
        );
        // A labelled address keeps its street number.
        assert_eq!(
            suggest("Hi", "Location: 1200 Main Street\nSee you", today)
                .location
                .as_deref(),
            Some("1200 Main Street")
        );
        // A time after "held at" isn't a place.
        assert_eq!(
            suggest("Hi", "The session will be held at 6pm on Friday.", today).location,
            None
        );
        // Emoji and other non-ASCII words don't trip the time parser.
        assert_eq!(
            suggest("Hi", "📍 Town Hall | Christchurch", today).location,
            None
        );
        assert_eq!(parse_time("📍"), None);
        // A time range isn't part of the place.
        assert_eq!(
            suggest("Hi", "Fri 6 Nov • 20:00 - 22:00 • Town Hall", today)
                .location
                .as_deref(),
            Some("Town Hall")
        );
        assert_eq!(
            suggest(
                "Hi",
                "Fri 6 Nov • 7pm to 9 pm Town Hall • Christchurch",
                today
            )
            .location
            .as_deref(),
            Some("Town Hall, Christchurch")
        );
        assert_eq!(
            suggest("Hi", "Fri 6 Nov • 20:00 –22:00 • Town Hall", today)
                .location
                .as_deref(),
            Some("Town Hall")
        );
        // A separator line with no time gives nothing.
        assert_eq!(
            suggest("Hi", "Home • About • Contact", today).location,
            None
        );
        let s = suggest("Swimming", "Lessons start 12/1/2027, see you then", today);
        assert_eq!(s.date, Some(d(2027, 1, 12)));
        assert_eq!(s.time, None);
        // A lone weekday with no time isn't taken.
        assert_eq!(suggest("Hi", "Have a good Monday", today).date, None);
        // Reply headers below carry dates; the event's own words win.
        let s = suggest(
            "RE: Spin Bowling Masterclass",
            "Reminder: the masterclass is tonight, starting 6:00pm at the nets.\n\nFrom: Coach\nSent: Monday, 19 October 2026 2:09 pm\nTo: Marc\nSubject: Spin Bowling Masterclass\n\nSee you there",
            today,
        );
        assert_eq!(s.date, Some(today));
        assert_eq!(s.time, chrono::NaiveTime::from_hms_opt(18, 0, 0));
        let s = suggest(
            "Re: Round 2",
            "Thanks, see the draw attached.\n\nOn Wed, Nov 4, 2026 at 4:37 PM Jane <jane@club.example> wrote:\n> Draw coming",
            today,
        );
        assert_eq!(s.date, None, "{s:?}");
        // Only quoted history: the original's date still counts, headers don't.
        let s = suggest(
            "FW: Prizegiving",
            "-----Original Message-----\nFrom: Club\nSent: Monday, 19 October 2026 2:09 pm\nPrizegiving on Saturday 21 November at 6:30 pm",
            today,
        );
        assert_eq!(s.date, Some(d(2026, 11, 21)));
        assert_eq!(
            suggest("Hi", "6pm tonight at the club", today).time,
            chrono::NaiveTime::from_hms_opt(18, 0, 0)
        );
        let s = suggest(
            "Masterclass",
            "Hi there, Thanks again for registering for the Masterclass tonight with Ish Sodhi. The session will begin at 6:00pm sharp. Please wear appropriate activewear",
            today,
        );
        assert_eq!(
            (s.date, s.time),
            (Some(today), chrono::NaiveTime::from_hms_opt(18, 0, 0))
        );
        // Sent on a Thursday: "tomorrow" is the Friday.
        let thu = d(2026, 11, 5);
        let s = suggest("P.E. Tomorrow/Friday", "Please bring your P.E. gear.", thu);
        assert_eq!(s.date, Some(d(2026, 11, 6)));
        // An Outlook reply flattened from HTML keeps its lines now, so the
        // quoted header is skipped.
        let text = familiar_search::text::html_to_lines(
            "<div>The masterclass is tonight, starting at the nets at 6:00pm.</div><hr><div><b>From:</b> Coach<br><b>Sent:</b> Monday, 19 October 2026 2:09 pm</div>",
        );
        let s = suggest("RE: Masterclass", &text, today);
        assert_eq!(
            (s.date, s.time),
            (Some(today), chrono::NaiveTime::from_hms_opt(18, 0, 0))
        );
    }

    #[test]
    fn graph_bodies() {
        let start = instant(
            d(2026, 11, 21),
            chrono::NaiveTime::from_hms_opt(18, 0, 0).unwrap(),
            "Pacific/Auckland",
        );
        let f = EventFields {
            subject: "Prizegiving".into(),
            start_at: start,
            end_at: start + 5400,
            time_zone: "Pacific/Auckland".into(),
            recurrence: Some(Recurrence {
                every: RepeatKind::Weekly,
                interval: 1,
                days: vec![],
                until: None,
                count: Some(4),
            }),
            ..EventFields::default()
        };
        let v = event_json(&f, "t1", None);
        assert_eq!(v["start"]["dateTime"], "2026-11-21T18:00:00");
        assert_eq!(v["start"]["timeZone"], "New Zealand Standard Time");
        assert_eq!(v["end"]["dateTime"], "2026-11-21T19:30:00");
        assert_eq!(v["recurrence"]["pattern"]["daysOfWeek"][0], "saturday");
        assert_eq!(v["recurrence"]["range"]["numberOfOccurrences"], 4);
        assert_eq!(v["recurrence"]["range"]["startDate"], "2026-11-21");
        let all = EventFields {
            all_day: true,
            start_at: date_secs(d(2026, 12, 24)),
            end_at: date_secs(d(2026, 12, 27)),
            time_zone: "Pacific/Auckland".into(),
            ..EventFields::default()
        };
        let v = event_json(&all, "t2", None);
        assert_eq!(v["start"]["dateTime"], "2026-12-24T00:00:00");
        assert_eq!(v["end"]["dateTime"], "2026-12-27T00:00:00");
        assert_eq!(
            when_text(all.start_at, all.end_at, true, "Pacific/Auckland"),
            "Thu 24 Dec to Sat 26 Dec 2026 (all day)"
        );
        assert_eq!(parse_weekday("Tues"), Some("tuesday"));
        assert_eq!(parse_weekday("x"), None);
    }
}
