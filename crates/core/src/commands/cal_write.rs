// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar writes (WP 2.4): `fam cal add`, `edit`, `delete` and `rsvp`.
//!
//! Like mail writes, each one changes this device's calendar at once and is
//! queued for the server; see [`crate::events`] and [`crate::ops`].
//!
//! Times are read in `--tz` (default: this computer's zone): `--start
//! "2026-11-21 18:00"`, `--start "sat 6pm"`, `--start tomorrow` (a date
//! alone makes an all-day event). `--calendar` takes a calendar name or a
//! purpose from `[calendar.purposes]`; `--purpose` takes only a purpose.
//! `--from-mail` fills the subject, date, time and place from an email
//! where it can, and links the email in the event.

use chrono::{Duration, NaiveDate};
use familiar_config::Config;
use familiar_store::{EventId, MessageId, Reader};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::commands::auth::config;
use crate::commands::write::{WRITE_ERRORS, WriteFlags, flags, need_yes, submit_at};
use crate::compose::{Recipient, text_to_html};
use crate::daemon::DaemonPaths;
use crate::error::{CommandError, ErrorKind};
use crate::events::{
    self, EventChanges, EventFields, Recurrence, RepeatKind, Rsvp, date_secs, instant, parse_date,
    parse_weekday, parse_when, today_in,
};
use crate::ops::{WriteOutput, WriteRequest};
use crate::registry::{Access, Command, CommandSpec, Context};
use crate::tz;

/// More invitees than this need `--yes`.
pub const INVITE_LIMIT: usize = 10;

const SHOW_AS: [&str; 5] = ["free", "tentative", "busy", "oof", "workingElsewhere"];

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn store_err(e: familiar_store::Error) -> CommandError {
    CommandError::internal(format!("store: {e}"))
}

pub(crate) fn open(paths: &DaemonPaths) -> Result<Reader, CommandError> {
    match Reader::open(&paths.store) {
        Ok(r) => Ok(r),
        Err(familiar_store::Error::NoStore(_)) => Err(CommandError::new(
            ErrorKind::NotFound,
            "nothing has synced on this device yet; start famd first",
        )),
        Err(e) => Err(store_err(e)),
    }
}

/// The IANA zone `--tz` names, or this computer's.
pub fn zone_name(tz_arg: Option<&str>) -> Result<String, CommandError> {
    match tz_arg {
        None => Ok(tz::local_zone().name().to_string()),
        Some(n) => tz::parse_zone(n)
            .map(|z| z.name().to_string())
            .ok_or_else(|| {
                CommandError::usage(format!("--tz {n:?}: not an IANA or Windows time zone"))
            }),
    }
}

fn show_as(v: Option<&str>) -> Result<Option<String>, CommandError> {
    match v {
        None => Ok(None),
        Some(s) => SHOW_AS
            .iter()
            .find(|x| x.eq_ignore_ascii_case(s))
            .map(|x| Some((*x).to_string()))
            .ok_or_else(|| {
                CommandError::usage(format!("--show-as {s:?}: use {}", SHOW_AS.join(", ")))
            }),
    }
}

/// Resolves `--calendar` / `--purpose` into an account and calendar name.
fn target(
    cfg: Option<&Config>,
    calendar: Option<&str>,
    purpose: Option<&str>,
    account: Option<&str>,
) -> Result<(Option<String>, Option<String>), CommandError> {
    let purposes = cfg.map(|c| &c.calendar.purposes);
    if let Some(p) = purpose {
        let found = purposes.and_then(|ps| ps.get(p)).ok_or_else(|| {
            let names: Vec<&str> = purposes
                .map(|ps| ps.keys().map(String::as_str).collect())
                .unwrap_or_default();
            CommandError::new(
                ErrorKind::Config,
                if names.is_empty() {
                    format!("no purpose {p:?}: add one under [calendar.purposes] in config.toml")
                } else {
                    format!("no purpose {p:?}; purposes: {}", names.join(", "))
                },
            )
        })?;
        return Ok((Some(found.account.clone()), Some(found.calendar.clone())));
    }
    if let Some(c) = calendar {
        if let Some(found) = purposes.and_then(|ps| ps.get(c)) {
            return Ok((Some(found.account.clone()), Some(found.calendar.clone())));
        }
    }
    Ok((account.map(str::to_string), calendar.map(str::to_string)))
}

