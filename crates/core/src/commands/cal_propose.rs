// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam cal propose` (AI events plan, WP AI.1): the calendar events an email
//! suggests, for the person to confirm before anything is created.
//!
//! In order: a meeting request is answered, not copied, so it comes back as
//! an invitation; else each `.ics` attachment's events; else the date, time
//! and place `fam cal add --from-mail` would guess. Nothing is written.
//! Each proposal is ready for `fam cal add` (`start`, `end`, `tz`, the
//! repeat flags), and one that is already in the calendar says so.
//!
//! With `[ai] enabled = true` in config.toml, an email with neither is sent
//! (its text, and the text of its PDF attachments) to the AI tool `[ai]`
//! names, which answers with events; Familiar checks them and falls back to
//! its own guess when the tool fails. See [`crate::ai`].
//!
//! Attachments and invitations are read from the server through famd. When
//! famd isn't running, or the server can't be reached, those checks are
//! skipped with a note and the guess is still given.

use chrono::{Duration, NaiveDate, NaiveTime};
use familiar_store::{AccountId, EventRange, MessageId, Reader};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai;
use crate::commands::attachments::{AttachmentInfo, content_at, graph_get, list_at, locate};
use crate::commands::auth::config;
use crate::commands::cal_write::{mail_text, open, zone_name};
use crate::daemon::DaemonPaths;
use crate::error::{CommandError, ErrorKind};
use crate::events::{self, date_secs, instant, today_in, when_text};
use crate::ics;
use crate::registry::{Access, Command, CommandSpec, Context};

/// Attachments bigger than this aren't read as calendars.
const ICS_LIMIT: u64 = 1024 * 1024;

/// PDFs bigger than this aren't read.
const PDF_LIMIT: u64 = 10 * 1024 * 1024;

/// `fam cal propose`.
pub struct CalPropose;

/// Input for `fam cal propose`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CalProposeInput {
    /// The email (local message ID).
    pub from_mail: i64,
    /// Time zone to give times in (IANA or Windows). Default: this computer's.
    #[serde(default)]
    pub tz: Option<String>,
    /// Don't ask the AI tool, even when [ai] is enabled.
    #[serde(default)]
    pub no_ai: bool,
}

/// A repeat, in `fam cal add`'s terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ProposedRepeat {
    /// `daily`, `weekly`, `monthly` or `yearly` (`--repeat`).
    pub every: String,
    /// Every this many (`--every`).
    pub interval: u32,
    /// Weekly: the days, such as `tue` (`--on`).
    pub on: Vec<String>,
    /// The last date, `YYYY-MM-DD` (`--until`).
    pub until: Option<String>,
    /// Or this many times (`--count`).
    pub count: Option<u32>,
}

/// An event the email suggests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ProposedEvent {
    /// Subject.
    pub subject: String,
    /// `YYYY-MM-DD HH:MM`, or `YYYY-MM-DD` for all day, in `time_zone`.
    pub start: String,
    /// Same form. All day: the last day. None: `fam cal add`'s default length.
    pub end: Option<String>,
    /// All day.
    pub all_day: bool,
    /// IANA zone the times are in.
    pub time_zone: String,
    /// Location.
    pub location: Option<String>,
    /// Notes.
    pub notes: Option<String>,
    /// How it repeats.
    pub repeat: Option<ProposedRepeat>,
    /// The words in the email it came from, when the AI found it.
    pub evidence: Option<String>,
    /// iCalendar UID, from an `.ics`.
    pub uid: Option<String>,
    /// The local ID of an event already in the calendar that matches it.
    pub already: Option<i64>,
}

/// A meeting request or cancellation the email is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Invitation {
    /// `request` or `cancelled`.
    pub kind: String,
    /// The meeting's local event ID, once it has synced to this device.
    pub event: Option<i64>,
    /// Its subject, when synced.
    pub subject: Option<String>,
    /// When it is, in words, when synced.
    pub when: Option<String>,
    /// The calendar it is in, when synced.
    pub calendar: Option<String>,
    /// Your answer so far, when synced.
    pub response: Option<String>,
}

