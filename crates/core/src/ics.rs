// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading `.ics` attachments (RFC 5545) for `fam cal propose`.
//!
//! Only what an event in someone's calendar needs: each `VEVENT`'s summary,
//! start and end (or duration), zone, location, description, UID and a
//! simple repeat rule. `VTIMEZONE` blocks are not read: a `TZID` is matched
//! by name (IANA or Windows), which covers Outlook, Google and the booking
//! sites seen so far. Anything else is left out with a note.

use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

use crate::tz;

/// A calendar file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Calendar {
    /// `METHOD` (`PUBLISH`, `REQUEST`, `CANCEL`), upper case, if given.
    pub method: Option<String>,
    /// Its events.
    pub events: Vec<IcsEvent>,
    /// Things that were left out or guessed.
    pub notes: Vec<String>,
}

/// When an event starts or ends, as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IcsTime {
    /// A date (all day).
    Date(NaiveDate),
    /// A wall-clock time in the event's zone, or floating.
    Local(NaiveDateTime),
    /// An instant (a `Z` time).
    Utc(NaiveDateTime),
}

/// A repeat rule `fam cal add --repeat` can express.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcsRepeat {
    /// `daily`, `weekly`, `monthly` or `yearly`.
    pub every: &'static str,
    /// Every this many.
    pub interval: u32,
    /// Weekly: days as `mon`, `tue`, ...
    pub on: Vec<String>,
    /// The last occurrence's latest start (`UNTIL`), as written.
    pub until: Option<IcsTime>,
    /// Or this many times.
    pub count: Option<u32>,
}

/// One `VEVENT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcsEvent {
    /// `UID`.
    pub uid: Option<String>,
    /// `SUMMARY`.
    pub summary: String,
    /// `DTSTART`.
    pub start: IcsTime,
    /// `DTEND`, or `DTSTART` plus `DURATION`.
    pub end: Option<IcsTime>,
    /// The `TZID` of `DTSTART`, as an IANA name, when it is a known zone.
    pub zone: Option<String>,
    /// `LOCATION`.
    pub location: Option<String>,
    /// `DESCRIPTION`.
    pub description: Option<String>,
    /// `RRULE`, when it is a simple one.
    pub repeat: Option<IcsRepeat>,
    /// `STATUS:CANCELLED`.
    pub cancelled: bool,
}

/// A content line: name, parameters and value.
struct Line {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Line {
    fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

/// Joins folded lines (a line break followed by a space or tab).
fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = raw.strip_prefix([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        if !raw.is_empty() {
            out.push(raw.to_string());
        }
    }
    out
}

fn split_line(s: &str) -> Option<Line> {
    // The value starts at the first ':' outside a quoted parameter value.
    let mut quoted = false;
    let mut colon = None;
    for (i, c) in s.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&s[..colon], &s[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.trim().to_string(), v.trim().trim_matches('"').to_string()))
        })
        .collect();
    Some(Line {
        name,
        params,
        value: value.to_string(),
    })
}

/// `\n`, `\,`, `\;` and `\\` in TEXT values.
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