fn when_err(flag: &str, v: &str) -> CommandError {
    CommandError::usage(format!(
        "{flag} {v:?}: use a date and time like \"2026-11-21 18:00\", \"sat 6pm\" or \"tomorrow 9:30am\", or a date alone for all day"
    ))
}

fn recurrence(
    repeat: Option<&str>,
    every: Option<u32>,
    on: &[String],
    until: Option<&str>,
    count: Option<u32>,
    today: NaiveDate,
) -> Result<Option<Recurrence>, CommandError> {
    let Some(kind) = repeat else {
        if every.is_some() || !on.is_empty() || until.is_some() || count.is_some() {
            return Err(CommandError::usage(
                "--every, --on, --until and --count need --repeat",
            ));
        }
        return Ok(None);
    };
    let every_kind = match kind.to_ascii_lowercase().as_str() {
        "daily" | "day" => RepeatKind::Daily,
        "weekly" | "week" => RepeatKind::Weekly,
        "monthly" | "month" => RepeatKind::Monthly,
        "yearly" | "year" | "annually" => RepeatKind::Yearly,
        _ => {
            return Err(CommandError::usage(format!(
                "--repeat {kind:?}: use daily, weekly, monthly or yearly"
            )));
        }
    };
    let mut days = Vec::new();
    for d in on.iter().flat_map(|s| s.split(',')) {
        if d.trim().is_empty() {
            continue;
        }
        days.push(
            parse_weekday(d)
                .ok_or_else(|| CommandError::usage(format!("--on {d:?}: not a day of the week")))?
                .to_string(),
        );
    }
    if !days.is_empty() && every_kind != RepeatKind::Weekly {
        return Err(CommandError::usage("--on only goes with --repeat weekly"));
    }
    let until = match until {
        None => None,
        Some(u) => {
            let words: Vec<&str> = u.split_whitespace().collect();
            let (d, _) = parse_date(&words, today).ok_or_else(|| when_err("--until", u))?;
            Some(d.format("%Y-%m-%d").to_string())
        }
    };
    if until.is_some() && count.is_some() {
        return Err(CommandError::usage("give --until or --count, not both"));
    }
    if count == Some(0) {
        return Err(CommandError::usage("--count must be 1 or more"));
    }
    Ok(Some(Recurrence {
        every: every_kind,
        interval: every.unwrap_or(1),
        days,
        until,
        count,
    }))
}

// ---------------------------------------------------------------------------
// fam cal add

/// `fam cal add`.
pub struct CalAdd;