/// Output of `fam cal propose`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CalProposeOutput {
    /// The email.
    pub message: i64,
    /// Its account.
    pub account: String,
    /// Where the proposals came from: `invite`, `ics`, `ai:claude`,
    /// `ai:codex`, `heuristic` or `none`.
    pub source: String,
    /// Events to add.
    pub events: Vec<ProposedEvent>,
    /// Set when the email is a meeting request or cancellation; answer it
    /// with `fam cal rsvp` (or remove it with `fam cal delete`) instead.
    pub invitation: Option<Invitation>,
    /// What was skipped or guessed.
    pub notes: Vec<String>,
}

fn store_err(e: familiar_store::Error) -> CommandError {
    CommandError::internal(format!("store: {e}"))
}

fn day(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn stamp(d: NaiveDate, t: Option<NaiveTime>) -> String {
    match t {
        Some(t) => format!("{} {}", day(d), t.format("%H:%M")),
        None => day(d),
    }
}

/// Is this a server error worth saying out loud, rather than famd simply
/// not running?
fn note_for(what: &str, e: &CommandError) -> String {
    if e.kind == ErrorKind::DaemonUnavailable {
        format!("famd isn't running, so {what} weren't checked")
    } else {
        format!("{what} couldn't be checked: {}", e.message)
    }
}

/// The meeting the email is a request or cancellation for, if it is one.
fn invitation(
    paths: &DaemonPaths,
    r: &Reader,
    account_id: AccountId,
    key: &str,
    remote: &str,
    zone: &str,
) -> Result<Option<Invitation>, CommandError> {
    let head = graph_get(paths, key, format!("/me/messages/{remote}?$select=id"))?;
    let is_event_message = head
        .get("@odata.type")
        .and_then(Value::as_str)
        // Requests come as the derived `eventMessageRequest`.
        .is_some_and(|t| t.starts_with("#microsoft.graph.eventMessage"));
    if !is_event_message {
        return Ok(None);
    }
    let v = graph_get(
        paths,
        key,
        format!(
            // Outlook.com refuses both the cast segment and a $select of
            // meetingMessageType; this form works there (checked live).
            "/me/messages/{remote}?$expand=microsoft.graph.eventMessage/event($select=id)"
        ),
    )?;
    let kind = match v.get("meetingMessageType").and_then(Value::as_str) {
        Some("meetingRequest") => "request",
        Some("meetingCancelled") => "cancelled",
        // Answers from attendees and the like: nothing to add or answer.
        _ => return Ok(None),
    };
    let found = match v.pointer("/event/id").and_then(Value::as_str) {
        Some(id) => r.event_by_remote(account_id, id).map_err(store_err)?,
        None => None,
    };
    let calendar = match &found {
        Some(e) => r
            .calendar(e.data.calendar)
            .map_err(store_err)?
            .map(|c| c.name),
        None => None,
    };
    Ok(Some(Invitation {
        kind: kind.into(),
        event: found.as_ref().map(|e| e.id.0),
        subject: found.as_ref().map(|e| e.data.subject.clone()),
        when: found
            .as_ref()
            .map(|e| when_text(e.data.start_at, e.data.end_at, e.data.is_all_day, zone)),
        calendar,
        response: found.and_then(|e| e.data.response),
    }))
}

fn from_ics(e: ics::IcsEvent, zone: &str) -> ProposedEvent {
    let all_day = matches!(e.start, ics::IcsTime::Date(_));
    // A TZID time keeps its zone. A `Z` time is shown in the viewer's zone,
    // unless it repeats: its days and times are UTC's, so it stays in UTC.
    let time_zone = match (e.start, &e.zone) {
        (ics::IcsTime::Local(_), Some(z)) => z.clone(),
        (ics::IcsTime::Utc(_), _) if e.repeat.is_some() => "UTC".to_string(),
        _ => zone.to_string(),
    };
    let (sd, st) = e.start.local_in(&time_zone);
    let end = e.end.map(|end| {
        let (ed, et) = end.local_in(&time_zone);
        if all_day {
            // DTEND is the day after; `cal add --end` takes the last day.
            day((ed - Duration::days(1)).max(sd))
        } else {
            stamp(ed, et)
        }
    });
    ProposedEvent {
        subject: e.summary,
        start: stamp(sd, st),
        end,
        all_day,
        time_zone: time_zone.clone(),
        location: e.location,
        notes: e.description,
        repeat: e.repeat.map(|r| ProposedRepeat {
            every: r.every.into(),
            interval: r.interval,
            on: r.on,
            until: r.until.map(|u| day(u.local_in(&time_zone).0)),
            count: r.count,
        }),
        evidence: None,
        uid: e.uid,
        already: None,
    }
}

/// The events of the email's `.ics` attachments, and whether one of them
/// cancels something (then the text is not guessed from either).
fn is_ics(a: &AttachmentInfo) -> bool {
    a.content_type
        .to_ascii_lowercase()
        .starts_with("text/calendar")
        || a.name.to_ascii_lowercase().ends_with(".ics")
}

fn is_pdf(a: &AttachmentInfo) -> bool {
    a.content_type.eq_ignore_ascii_case("application/pdf")
        || a.name.to_ascii_lowercase().ends_with(".pdf")
}

fn ics_events(
    paths: &DaemonPaths,
    message: i64,
    list: &[AttachmentInfo],
    zone: &str,
    notes: &mut Vec<String>,
) -> Result<(Vec<ProposedEvent>, bool), CommandError> {
    let mut out = Vec::new();
    let mut cancels = false;
    for a in list {
        if !is_ics(a) || a.is_inline {
            continue;
        }
        if a.size > ICS_LIMIT {
            notes.push(format!("{} is too big to read as a calendar", a.name));
            continue;
        }
        let c = content_at(paths, message, &a.id)?;
        let cal = ics::parse(&String::from_utf8_lossy(&c.bytes));
        notes.extend(cal.notes);
        if cal.method.as_deref() == Some("CANCEL") {
            notes.push(format!("{} cancels an event, so nothing is added", a.name));
            cancels = true;
            continue;
        }
        for e in cal.events {
            if e.cancelled {
                notes.push(format!("{:?} is marked cancelled", e.summary));
                cancels = true;
                continue;
            }
            out.push(from_ics(e, zone));
        }
    }
    Ok((out, cancels))
}

/// The text of the email's PDF attachments, and the names of the rest.
fn pdf_texts(
    paths: &DaemonPaths,
    message: i64,
    list: &[AttachmentInfo],
    notes: &mut Vec<String>,
) -> (Vec<(String, String)>, Vec<String>) {
    let mut pdfs = Vec::new();
    let mut others = Vec::new();
    for a in list.iter().filter(|a| !a.is_inline) {
        if !is_pdf(a) {
            others.push(a.name.clone());
            continue;
        }
        if a.size > PDF_LIMIT {
            notes.push(format!("{} is too big to read", a.name));
            continue;
        }
        let bytes = match content_at(paths, message, &a.id) {
            Ok(c) => c.bytes,
            Err(e) => {
                notes.push(format!("{} couldn't be downloaded: {}", a.name, e.message));
                continue;
            }
        };
        // The PDF reader can panic on a damaged file; that file is skipped.
        let text = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&bytes))
            .ok()
            .and_then(Result::ok)
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        if text.is_empty() {
            notes.push(format!("{} has no readable text", a.name));
        } else {
            pdfs.push((a.name.clone(), text));
        }
    }
    (pdfs, others)
}

