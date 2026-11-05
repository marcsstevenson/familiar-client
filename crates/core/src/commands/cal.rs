// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam cal list` and `fam cal calendars` (WP 1.11): events and calendars
//! from the store.
//!
//! `famd` keeps each calendar's events for a window around today
//! (`[calendar] past` and `future`); these commands read the store directly,
//! so they work without the daemon and never touch the network.
//!
//! Times are shown in one zone: `--tz` (IANA or Windows name), otherwise
//! the machine's own. All-day events are dates, not instants, so they show
//! on their own dates in any zone.

use chrono::{DateTime, Datelike, Duration, NaiveDate};
use chrono_tz::Tz;
use familiar_config::Config;
use familiar_store::{Account, Calendar, CalendarId, EventRange, Reader};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::commands::auth::config;
use crate::daemon::DaemonPaths;
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use crate::tz;

/// `fam cal list`.
pub struct CalList;

/// Input for `fam cal list`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CalListInput {
    /// A calendar's name (such as "Family Room") or a purpose from
    /// `[calendar.purposes]` (such as "family"). All calendars if left out.
    #[serde(default)]
    pub calendar: Option<String>,
    /// Only this account (its name in config.toml).
    #[serde(default)]
    pub account: Option<String>,
    /// First day: `today` (the default), `tomorrow`, `yesterday` or a date like 2026-11-05.
    #[serde(default)]
    pub from: Option<String>,
    /// How many days to list, from 1 to 366. Default 7.
    #[serde(default)]
    pub days: Option<u32>,
    /// Time zone to show times in, IANA (Pacific/Auckland) or Windows
    /// (New Zealand Standard Time). Default: this computer's.
    #[serde(default)]
    pub tz: Option<String>,
}

/// One event in `fam cal list`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalEvent {
    /// Local event ID.
    pub id: i64,
    /// Account name.
    pub account: String,
    /// Calendar name.
    pub calendar: String,
    /// Subject.
    pub subject: String,
    /// Start: RFC 3339 in the output zone, or a date (YYYY-MM-DD) for all-day events.
    pub start: String,
    /// End: RFC 3339 in the output zone, or the day after the last day for all-day events.
    pub end: String,
    /// All-day event.
    pub all_day: bool,
    /// Location.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub location: String,
    /// Organiser's address.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub organizer: String,
    /// `free`, `tentative`, `busy`, `oof`, `workingElsewhere` or `unknown`.
    pub show_as: String,
    /// Cancelled meeting.
    pub cancelled: bool,
    /// Part of a recurring series.
    pub recurring: bool,
    /// Your response, when Graph gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    /// The organiser's time zone (IANA), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
}

/// Output of `fam cal list`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalListOutput {
    /// The zone times are shown in (IANA).
    pub time_zone: String,
    /// Start of the range (RFC 3339).
    pub from: String,
    /// End of the range, exclusive (RFC 3339).
    pub to: String,
    /// Calendars searched, as `account/calendar`.
    pub calendars: Vec<String>,
    /// Events, by start time.
    pub events: Vec<CalEvent>,
    /// Problems worth knowing, such as a calendar that has not synced yet
    /// or a range outside the synced window.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// `fam cal calendars`.
pub struct CalCalendars;

/// Input for `fam cal calendars`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CalCalendarsInput {
    /// Only this account (its name in config.toml).
    #[serde(default)]
    pub account: Option<String>,
}

/// One calendar in `fam cal calendars`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalCalendar {
    /// Account name.
    pub account: String,
    /// Calendar name.
    pub name: String,
    /// The account's default calendar.
    pub is_default: bool,
    /// You can add and change events.
    pub can_edit: bool,
    /// Owner's address.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
    /// Purposes from `[calendar.purposes]` that point here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub purposes: Vec<String>,
    /// Events on this device.
    pub events: i64,
    /// Start of the synced window (date), once a sync has run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synced_from: Option<String>,
    /// End of the synced window (date, exclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synced_to: Option<String>,
    /// When the events last synced (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<String>,
    /// The last sync error, if the last sync failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Output of `fam cal calendars`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CalCalendarsOutput {
    /// Calendars, by account, the default first.
    pub calendars: Vec<CalCalendar>,
}