/// Input for `fam cal add`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CalAddInput {
    /// Subject. With --from-mail, the email's subject when left out.
    #[serde(default)]
    #[schemars(extend("x-positional" = 0))]
    pub subject: Option<String>,
    /// Start: "2026-11-21 18:00", "sat 6pm", "tomorrow 9:30am", or a date
    /// alone for an all-day event. With --from-mail, found in the email when left out.
    #[serde(default)]
    pub start: Option<String>,
    /// End: a date and time, or a time on the start's day. All-day: the last day.
    #[serde(default)]
    pub end: Option<String>,
    /// Length in minutes when there's no --end (default 60).
    #[serde(default)]
    pub minutes: Option<u32>,
    /// All-day event (also when --start is a date alone).
    #[serde(default)]
    pub all_day: bool,
    /// All-day: how many days when there's no --end (default 1).
    #[serde(default)]
    pub days: Option<u32>,
    /// Calendar name ("Family Room") or a purpose from [calendar.purposes]
    /// ("family"). Default: the account's main calendar.
    #[serde(default)]
    pub calendar: Option<String>,
    /// A purpose from [calendar.purposes], such as family.
    #[serde(default)]
    pub purpose: Option<String>,
    /// Account (its name in config.toml), when --calendar doesn't settle it.
    #[serde(default)]
    pub account: Option<String>,
    /// Location.
    #[serde(default)]
    pub location: Option<String>,
    /// Notes (plain text).
    #[serde(default)]
    pub body: Option<String>,
    /// Invite someone ("Name <addr>" or an address); repeat or separate with commas.
    #[serde(default)]
    pub attendee: Vec<String>,
    /// Invite someone as optional.
    #[serde(default)]
    pub optional: Vec<String>,
    /// Reminder, minutes before the start (default 15).
    #[serde(default)]
    pub reminder: Option<i64>,
    /// No reminder.
    #[serde(default)]
    pub no_reminder: bool,
    /// Show as free, tentative, busy (the default), oof or workingElsewhere.
    #[serde(default)]
    pub show_as: Option<String>,
    /// Outlook category; repeat for more.
    #[serde(default)]
    pub category: Vec<String>,
    /// Repeat: daily, weekly, monthly or yearly.
    #[serde(default)]
    pub repeat: Option<String>,
    /// Repeat every this many days, weeks, months or years (default 1).
    #[serde(default)]
    pub every: Option<u32>,
    /// Weekly: the days, such as mon,wed (default the start's day).
    #[serde(default)]
    pub on: Vec<String>,
    /// Repeat until this date.
    #[serde(default)]
    pub until: Option<String>,
    /// Or: repeat this many times.
    #[serde(default)]
    pub count: Option<u32>,
    /// Time zone for the times (IANA or Windows). Default: this computer's.
    #[serde(default)]
    pub tz: Option<String>,
    /// Make it from this email (local message ID): suggests the subject,
    /// date, time and place, and links the email in the event.
    #[serde(default)]
    pub from_mail: Option<i64>,
    /// Go ahead with more than 10 invitees.
    #[serde(default)]
    pub yes: bool,
    /// Only show what would happen.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has it (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

/// The email's subject, text, when it arrived and its account's key.
pub(crate) fn mail_text(
    r: &Reader,
    id: i64,
) -> Result<(String, String, i64, String), CommandError> {
    let m = r
        .message(MessageId(id))
        .map_err(store_err)?
        .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("no message {id}")))?;
    let text = match &m.body {
        Some(b) => match r.body(&b.hash) {
            Ok(raw) => {
                let t = String::from_utf8_lossy(&raw).into_owned();
                if b.is_html {
                    familiar_search::text::html_to_lines(&t)
                } else {
                    t
                }
            }
            Err(_) => m.preview.clone(),
        },
        None => m.preview.clone(),
    };
    let key = r
        .accounts()
        .map_err(store_err)?
        .into_iter()
        .find(|a| a.id == m.account)
        .map(|a| a.key)
        .unwrap_or_default();
    Ok((m.subject, text, m.received_at, key))
}