/// The AI's events, checked: a start `cal add` can read, an end after it,
/// a known zone and repeat. Events that fail are left out with a note.
fn ai_events(
    kind: ai::ToolKind,
    answer: ai::Answer,
    today: NaiveDate,
    zone: &str,
    notes: &mut Vec<String>,
) -> Vec<ProposedEvent> {
    let who = kind.title();
    if let Some(c) = answer.comment.filter(|c| !c.trim().is_empty()) {
        notes.push(format!("{who}: {}", c.trim()));
    }
    let mut out = Vec::new();
    for e in answer.events.into_iter().take(10) {
        let subject = e.subject.trim().to_string();
        let Some(start) = events::parse_when(&e.start, today, None) else {
            notes.push(format!(
                "{who}'s start for {subject:?} ({}) couldn't be read",
                e.start
            ));
            continue;
        };
        let all_day = e.all_day || start.time.is_none();
        let time_zone = match e.time_zone.as_deref().filter(|z| !z.trim().is_empty()) {
            None => zone.to_string(),
            Some(z) => match crate::tz::to_iana(z) {
                Some(iana) => iana,
                None => {
                    notes.push(format!("{who} gave {subject:?} the zone {z:?}, which isn't one I know; using {zone}"));
                    zone.to_string()
                }
            },
        };
        let end = match e.end.as_deref().filter(|x| !x.trim().is_empty()) {
            None => None,
            Some(x) => match events::parse_when(x, today, Some(start.date)) {
                Some(w) if (w.date, w.time) >= (start.date, start.time) => {
                    Some(stamp(w.date, if all_day { None } else { w.time }))
                }
                _ => {
                    notes.push(format!("{who}'s end for {subject:?} ({x}) is before its start or unreadable, so it is left out"));
                    None
                }
            },
        };
        let repeat = e.repeat.and_then(|r| {
            let every = ["daily", "weekly", "monthly", "yearly"]
                .into_iter()
                .find(|k| k.eq_ignore_ascii_case(r.every.trim()))?;
            let on: Vec<String> =
                r.on.iter()
                    .filter_map(|d| events::parse_weekday(d))
                    .map(str::to_string)
                    .collect();
            let until = match r.until.as_deref() {
                None => None,
                Some(u) => {
                    let words: Vec<&str> = u.split_whitespace().collect();
                    match events::parse_date(&words, today) {
                        Some((d, _)) => Some(day(d)),
                        // Not an endless series in its place: the first date only.
                        None => {
                            notes.push(format!("{who}'s repeat end for {subject:?} ({u}) is unreadable, so only the first date is proposed"));
                            return None;
                        }
                    }
                }
            };
            Some(ProposedRepeat {
                every: every.to_string(),
                interval: r.interval.unwrap_or(1).max(1),
                on: if every == "weekly" { on } else { Vec::new() },
                count: if until.is_some() {
                    None
                } else {
                    r.count.filter(|c| *c > 0)
                },
                until,
            })
        });
        out.push(ProposedEvent {
            subject: if subject.is_empty() {
                "(no subject)".into()
            } else {
                subject
            },
            start: stamp(start.date, if all_day { None } else { start.time }),
            end,
            all_day,
            time_zone,
            location: e.location.filter(|l| !l.trim().is_empty()),
            notes: e.notes.filter(|n| !n.trim().is_empty()),
            repeat,
            evidence: e.evidence.filter(|x| !x.trim().is_empty()),
            uid: None,
            already: None,
        });
    }
    out
}