fn store_err(e: familiar_store::Error) -> CommandError {
    CommandError::internal(format!("store: {e}"))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn open(paths: &DaemonPaths) -> Result<Reader, CommandError> {
    match Reader::open(&paths.store) {
        Ok(r) => Ok(r),
        Err(familiar_store::Error::NoStore(_)) => Err(CommandError::new(
            ErrorKind::NotFound,
            "nothing has synced on this device yet; start famd first",
        )),
        Err(e) => Err(store_err(e)),
    }
}

fn accounts(r: &Reader, only: Option<&str>) -> Result<Vec<Account>, CommandError> {
    let all = r.accounts().map_err(store_err)?;
    let picked: Vec<Account> = all
        .into_iter()
        .filter(|a| only.is_none_or(|k| a.key == k))
        .collect();
    if let (Some(k), true) = (only, picked.is_empty()) {
        return Err(CommandError::new(
            ErrorKind::NotFound,
            format!("account {k:?} has not synced on this device"),
        ));
    }
    Ok(picked)
}

fn date(secs: i64) -> String {
    DateTime::from_timestamp(secs, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d")
        .to_string()
}

fn utc(secs: i64) -> String {
    DateTime::from_timestamp(secs, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn in_zone(secs: i64, zone: Tz) -> String {
    DateTime::from_timestamp(secs, 0)
        .unwrap_or_default()
        .with_timezone(&zone)
        .to_rfc3339()
}

/// Parses `--from` into a date, `today` being today in `zone` at `now`.
pub fn parse_day(from: Option<&str>, zone: Tz, now: i64) -> Result<NaiveDate, CommandError> {
    let today = DateTime::from_timestamp(now, 0)
        .unwrap_or_default()
        .with_timezone(&zone)
        .date_naive();
    let s = from.unwrap_or("today").trim().to_ascii_lowercase();
    match s.as_str() {
        "today" => Ok(today),
        "tomorrow" => Ok(today + Duration::days(1)),
        "yesterday" => Ok(today - Duration::days(1)),
        _ => NaiveDate::parse_from_str(&s, "%Y-%m-%d").map_err(|_| {
            CommandError::new(
                ErrorKind::Usage,
                format!("--from {s:?}: use today, tomorrow, yesterday or a date like 2026-11-05"),
            )
        }),
    }
}

/// Picks the calendars a `--calendar` value names.
fn pick_calendars(
    r: &Reader,
    accounts: &[Account],
    wanted: Option<&str>,
    cfg: Option<&Config>,
) -> Result<Vec<(Account, Calendar)>, CommandError> {
    let mut all = Vec::new();
    for a in accounts {
        for c in r.calendars(a.id).map_err(store_err)? {
            all.push((a.clone(), c));
        }
    }
    let Some(wanted) = wanted else {
        return Ok(all);
    };
    let purpose = cfg.and_then(|c| c.calendar.purposes.get(wanted));
    let picked: Vec<(Account, Calendar)> = all
        .iter()
        .filter(|(a, c)| match purpose {
            Some(p) => a.key == p.account && c.name.eq_ignore_ascii_case(&p.calendar),
            None => c.name.eq_ignore_ascii_case(wanted.trim()),
        })
        .cloned()
        .collect();
    if picked.is_empty() {
        let mut names: Vec<String> = all.iter().map(|(_, c)| format!("{:?}", c.name)).collect();
        names.sort();
        names.dedup();
        let what = match purpose {
            Some(p) => format!(
                "purpose {wanted:?} points to {:?} on {}, which has not synced on this device",
                p.calendar, p.account
            ),
            None => format!("no calendar called {wanted:?}"),
        };
        return Err(CommandError::new(
            ErrorKind::NotFound,
            if names.is_empty() {
                format!("{what}; no calendars have synced yet")
            } else {
                format!("{what}; calendars here: {}", names.join(", "))
            },
        ));
    }
    Ok(picked)
}

/// Lists events from the store at `paths`, as `fam cal list` does, with
/// `now` (Unix seconds) and `zone` given. Used by the command and by tests.
pub fn list_at(
    paths: &DaemonPaths,
    cfg: Option<&Config>,
    input: &CalListInput,
    zone: Tz,
    now: i64,
) -> Result<CalListOutput, CommandError> {
    let days = input.days.unwrap_or(7);
    if !(1..=366).contains(&days) {
        return Err(CommandError::new(
            ErrorKind::Usage,
            "--days must be between 1 and 366",
        ));
    }
    let first = parse_day(input.from.as_deref(), zone, now)?;
    let last = first + Duration::days(i64::from(days));
    let midnight = |d: NaiveDate| d.and_hms_opt(0, 0, 0).expect("midnight exists");
    let range = EventRange {
        start: tz::local_to_utc(midnight(first), zone).timestamp(),
        end: tz::local_to_utc(midnight(last), zone).timestamp(),
        floating_start: midnight(first).and_utc().timestamp(),
        floating_end: midnight(last).and_utc().timestamp(),
    };

    let mut reader = open(paths)?;
    reader
        .snapshot(|r| Ok(list_in(r, cfg, input, zone, range)))
        .map_err(store_err)?
}

fn list_in(
    r: &Reader,
    cfg: Option<&Config>,
    input: &CalListInput,
    zone: Tz,
    range: EventRange,
) -> Result<CalListOutput, CommandError> {
    let accts = accounts(r, input.account.as_deref())?;
    let cals = pick_calendars(r, &accts, input.calendar.as_deref(), cfg)?;
    let mut warnings = Vec::new();
    for (a, c) in &cals {
        match (c.window_start, c.window_end) {
            (Some(ws), Some(we)) => {
                if range.floating_start < ws || range.floating_end > we + 86_400 {
                    warnings.push(format!(
                                "{}/{}: this device keeps events from {} to {}; the rest of the range is not synced",
                                a.key,
                                c.name,
                                date(ws),
                                date(we)
                            ));
                }
            }
            _ => warnings.push(format!("{}/{}: not synced yet", a.key, c.name)),
        }
    }
    let ids: Vec<CalendarId> = cals.iter().map(|(_, c)| c.id).collect();
    let events = r.events_in(&ids, range).map_err(store_err)?;
    let lookup = |id: CalendarId| cals.iter().find(|(_, c)| c.id == id);
    let events = events
        .into_iter()
        .filter_map(|e| {
            let (a, c) = lookup(e.data.calendar)?;
            let d = e.data;
            let (start, end) = if d.is_all_day {
                (date(d.start_at), date(d.end_at))
            } else {
                (in_zone(d.start_at, zone), in_zone(d.end_at, zone))
            };
            Some(CalEvent {
                id: e.id.0,
                account: a.key.clone(),
                calendar: c.name.clone(),
                subject: d.subject,
                start,
                end,
                all_day: d.is_all_day,
                location: d.location,
                organizer: d.organizer.addr,
                show_as: d.show_as,
                cancelled: d.is_cancelled,
                recurring: d.series_master_id.is_some() || d.kind != "singleInstance",
                response: d.response,
                time_zone: d.start_tz,
            })
        })
        .collect();
    Ok(CalListOutput {
        time_zone: zone.name().to_string(),
        from: in_zone(range.start, zone),
        to: in_zone(range.end, zone),
        calendars: cals
            .iter()
            .map(|(a, c)| format!("{}/{}", a.key, c.name))
            .collect(),
        events,
        warnings,
    })
}

/// Lists calendars from the store at `paths`, as `fam cal calendars` does.
pub fn calendars_at(
    paths: &DaemonPaths,
    cfg: Option<&Config>,
    account: Option<&str>,
) -> Result<CalCalendarsOutput, CommandError> {
    let mut reader = open(paths)?;
    reader
        .snapshot(|r| Ok(calendars_in(r, cfg, account)))
        .map_err(store_err)?
}

fn calendars_in(
    r: &Reader,
    cfg: Option<&Config>,
    account: Option<&str>,
) -> Result<CalCalendarsOutput, CommandError> {
    let accts = accounts(r, account)?;
    let mut out = Vec::new();
    for a in &accts {
        let states = r.sync_states(a.id).map_err(store_err)?;
        for c in r.calendars(a.id).map_err(store_err)? {
            let state = states
                .iter()
                .find(|s| s.resource == format!("events:{}", c.remote_id));
            let purposes = cfg
                .map(|cfg| {
                    cfg.calendar
                        .purposes
                        .iter()
                        .filter(|(_, p)| {
                            p.account == a.key && p.calendar.eq_ignore_ascii_case(&c.name)
                        })
                        .map(|(k, _)| k.clone())
                        .collect()
                })
                .unwrap_or_default();
            out.push(CalCalendar {
                account: a.key.clone(),
                name: c.name.clone(),
                is_default: c.is_default,
                can_edit: c.can_edit,
                owner: c.owner.addr.clone(),
                purposes,
                events: r.event_count(c.id).map_err(store_err)?,
                synced_from: c.window_start.map(date),
                synced_to: c.window_end.map(date),
                last_synced_at: state.and_then(|s| s.synced_at).map(utc),
                error: state.and_then(|s| s.last_error.clone()),
            });
        }
    }
    Ok(CalCalendarsOutput { calendars: out })
}

fn zone_of(name: Option<&str>) -> Result<Tz, CommandError> {
    match name {
        None => Ok(tz::local_zone()),
        Some(n) => tz::parse_zone(n).ok_or_else(|| {
            CommandError::new(
                ErrorKind::Usage,
                format!("--tz {n:?}: not an IANA or Windows time zone"),
            )
        }),
    }
}

/// `HH:MM` of an RFC 3339 time.
fn hm(t: &str) -> &str {
    t.get(11..16).unwrap_or(t)
}

impl Command for CalList {
    type Input = CalListInput;
    type Output = CalListOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["cal", "list"],
        summary: "List events in a date range",
        description: "Lists events from the calendars on this device for --days days (default 7) from --from (default today), in --tz or this computer's time zone. --calendar takes a calendar name or a purpose from [calendar.purposes]. Reads the store, so it works without famd; famd keeps events for the window set by [calendar] past and future.",
        access: Access::Read,
        errors: &[ErrorKind::Usage, ErrorKind::Config, ErrorKind::NotFound],
    };

    fn run(&self, _ctx: &Context<'_>, input: CalListInput) -> Result<CalListOutput, CommandError> {
        let zone = zone_of(input.tz.as_deref())?;
        let cfg = config()?;
        list_at(&DaemonPaths::resolve()?, cfg.as_ref(), &input, zone, now())
    }

    fn render_text(&self, o: &CalListOutput) -> String {
        let mut s = String::new();
        let mut day = String::new();
        let many = {
            let mut c: Vec<&str> = o.events.iter().map(|e| e.calendar.as_str()).collect();
            c.sort();
            c.dedup();
            c.len() > 1
        };
        for e in &o.events {
            let d = e.start.get(..10).unwrap_or(&e.start).to_string();
            if d != day {
                if !day.is_empty() {
                    s.push('\n');
                }
                let label = NaiveDate::parse_from_str(&d, "%Y-%m-%d")
                    .map(|n| format!("{} {} {}", n.format("%a"), n.day(), n.format("%b %Y")))
                    .unwrap_or_else(|_| d.clone());
                s.push_str(&label);
                s.push('\n');
                day = d;
            }
            let when = if e.all_day {
                "all day    ".to_string()
            } else {
                format!("{}-{}", hm(&e.start), hm(&e.end))
            };
            s.push_str(&format!("  {when}  {}", e.subject));
            if !e.location.is_empty() {
                s.push_str(&format!(" @ {}", e.location));
            }
            if many {
                s.push_str(&format!(" [{}]", e.calendar));
            }
            if e.cancelled {
                s.push_str(" (cancelled)");
            }
            s.push('\n');
        }
        if o.events.is_empty() {
            s.push_str(&format!(
                "No events from {} to {}.\n",
                o.from.get(..10).unwrap_or(&o.from),
                o.to.get(..10).unwrap_or(&o.to)
            ));
        }
        s.push_str(&format!("\nTimes in {}.", o.time_zone));
        for w in &o.warnings {
            s.push_str(&format!("\nNote: {w}"));
        }
        s
    }
}

impl Command for CalCalendars {
    type Input = CalCalendarsInput;
    type Output = CalCalendarsOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["cal", "calendars"],
        summary: "List the calendars on this device",
        description: "Lists each account's calendars with how many events are on this device, the synced window, the last sync and any error, and the purposes from [calendar.purposes] that point to each.",
        access: Access::Read,
        errors: &[ErrorKind::Config, ErrorKind::NotFound],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: CalCalendarsInput,
    ) -> Result<CalCalendarsOutput, CommandError> {
        let cfg = config()?;
        calendars_at(
            &DaemonPaths::resolve()?,
            cfg.as_ref(),
            input.account.as_deref(),
        )
    }

    fn render_text(&self, o: &CalCalendarsOutput) -> String {
        if o.calendars.is_empty() {
            return "No calendars have synced on this device yet.".into();
        }
        let mut s = String::new();
        for c in &o.calendars {
            s.push_str(&format!("{}/{}", c.account, c.name));
            if c.is_default {
                s.push_str(" (default)");
            }
            if !c.purposes.is_empty() {
                s.push_str(&format!(" purpose {}", c.purposes.join(", ")));
            }
            s.push_str(&format!(": {} events", c.events));
            if let (Some(f), Some(t)) = (&c.synced_from, &c.synced_to) {
                s.push_str(&format!(" from {f} to {t}"));
            } else {
                s.push_str(", not synced yet");
            }
            s.push('\n');
            if let Some(e) = &c.error {
                s.push_str(&format!("  error: {e}\n"));
            }
        }
        s.pop();
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_words_and_dates() {
        // 2026-11-04T23:00Z is already 5 Nov in Auckland.
        let now = 1_793_833_200;
        let nz = Tz::Pacific__Auckland;
        assert_eq!(parse_day(None, nz, now).unwrap().to_string(), "2026-11-05");
        assert_eq!(
            parse_day(None, Tz::UTC, now).unwrap().to_string(),
            "2026-11-04"
        );
        assert_eq!(
            parse_day(Some("Tomorrow"), nz, now).unwrap().to_string(),
            "2026-11-06"
        );
        assert_eq!(
            parse_day(Some("2026-12-20"), nz, now).unwrap().to_string(),
            "2026-12-20"
        );
        assert_eq!(
            parse_day(Some("next tuesday"), nz, now).unwrap_err().kind,
            ErrorKind::Usage
        );
    }
}