/// Turns `fam cal add`'s input into a request. `r` is needed for
/// `--from-mail`; `zone` is IANA.
pub fn add_request(
    r: Option<&Reader>,
    cfg: Option<&Config>,
    i: &CalAddInput,
    zone: &str,
    now: i64,
) -> Result<WriteRequest, CommandError> {
    let today = today_in(now, zone);
    let mut mail_account = None;
    let suggestion = match (i.from_mail, r) {
        (Some(id), Some(r)) => {
            let (subject, text, at, key) = mail_text(r, id)?;
            mail_account = (!key.is_empty()).then_some(key);
            // "Tonight" and "Saturday" are from when it was sent.
            let sent = if at > 0 { today_in(at, zone) } else { today };
            Some(events::suggest(&subject, &text, sent))
        }
        (Some(_), None) => return Err(CommandError::internal("--from-mail needs the store")),
        _ => None,
    };
    let subject = i
        .subject
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| suggestion.as_ref().map(|s| s.subject.clone()))
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| CommandError::usage("give the event a subject"))?;
    let start = match &i.start {
        Some(s) => parse_when(s, today, None).ok_or_else(|| when_err("--start", s))?,
        None => match suggestion
            .as_ref()
            .and_then(|s| s.date.map(|d| (d, s.time)))
        {
            Some((date, time)) => events::When { date, time },
            None => {
                return Err(CommandError::usage(if i.from_mail.is_some() {
                    "the email doesn't say when; give --start"
                } else {
                    "give --start, such as \"2026-11-21 18:00\" or \"sat 6pm\""
                }));
            }
        },
    };
    let all_day = i.all_day || start.time.is_none();
    let (start_at, end_at) = if all_day {
        let first = date_secs(start.date);
        let end = match &i.end {
            Some(e) => {
                let words: Vec<&str> = e.split_whitespace().collect();
                let (last, _) = parse_date(&words, today).ok_or_else(|| when_err("--end", e))?;
                date_secs(last + Duration::days(1))
            }
            None => first + i64::from(i.days.unwrap_or(1).max(1)) * 86_400,
        };
        (first, end)
    } else {
        let t = start.time.expect("timed");
        let s = instant(start.date, t, zone);
        let e = match &i.end {
            Some(e) => {
                let w =
                    parse_when(e, today, Some(start.date)).ok_or_else(|| when_err("--end", e))?;
                let wt = w.time.ok_or_else(|| when_err("--end", e))?;
                instant(w.date, wt, zone)
            }
            None => s + i64::from(i.minutes.unwrap_or(60)) * 60,
        };
        (s, e)
    };
    let attendees = Recipient::parse_all(&i.attendee)?;
    let optional = Recipient::parse_all(&i.optional)?;
    let invited = attendees.len() + optional.len();
    if invited > INVITE_LIMIT && !i.yes && !i.dry_run {
        return Err(CommandError::new(
            ErrorKind::ConfirmationRequired,
            format!("this invites {invited} people; add --yes to go ahead"),
        ));
    }
    let body = i.body.as_deref().map(text_to_html).unwrap_or_default();
    let (mut account, calendar) = target(
        cfg,
        i.calendar.as_deref(),
        i.purpose.as_deref(),
        i.account.as_deref(),
    )?;
    // From an email with nothing else naming one: the email's account.
    if account.is_none() && calendar.is_none() {
        account = mail_account;
    }
    if i.reminder.is_some() && i.no_reminder {
        return Err(CommandError::usage(
            "give --reminder or --no-reminder, not both",
        ));
    }
    Ok(WriteRequest::EventCreate {
        account,
        calendar,
        from_mail: i.from_mail,
        event: EventFields {
            subject,
            start_at,
            end_at,
            all_day,
            time_zone: zone.to_string(),
            location: i
                .location
                .clone()
                .or_else(|| suggestion.as_ref().and_then(|s| s.location.clone()))
                .unwrap_or_default(),
            body_html: body,
            attendees,
            optional,
            show_as: show_as(i.show_as.as_deref())?,
            reminder_minutes: i.reminder,
            no_reminder: i.no_reminder,
            categories: i.category.clone(),
            recurrence: recurrence(
                i.repeat.as_deref(),
                i.every,
                &i.on,
                i.until.as_deref(),
                i.count,
                today,
            )?,
        },
    })
}

/// Runs `fam cal add` against the store and socket at `paths`.
pub fn add_at(
    paths: &DaemonPaths,
    cfg: Option<&Config>,
    input: &CalAddInput,
    zone: &str,
    now: i64,
    flags: &WriteFlags,
) -> Result<WriteOutput, CommandError> {
    let reader = match input.from_mail {
        Some(_) => Some(open(paths)?),
        None => None,
    };
    let req = add_request(reader.as_ref(), cfg, input, zone, now)?;
    submit_at(paths, req, flags)
}