/// The AI tool to ask for this account's mail, if any.
fn ai_tool<'a>(
    cfg: Option<&'a familiar_config::Config>,
    account: &str,
    no_ai: bool,
) -> Option<(ai::Tool, &'a familiar_config::model::Ai)> {
    let a = &cfg?.ai;
    if no_ai || !a.enabled {
        return None;
    }
    if a.accounts
        .as_ref()
        .is_some_and(|list| !list.iter().any(|x| x == account))
    {
        return None;
    }
    // An agent (or the MCP server) asking another agent is a loop nobody needs.
    let source = crate::agent::session_source();
    if source == "mcp" || source.starts_with("agent:") {
        return None;
    }
    ai::pick(&a.tool).map(|t| (t, a))
}

/// The local ID of an event already in the account's calendars that
/// matches: the same UID, or the same subject starting within a minute.
/// The same subject, ignoring case, or one inside the other: a ticket
/// email's "Flight of the Conchords" is the calendar's "Flight of the
/// Conchords – New Zealand Tour 2026".
fn same_subject(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_lowercase(), b.trim().to_lowercase());
    !a.is_empty() && !b.is_empty() && (a.contains(&b) || b.contains(&a))
}

fn already(r: &Reader, account: AccountId, p: &ProposedEvent) -> Result<Option<i64>, CommandError> {
    if let Some(uid) = &p.uid {
        if let Some(e) = r
            .events_by_uid(account, uid)
            .map_err(store_err)?
            .into_iter()
            .next()
        {
            return Ok(Some(e.id.0));
        }
    }
    let Some(w) = events::parse_when(&p.start, NaiveDate::MIN, None) else {
        return Ok(None);
    };
    let (start, range) = match w.time {
        Some(t) => {
            let at = instant(w.date, t, &p.time_zone);
            (at, EventRange::utc(at - 60, at + 61))
        }
        None => {
            let d = date_secs(w.date);
            (d, EventRange::utc(d, d + 86_400))
        }
    };
    let cals: Vec<_> = r
        .calendars(account)
        .map_err(store_err)?
        .into_iter()
        .map(|c| c.id)
        .collect();
    Ok(r.events_in(&cals, range)
        .map_err(store_err)?
        .into_iter()
        .find(|e| {
            // The range finds events that overlap; only the same start counts.
            (e.data.start_at - start).abs() <= 60
                && same_subject(&e.data.subject, &p.subject)
                && e.data.is_all_day == p.all_day
        })
        .map(|e| e.id.0))
}