fn text(v: &str) -> Option<String> {
    let t = unescape(v).trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn parse_date(v: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(v.get(..8)?, "%Y%m%d").ok()
}

/// A `DTSTART`/`DTEND` value with its zone, if it names one.
fn parse_time(l: &Line) -> Option<(IcsTime, Option<&str>)> {
    let v = l.value.trim();
    let is_date = l
        .param("VALUE")
        .is_some_and(|t| t.eq_ignore_ascii_case("DATE"))
        || v.len() == 8;
    if is_date {
        return Some((IcsTime::Date(parse_date(v)?), None));
    }
    let (body, utc) = match v.strip_suffix(['Z', 'z']) {
        Some(b) => (b, true),
        None => (v, false),
    };
    let dt = NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S")
        .or_else(|_| NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M"))
        .ok()?;
    Some(if utc {
        (IcsTime::Utc(dt), None)
    } else {
        (IcsTime::Local(dt), l.param("TZID"))
    })
}

/// `PT1H30M`, `P1D`, `P2W`, `-PT15M`. Negative durations give `None`.
fn parse_duration(v: &str) -> Option<Duration> {
    let v = v.trim().strip_prefix('+').unwrap_or(v.trim());
    let v = v.strip_prefix('P')?;
    let mut total = Duration::zero();
    let mut num = String::new();
    let mut in_time = false;
    for c in v.chars() {
        match c {
            '0'..='9' => num.push(c),
            'T' => in_time = true,
            _ => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                let part = match (c, in_time) {
                    ('W', false) => Duration::try_weeks(n),
                    ('D', false) => Duration::try_days(n),
                    ('H', true) => Duration::try_hours(n),
                    ('M', true) => Duration::try_minutes(n),
                    ('S', true) => Duration::try_seconds(n),
                    _ => return None,
                }?;
                total = total.checked_add(&part)?;
            }
        }
    }
    num.is_empty().then_some(total)
}

const DAYS: [(&str, &str); 7] = [
    ("MO", "mon"),
    ("TU", "tue"),
    ("WE", "wed"),
    ("TH", "thu"),
    ("FR", "fri"),
    ("SA", "sat"),
    ("SU", "sun"),
];

/// A simple `RRULE`, or `None` with the reason when it has parts `cal add`
/// can't express (`BYMONTHDAY`, `BYSETPOS`, `2TU`, ...).
fn parse_rrule(v: &str) -> Result<IcsRepeat, String> {
    let mut every = None;
    let mut interval = 1;
    let mut on = Vec::new();
    let mut until = None;
    let mut count = None;
    for part in v.split(';').filter(|p| !p.is_empty()) {
        let (k, val) = part
            .split_once('=')
            .ok_or_else(|| format!("odd part {part:?}"))?;
        match k.to_ascii_uppercase().as_str() {
            "FREQ" => {
                every = Some(match val.to_ascii_uppercase().as_str() {
                    "DAILY" => "daily",
                    "WEEKLY" => "weekly",
                    "MONTHLY" => "monthly",
                    "YEARLY" => "yearly",
                    other => return Err(format!("FREQ={other}")),
                });
            }
            "INTERVAL" => interval = val.parse().map_err(|_| format!("INTERVAL={val}"))?,
            "COUNT" => count = Some(val.parse().map_err(|_| format!("COUNT={val}"))?),
            "UNTIL" => {
                let l = Line {
                    name: "UNTIL".into(),
                    params: Vec::new(),
                    value: val.to_string(),
                };
                until = Some(parse_time(&l).ok_or_else(|| format!("UNTIL={val}"))?.0);
            }
            "BYDAY" => {
                for d in val.split(',') {
                    let d = d.trim().to_ascii_uppercase();
                    let day = DAYS
                        .iter()
                        .find(|(code, _)| *code == d)
                        .ok_or_else(|| format!("BYDAY={val}"))?;
                    on.push(day.1.to_string());
                }
            }
            "WKST" => {}
            other => return Err(other.to_string()),
        }
    }
    let every = every.ok_or("no FREQ")?;
    if !on.is_empty() && every != "weekly" {
        return Err(format!("BYDAY with FREQ={}", every.to_ascii_uppercase()));
    }
    Ok(IcsRepeat {
        every,
        interval: interval.max(1),
        on,
        until,
        count,
    })
}

fn add(t: IcsTime, d: Duration) -> Option<IcsTime> {
    Some(match t {
        IcsTime::Date(x) => IcsTime::Date(x.checked_add_signed(d)?),
        IcsTime::Local(x) => IcsTime::Local(x.checked_add_signed(d)?),
        IcsTime::Utc(x) => IcsTime::Utc(x.checked_add_signed(d)?),
    })
}

/// Reads a calendar file. Events without a usable start are left out, with
/// a note.
pub fn parse(text: &str) -> Calendar {
    let mut cal = Calendar::default();
    let mut depth: Vec<String> = Vec::new();
    let mut cur: Option<Vec<Line>> = None;
    for raw in unfold(text) {
        let Some(l) = split_line(&raw) else { continue };
        match l.name.as_str() {
            "BEGIN" => {
                let what = l.value.trim().to_ascii_uppercase();
                if what == "VEVENT" && cur.is_none() {
                    cur = Some(Vec::new());
                }
                depth.push(what);
            }
            "END" => {
                let what = l.value.trim().to_ascii_uppercase();
                if depth.last() == Some(&what) {
                    depth.pop();
                }
                if what == "VEVENT" && !depth.iter().any(|d| d == "VEVENT") {
                    if let Some(lines) = cur.take() {
                        if let Some(e) = event(&lines, &mut cal.notes) {
                            cal.events.push(e);
                        }
                    }
                }
            }
            "METHOD" if cur.is_none() => cal.method = Some(l.value.trim().to_ascii_uppercase()),
            _ => {
                // Only the event's own lines: not those of a VALARM inside it.
                if let Some(lines) = cur.as_mut() {
                    if depth.last().map(String::as_str) == Some("VEVENT") {
                        lines.push(l);
                    }
                }
            }
        }
    }
    cal
}

fn event(lines: &[Line], notes: &mut Vec<String>) -> Option<IcsEvent> {
    let get = |n: &str| lines.iter().find(|l| l.name == n);
    let summary = get("SUMMARY")
        .and_then(|l| text(&l.value))
        .unwrap_or_else(|| "(no subject)".into());
    let Some((start, tzid)) = get("DTSTART").and_then(parse_time) else {
        notes.push(format!(
            "{summary:?} in the invitation has no start I can read"
        ));
        return None;
    };
    let zone = tzid.and_then(|z| {
        let found = tz::to_iana(z);
        if found.is_none() {
            notes.push(format!(
                "{summary:?}: time zone {z:?} isn't one I know, so its times are read in this computer's zone"
            ));
        }
        found
    });
    let end = match get("DTEND").and_then(parse_time) {
        // An end in another zone than the start (a flight): as an instant,
        // so it is shown in the start's zone.
        Some((IcsTime::Local(dt), Some(z))) if tz::to_iana(z) != zone => match tz::parse_zone(z) {
            Some(end_zone) => Some(IcsTime::Utc(tz::local_to_utc(dt, end_zone).naive_utc())),
            None => Some(IcsTime::Local(dt)),
        },
        Some((e, _)) => Some(e),
        None => get("DURATION")
            .and_then(|l| parse_duration(&l.value))
            .and_then(|d| add(start, d)),
    };
    // A changed or moved occurrence of a series sent on its own: the series
    // itself, or the person's copy of it, carries the change.
    if get("RECURRENCE-ID").is_some() {
        notes.push(format!(
            "{summary:?}: one changed date of a repeating event is left out"
        ));
        return None;
    }
    let exceptions = get("EXDATE").or_else(|| get("RDATE")).is_some();
    let repeat = match get("RRULE") {
        None => None,
        Some(_) if exceptions => {
            notes.push(format!(
                "{summary:?} repeats with dates added or left out, which Familiar can't set yet; only its first date is shown"
            ));
            None
        }
        Some(l) => match parse_rrule(&l.value) {
            Ok(r) => Some(r),
            Err(why) => {
                notes.push(format!(
                    "{summary:?} repeats in a way Familiar can't set yet ({why}); only its first date is shown"
                ));
                None
            }
        },
    };
    Some(IcsEvent {
        uid: get("UID").and_then(|l| text(&l.value)),
        summary,
        start,
        end,
        zone,
        location: get("LOCATION").and_then(|l| text(&l.value)),
        description: get("DESCRIPTION").and_then(|l| text(&l.value)),
        repeat,
        cancelled: get("STATUS").is_some_and(|l| l.value.trim().eq_ignore_ascii_case("CANCELLED")),
    })
}

impl IcsTime {
    /// The wall-clock time in `zone` (IANA): `Utc` times are converted, the
    /// others are as written. Dates give `None` for the time.
    pub fn local_in(self, zone: &str) -> (NaiveDate, Option<NaiveTime>) {
        match self {
            IcsTime::Date(d) => (d, None),
            IcsTime::Local(dt) => (dt.date(), Some(dt.time())),
            IcsTime::Utc(dt) => {
                let z = tz::parse_zone(zone).unwrap_or(chrono_tz::UTC);
                let local = Utc.from_utc_datetime(&dt).with_timezone(&z).naive_local();
                (local.date(), Some(local.time()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTLOOK: &str = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nPRODID:Microsoft Exchange Server 2010\r\nVERSION:2.0\r\nBEGIN:VTIMEZONE\r\nTZID:New Zealand Standard Time\r\nBEGIN:STANDARD\r\nDTSTART:16010101T030000\r\nTZOFFSETFROM:+1300\r\nTZOFFSETTO:+1200\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nORGANIZER;CN=Jo Smith:mailto:jo@club.example\r\nDESCRIPTION;LANGUAGE=en-US:Agenda attached.\\nBring the accounts\\, please.\\n\r\nUID:040000008200E00074C5B7101A82E00800000000\r\nSUMMARY;LANGUAGE=en-US:Committee meeting\r\nDTSTART;TZID=New Zealand Standard Time:20261124T190000\r\nDTEND;TZID=New Zealand Standard Time:20261124T203000\r\nLOCATION;LANGUAGE=en-US:Clubrooms\r\nBEGIN:VALARM\r\nDESCRIPTION:REMINDER\r\nTRIGGER;RELATED=START:-PT15M\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn reads_an_outlook_invitation() {
        let c = parse(OUTLOOK);
        assert_eq!(c.method.as_deref(), Some("REQUEST"));
        assert!(c.notes.is_empty(), "{:?}", c.notes);
        let e = &c.events[0];
        assert_eq!(e.summary, "Committee meeting");
        assert_eq!(e.zone.as_deref(), Some("Pacific/Auckland"));
        assert_eq!(
            e.start,
            IcsTime::Local(
                NaiveDate::from_ymd_opt(2026, 11, 24)
                    .unwrap()
                    .and_hms_opt(19, 0, 0)
                    .unwrap()
            )
        );
        assert_eq!(e.location.as_deref(), Some("Clubrooms"));
        // The VALARM's DESCRIPTION is not the event's.
        assert_eq!(
            e.description.as_deref(),
            Some("Agenda attached.\nBring the accounts, please.")
        );
        assert_eq!(
            e.uid.as_deref(),
            Some("040000008200E00074C5B7101A82E00800000000")
        );
    }

    #[test]
    fn reads_dates_utc_times_durations_and_folding() {
        let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nSUMMARY:Concert at the Town\n  Hall\nDTSTART:20261121T053000Z\nDURATION:PT2H30M\nEND:VEVENT\nBEGIN:VEVENT\nSUMMARY:School camp\nDTSTART;VALUE=DATE:20270203\nDTEND;VALUE=DATE:20270206\nEND:VEVENT\nEND:VCALENDAR\n";
        let c = parse(ics);
        assert_eq!(c.events.len(), 2);
        let concert = &c.events[0];
        assert_eq!(concert.summary, "Concert at the Town Hall");
        let (d, t) = concert.start.local_in("Pacific/Auckland");
        assert_eq!(d, NaiveDate::from_ymd_opt(2026, 11, 21).unwrap());
        assert_eq!(t, NaiveTime::from_hms_opt(18, 30, 0));
        let (_, end_t) = concert.end.unwrap().local_in("Pacific/Auckland");
        assert_eq!(end_t, NaiveTime::from_hms_opt(21, 0, 0));
        let camp = &c.events[1];
        assert_eq!(
            camp.start,
            IcsTime::Date(NaiveDate::from_ymd_opt(2027, 2, 3).unwrap())
        );
        assert_eq!(
            camp.end,
            Some(IcsTime::Date(NaiveDate::from_ymd_opt(2027, 2, 6).unwrap()))
        );
    }

    #[test]
    fn reads_simple_repeats_and_notes_the_rest() {
        let ics = "BEGIN:VEVENT\nSUMMARY:Nets\nDTSTART;TZID=Pacific/Auckland:20270202T160000\nRRULE:FREQ=WEEKLY;BYDAY=TU,TH;UNTIL=20270413T000000Z\nEND:VEVENT\nBEGIN:VEVENT\nSUMMARY:AGM\nDTSTART;TZID=Somewhere Odd:20270301T190000\nRRULE:FREQ=MONTHLY;BYDAY=1MO\nEND:VEVENT\n";
        let c = parse(ics);
        let nets = c.events[0].repeat.as_ref().unwrap();
        assert_eq!(nets.every, "weekly");
        assert_eq!(nets.on, ["tue", "thu"]);
        assert_eq!(
            nets.until.unwrap().local_in("Pacific/Auckland").0,
            NaiveDate::from_ymd_opt(2027, 4, 13).unwrap()
        );
        assert_eq!(c.events[1].repeat, None);
        assert_eq!(c.events[1].zone, None);
        assert_eq!(c.notes.len(), 2, "{:?}", c.notes);
    }

    #[test]
    fn repeats_with_exceptions_keep_only_the_first_date() {
        let ics = "BEGIN:VEVENT\nUID:a\nSUMMARY:Nets\nDTSTART:20270202T030000Z\nRRULE:FREQ=WEEKLY;COUNT=8\nEXDATE:20270216T030000Z\nEND:VEVENT\nBEGIN:VEVENT\nUID:a\nRECURRENCE-ID:20270223T030000Z\nSUMMARY:Nets (moved)\nDTSTART:20270224T030000Z\nEND:VEVENT\n";
        let c = parse(ics);
        assert_eq!(c.events.len(), 1);
        assert_eq!(c.events[0].repeat, None);
        assert_eq!(c.notes.len(), 2, "{:?}", c.notes);
    }

    #[test]
    fn broken_files_give_what_they_can() {
        assert!(parse("not a calendar").events.is_empty());
        let c = parse("BEGIN:VEVENT\nSUMMARY:No start\nEND:VEVENT\n");
        assert!(c.events.is_empty());
        assert_eq!(c.notes.len(), 1);
    }

    #[test]
    fn ends_in_another_zone_and_utc_until() {
        let ics = "BEGIN:VEVENT\nSUMMARY:Flight\nDTSTART;TZID=America/New_York:20261123T090000\nDTEND;TZID=America/Los_Angeles:20261123T120000\nEND:VEVENT\nBEGIN:VEVENT\nSUMMARY:Swim\nDTSTART;TZID=Pacific/Auckland:20270201T090000\nRRULE:FREQ=DAILY;UNTIL=20270204T200000Z\nEND:VEVENT\n";
        let c = parse(ics);
        let (_, t) = c.events[0].end.unwrap().local_in("America/New_York");
        assert_eq!(t, NaiveTime::from_hms_opt(15, 0, 0));
        // 20:00 UTC on the 4th is 9 am on the 5th in Auckland.
        let until = c.events[1].repeat.as_ref().unwrap().until.unwrap();
        assert_eq!(
            until.local_in("Pacific/Auckland").0,
            NaiveDate::from_ymd_opt(2027, 2, 5).unwrap()
        );
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("PT1H30M"), Some(Duration::minutes(90)));
        assert_eq!(parse_duration("P1D"), Some(Duration::days(1)));
        assert_eq!(parse_duration("P2W"), Some(Duration::weeks(2)));
        assert_eq!(parse_duration("-PT15M"), None);
        assert_eq!(parse_duration("PT"), Some(Duration::zero()));
        assert_eq!(parse_duration("P9223372036854775807D"), None);
    }
}