impl Command for CalAdd {
    type Input = CalAddInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["cal", "add"],
        summary: "Add an event",
        description: "Adds an event to a calendar on this device at once and queues it for the server. --start takes \"2026-11-21 18:00\", \"sat 6pm\" or a date alone (all day), in --tz or this computer's zone; --end or --minutes (default 60) set the end. --calendar takes a calendar name or a purpose from [calendar.purposes]; --purpose takes a purpose (family). --repeat daily|weekly|monthly|yearly with --every, --on, --until or --count makes a series the server expands. --attendee sends invitations. --from-mail <id> suggests the subject, date, time and place from an email and links the email in the event.",
        access: Access::Write,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: CalAddInput) -> Result<WriteOutput, CommandError> {
        let zone = zone_name(i.tz.as_deref())?;
        let cfg = config()?;
        add_at(
            &DaemonPaths::resolve()?,
            cfg.as_ref(),
            &i,
            &zone,
            now(),
            &flags(i.dry_run, i.wait, i.wait_secs),
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

// ---------------------------------------------------------------------------
// fam cal edit

/// `fam cal edit`.
pub struct CalEdit;

/// Input for `fam cal edit`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CalEditInput {
    /// Local event ID (from fam cal list).
    #[schemars(extend("x-positional" = 0))]
    pub event: i64,
    /// New subject.
    #[serde(default)]
    pub subject: Option<String>,
    /// New start (as fam cal add takes it). The length stays unless --end or --minutes.
    #[serde(default)]
    pub start: Option<String>,
    /// New end.
    #[serde(default)]
    pub end: Option<String>,
    /// New length in minutes.
    #[serde(default)]
    pub minutes: Option<u32>,
    /// New location.
    #[serde(default)]
    pub location: Option<String>,
    /// New notes (plain text).
    #[serde(default)]
    pub body: Option<String>,
    /// New attendee list (replaces it); repeat for more.
    #[serde(default)]
    pub attendee: Vec<String>,
    /// New reminder, minutes before the start.
    #[serde(default)]
    pub reminder: Option<i64>,
    /// Turn the reminder off.
    #[serde(default)]
    pub no_reminder: bool,
    /// Show as free, tentative, busy, oof or workingElsewhere.
    #[serde(default)]
    pub show_as: Option<String>,
    /// New categories (replaces them); repeat for more.
    #[serde(default)]
    pub category: Vec<String>,
    /// Change every event in its series, not just this one.
    #[serde(default)]
    pub series: bool,
    /// Time zone for the times (IANA or Windows). Default: this computer's.
    #[serde(default)]
    pub tz: Option<String>,
    /// Go ahead with more than 10 attendees.
    #[serde(default)]
    pub yes: bool,
    /// Only show what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the change (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

/// Turns `fam cal edit`'s input into a request, reading the event from `r`.
pub fn edit_request(
    r: &Reader,
    i: &CalEditInput,
    zone: &str,
    now: i64,
) -> Result<WriteRequest, CommandError> {
    let e = r
        .event(EventId(i.event))
        .map_err(store_err)?
        .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("no event {}", i.event)))?;
    let d = &e.data;
    let today = today_in(now, zone);
    let mut c = EventChanges {
        subject: i.subject.clone(),
        location: i.location.clone(),
        body_html: i.body.as_deref().map(text_to_html),
        show_as: show_as(i.show_as.as_deref())?,
        reminder_minutes: i.reminder,
        no_reminder: i.no_reminder,
        categories: (!i.category.is_empty()).then(|| i.category.clone()),
        ..EventChanges::default()
    };
    if !i.attendee.is_empty() {
        let a = Recipient::parse_all(&i.attendee)?;
        if a.len() > INVITE_LIMIT && !i.yes && !i.dry_run {
            return Err(CommandError::new(
                ErrorKind::ConfirmationRequired,
                format!("this invites {} people; add --yes to go ahead", a.len()),
            ));
        }
        c.attendees = Some(a);
    }
    let length = d.end_at - d.start_at;
    if let Some(s) = &i.start {
        let w = parse_when(s, today, None).ok_or_else(|| when_err("--start", s))?;
        match w.time {
            None => {
                c.all_day = Some(true);
                c.start_at = Some(date_secs(w.date));
                let days = if d.is_all_day {
                    length.max(86_400)
                } else {
                    86_400
                };
                c.end_at = Some(date_secs(w.date) + days);
            }
            Some(t) => {
                c.all_day = Some(false);
                let st = instant(w.date, t, zone);
                c.start_at = Some(st);
                c.end_at = Some(st + if d.is_all_day { 3600 } else { length });
            }
        }
        c.time_zone = Some(zone.to_string());
    }
    let start_now = c.start_at.unwrap_or(d.start_at);
    let all_day_now = c.all_day.unwrap_or(d.is_all_day);
    if let Some(e) = &i.end {
        if all_day_now {
            let words: Vec<&str> = e.split_whitespace().collect();
            let (last, _) = parse_date(&words, today).ok_or_else(|| when_err("--end", e))?;
            c.end_at = Some(date_secs(last + Duration::days(1)));
        } else {
            let on = chrono::DateTime::from_timestamp(start_now, 0)
                .unwrap_or_default()
                .with_timezone(&tz::parse_zone(zone).unwrap_or(chrono_tz::UTC))
                .date_naive();
            let w = parse_when(e, today, Some(on)).ok_or_else(|| when_err("--end", e))?;
            let t = w.time.ok_or_else(|| when_err("--end", e))?;
            c.end_at = Some(instant(w.date, t, zone));
        }
        c.time_zone = Some(zone.to_string());
    } else if let Some(m) = i.minutes {
        if all_day_now {
            return Err(CommandError::usage(
                "--minutes is for timed events; use --end",
            ));
        }
        c.end_at = Some(start_now + i64::from(m) * 60);
        c.time_zone = Some(zone.to_string());
    }
    if c.end_at.is_some() && c.start_at.is_none() {
        c.start_at = Some(d.start_at);
    }
    Ok(WriteRequest::EventUpdate {
        event: i.event,
        series: i.series,
        changes: c,
    })
}