/// Runs `fam cal propose` against the store and famd at `paths`.
pub fn propose_at(
    paths: &DaemonPaths,
    cfg: Option<&familiar_config::Config>,
    input: &CalProposeInput,
    now: i64,
) -> Result<CalProposeOutput, CommandError> {
    let zone = zone_name(input.tz.as_deref())?;
    let r = open(paths)?;
    let m = r
        .message(MessageId(input.from_mail))
        .map_err(store_err)?
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::NotFound,
                format!("no message {} on this device", input.from_mail),
            )
        })?;
    let (key, remote) = locate(paths, input.from_mail)?;
    let mut out = CalProposeOutput {
        message: input.from_mail,
        account: key.clone(),
        source: "none".into(),
        events: Vec::new(),
        invitation: None,
        notes: Vec::new(),
    };

    match invitation(paths, &r, m.account, &key, &remote, &zone) {
        Ok(Some(inv)) => {
            if inv.event.is_none() {
                out.notes.push(
                    "this meeting hasn't reached your calendar on this computer yet; try again in a few minutes"
                        .into(),
                );
            }
            out.source = "invite".into();
            out.invitation = Some(inv);
            return Ok(out);
        }
        Ok(None) => {}
        Err(e) => out.notes.push(note_for("invitations", &e)),
    }

    // Only ask for attachments when there are any, and not again when famd
    // was already found to be down.
    let daemon_down = out
        .notes
        .iter()
        .any(|n| n.starts_with("famd isn't running"));
    let mut cancels = false;
    let mut list = Vec::new();
    if m.has_attachments && !daemon_down {
        match list_at(paths, input.from_mail) {
            Ok(l) => list = l,
            Err(e) => out.notes.push(note_for("attachments", &e)),
        }
    }
    if !list.is_empty() {
        match ics_events(paths, input.from_mail, &list, &zone, &mut out.notes) {
            Ok((evs, c)) => {
                cancels = c;
                if !evs.is_empty() {
                    out.source = "ics".into();
                    out.events = evs;
                }
            }
            Err(e) => out.notes.push(note_for("attachments", &e)),
        }
    }

    // A cancellation repeats the date it cancels: don't offer to add it.
    let mut asked_ai = false;
    if out.events.is_empty() && !cancels {
        let (subject, text, at, _) = mail_text(&r, input.from_mail)?;
        // "Tonight" and "Saturday" are from when it was sent.
        let sent = if at > 0 {
            today_in(at, &zone)
        } else {
            today_in(now, &zone)
        };
        if let Some((tool, settings)) = ai_tool(cfg, &key, input.no_ai) {
            let (pdfs, others) = pdf_texts(paths, input.from_mail, &list, &mut out.notes);
            let from = if m.from.name.is_empty() {
                m.from.addr.clone()
            } else {
                format!("{} <{}>", m.from.name, m.from.addr)
            };
            let prompt = ai::prompt(
                &ai::Email {
                    subject: &subject,
                    from: &from,
                    received: &sent.format("%A %-d %B %Y").to_string(),
                    body: &text,
                    pdfs: &pdfs,
                    others: &others,
                },
                &today_in(now, &zone).format("%A %-d %B %Y").to_string(),
                &zone,
            );
            let timeout = std::time::Duration::from_secs(u64::from(settings.timeout_seconds));
            match ai::ask(&tool, settings.model.as_deref(), &prompt, timeout)
                .and_then(|v| ai::parse_answer(tool.kind, v))
            {
                Ok(answer) => {
                    asked_ai = true;
                    out.source = format!("ai:{}", tool.kind.name());
                    out.events = ai_events(tool.kind, answer, sent, &zone, &mut out.notes);
                    if out.events.is_empty() {
                        out.notes
                            .push(format!("{} found no events", tool.kind.title()));
                    }
                }
                Err(why) => out
                    .notes
                    .push(format!("{why}; showing Familiar's own guess")),
            }
        }
    }
    if out.events.is_empty() && !cancels && !asked_ai {
        let (subject, text, at, _) = mail_text(&r, input.from_mail)?;
        let sent = if at > 0 {
            today_in(at, &zone)
        } else {
            today_in(now, &zone)
        };
        let s = events::suggest(&subject, &text, sent);
        match s.date {
            Some(date) => {
                out.source = "heuristic".into();
                out.events.push(ProposedEvent {
                    subject: if s.subject.trim().is_empty() {
                        "(no subject)".into()
                    } else {
                        s.subject
                    },
                    start: stamp(date, s.time),
                    end: None,
                    all_day: s.time.is_none(),
                    time_zone: zone.clone(),
                    location: s.location,
                    notes: None,
                    repeat: None,
                    evidence: None,
                    uid: None,
                    already: None,
                });
            }
            None => out.notes.push("no date found in this email".into()),
        }
    }

    for p in &mut out.events {
        p.already = already(&r, m.account, p)?;
    }
    Ok(out)
}