impl Command for CalEdit {
    type Input = CalEditInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["cal", "edit"],
        summary: "Change an event",
        description: "Changes an event on this device at once and queues the change for the server. Times are read as fam cal add reads them; a new --start keeps the event's length unless --end or --minutes is given. --series changes every event of a recurring series (one occurrence otherwise). Only the organiser can change the subject, times, place or attendees; attendees with changes get an update.",
        access: Access::Write,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: CalEditInput) -> Result<WriteOutput, CommandError> {
        let zone = zone_name(i.tz.as_deref())?;
        let paths = DaemonPaths::resolve()?;
        let req = edit_request(&open(&paths)?, &i, &zone, now())?;
        submit_at(&paths, req, &flags(i.dry_run, i.wait, i.wait_secs))
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

// ---------------------------------------------------------------------------
// fam cal delete

/// `fam cal delete`.
pub struct CalDelete;

/// Input for `fam cal delete`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CalDeleteInput {
    /// Local event ID (from fam cal list).
    #[schemars(extend("x-positional" = 0))]
    pub event: i64,
    /// Delete the whole recurring series.
    #[serde(default)]
    pub series: bool,
    /// For a meeting you organised: a note in the cancellation attendees get.
    #[serde(default)]
    pub comment: Option<String>,
    /// Go ahead with cancelling a meeting you organised (attendees are told).
    #[serde(default)]
    pub yes: bool,
    /// Only show what would happen.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has it (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

/// Turns `fam cal delete`'s input into a request; a meeting you organised
/// with attendees needs `--yes`.
pub fn delete_request(r: &Reader, i: &CalDeleteInput) -> Result<WriteRequest, CommandError> {
    let e = r
        .event(EventId(i.event))
        .map_err(store_err)?
        .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("no event {}", i.event)))?;
    if e.data.is_organizer && !e.data.attendees.is_empty() {
        need_yes(
            i.yes,
            i.dry_run,
            &format!(
                "cancelling {:?} tells its {} attendees and",
                e.data.subject,
                e.data.attendees.len()
            ),
        )?;
    }
    Ok(WriteRequest::EventDelete {
        event: i.event,
        series: i.series,
        comment: i
            .comment
            .clone()
            .or_else(|| (e.data.is_organizer && !e.data.attendees.is_empty()).then(String::new)),
    })
}