impl Command for CalPropose {
    type Input = CalProposeInput;
    type Output = CalProposeOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["cal", "propose"],
        summary: "Suggest calendar events from an email, without adding them",
        description: "Reads an email and says which calendar events it suggests, for you to check before adding them with fam cal add. A meeting request comes back as an invitation to answer with fam cal rsvp, not as a new event. Otherwise the events in its .ics attachments; else, with [ai] enabled in config.toml, the events the AI tool (Claude or Codex) finds in its text and PDFs; else the date, time and place found in its text. Each event says whether it is already in your calendar. Nothing is changed. Attachments and invitations are read through famd; without famd only the text is used.",
        access: Access::Read,
        errors: &[ErrorKind::NotFound, ErrorKind::Usage],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: CalProposeInput,
    ) -> Result<CalProposeOutput, CommandError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let cfg = config()?;
        propose_at(&DaemonPaths::resolve()?, cfg.as_ref(), &input, now)
    }

    fn render_text(&self, o: &CalProposeOutput) -> String {
        let mut lines = Vec::new();
        if let Some(inv) = &o.invitation {
            let what = if inv.kind == "cancelled" {
                "Meeting cancelled"
            } else {
                "Meeting request"
            };
            match (inv.event, &inv.subject, &inv.when) {
                (Some(id), Some(s), Some(w)) => {
                    lines.push(format!("{what}: {s}, {w} (event {id})"));
                    if let Some(c) = &inv.calendar {
                        lines.push(format!("  in {c}"));
                    }
                    if let Some(resp) = &inv.response {
                        lines.push(format!("  your answer so far: {resp}"));
                    }
                    lines.push(if inv.kind == "cancelled" {
                        format!("  remove it with: fam cal delete {id}")
                    } else {
                        format!("  answer with: fam cal rsvp {id} accept|tentative|decline")
                    });
                }
                _ => lines.push(what.to_string()),
            }
        }
        if o.events.is_empty() && o.invitation.is_none() {
            lines.push(format!("No events found in message {}.", o.message));
        }
        for e in &o.events {
            let mut when = e.start.clone();
            if let Some(end) = &e.end {
                when.push_str(&format!(" to {end}"));
            }
            if e.all_day {
                when.push_str(" (all day)");
            }
            lines.push(format!("{}  {when}  {}", e.subject, e.time_zone));
            if let Some(l) = &e.location {
                lines.push(format!("  at {l}"));
            }
            if let Some(r) = &e.repeat {
                let mut s = format!("  repeats {}", r.every);
                if r.interval > 1 {
                    s.push_str(&format!(" every {}", r.interval));
                }
                if !r.on.is_empty() {
                    s.push_str(&format!(" on {}", r.on.join(",")));
                }
                if let Some(u) = &r.until {
                    s.push_str(&format!(" until {u}"));
                }
                if let Some(c) = r.count {
                    s.push_str(&format!(", {c} times"));
                }
                lines.push(s);
            }
            if let Some(id) = e.already {
                lines.push(format!("  already in your calendar (event {id})"));
            }
        }
        for n in &o.notes {
            lines.push(format!("Note: {n}"));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::same_subject;

    #[test]
    fn a_shorter_subject_matches_a_longer_one() {
        assert!(same_subject(
            "Flight of the Conchords – New Zealand Tour 2026",
            "flight of the conchords"
        ));
        assert!(same_subject("Dentist", "Dentist"));
        assert!(!same_subject("Dentist", "Haircut"));
        assert!(!same_subject("", "Dentist"));
    }
}