impl Command for CalDelete {
    type Input = CalDeleteInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["cal", "delete"],
        summary: "Delete an event, or cancel a meeting you organised",
        description: "Takes an event off this device's calendar at once and queues the delete for the server. --series deletes the whole recurring series. A meeting you organised with attendees is cancelled, which tells them (needs --yes; --comment adds a note). To turn down someone else's meeting, use fam cal rsvp decline.",
        access: Access::Delete,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: CalDeleteInput) -> Result<WriteOutput, CommandError> {
        let paths = DaemonPaths::resolve()?;
        let req = delete_request(&open(&paths)?, &i)?;
        submit_at(&paths, req, &flags(i.dry_run, i.wait, i.wait_secs))
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

// ---------------------------------------------------------------------------
// fam cal rsvp

/// `fam cal rsvp`.
pub struct CalRsvp;

/// Input for `fam cal rsvp`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CalRsvpInput {
    /// Local event ID (from fam cal list).
    #[schemars(extend("x-positional" = 0))]
    pub event: i64,
    /// accept, tentative or decline.
    #[schemars(extend("x-positional" = 1))]
    pub response: String,
    /// A note to the organiser.
    #[serde(default)]
    pub comment: Option<String>,
    /// Don't tell the organiser.
    #[serde(default)]
    pub no_reply: bool,
    /// Answer for the whole recurring series.
    #[serde(default)]
    pub series: bool,
    /// Only show what would happen.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has it (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

/// Reads `accept`, `tentative` or `decline`.
pub fn parse_rsvp(s: &str) -> Result<Rsvp, CommandError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "accept" | "yes" | "accepted" => Ok(Rsvp::Accept),
        "tentative" | "maybe" | "tentatively" => Ok(Rsvp::Tentative),
        "decline" | "no" | "declined" => Ok(Rsvp::Decline),
        _ => Err(CommandError::usage(format!(
            "{s:?}: answer accept, tentative or decline"
        ))),
    }
}

/// Turns `fam cal rsvp`'s input into a request.
pub fn rsvp_request(i: &CalRsvpInput) -> Result<WriteRequest, CommandError> {
    Ok(WriteRequest::EventRespond {
        event: i.event,
        series: i.series,
        response: parse_rsvp(&i.response)?,
        comment: i.comment.clone(),
        send_response: !i.no_reply,
    })
}

impl Command for CalRsvp {
    type Input = CalRsvpInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["cal", "rsvp"],
        summary: "Answer a meeting invitation",
        description: "Accepts, tentatively accepts or declines a meeting on this device at once and queues the answer for the server, which tells the organiser unless --no-reply. A declined meeting leaves the calendar. --series answers for the whole series.",
        access: Access::Write,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: CalRsvpInput) -> Result<WriteOutput, CommandError> {
        submit_at(
            &DaemonPaths::resolve()?,
            rsvp_request(&i)?,
            &flags(i.dry_run, i.wait, i.wait_secs),
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}
