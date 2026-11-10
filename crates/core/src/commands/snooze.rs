// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam mail snooze`, `fam mail unsnooze` and `fam mail snoozed`.
//!
//! Snoozing moves a message into its account's Snoozed folder (made at the
//! top level the first time) and notes when it comes back. famd moves it
//! back to the folder it came from, marked unread, when the time comes, so
//! the computer has to be on (famd running) then; a snooze that came due
//! while it was off comes back when famd next starts.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Timelike, Utc, Weekday};
use familiar_store::{MessageId, Reader};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::commands::cal_write::zone_name;
use crate::commands::write::{WRITE_ERRORS, flags, submit_at};
use crate::daemon::DaemonPaths;
use crate::error::{CommandError, ErrorKind};
use crate::events::{instant, parse_weekday, parse_when, today_in};
use crate::ops::{WriteOutput, WriteRequest};
use crate::registry::{Access, Command, CommandSpec, Context};
use crate::tz;

/// The time of day a snooze to a date alone comes back.
const MORNING: (u32, u32) = (9, 0);
/// The time "tonight" means.
const EVENING: (u32, u32) = (18, 0);

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn hm((h, m): (u32, u32)) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).expect("valid time")
}

/// Reads a snooze time, in `zone`: `2h`, `3d`, `1w` or `90m` from now;
/// `tonight` (6 pm); `tomorrow`, `weekend` (Saturday) or `next week`
/// (Monday), at 9 am; or anything `fam cal add --start` takes, such as
/// `mon 9am`, `fri 2pm` or `2026-11-21 08:30` (a date alone is 9 am). A day
/// name whose time today has passed means next week's.
pub fn parse_until(s: &str, now: i64, zone: &str) -> Result<i64, CommandError> {
    let bad = || {
        CommandError::usage(format!(
            "--until {s:?}: use a time such as \"mon 9am\", \"tomorrow\", \"tonight\", \"3h\" or \"2026-11-21 08:30\""
        ))
    };
    let t = s.trim().to_ascii_lowercase();
    let today = today_in(now, zone);
    let at = |d: NaiveDate, time: (u32, u32)| instant(d, hm(time), zone);
    let next = |wd: Weekday| {
        let ahead = (7 + i64::from(wd.num_days_from_monday())
            - i64::from(today.weekday().num_days_from_monday()))
            % 7;
        today + Duration::days(if ahead == 0 { 7 } else { ahead })
    };
    let until = if let Some(secs) = relative(&t) {
        now + secs
    } else {
        match t.as_str() {
            "tonight" | "this evening" => at(today, EVENING),
            "weekend" | "this weekend" => at(next(Weekday::Sat), MORNING),
            "next week" => at(next(Weekday::Mon), MORNING),
            _ => {
                let w = parse_when(&t, today, None).ok_or_else(bad)?;
                let mut u = at(w.date, w.time.map_or(MORNING, |x| (x.hour(), x.minute())));
                let names_day = t.split_whitespace().next().and_then(parse_weekday);
                if u <= now && names_day.is_some() {
                    u += 7 * 86_400;
                }
                u
            }
        }
    };
    if until <= now {
        return Err(CommandError::usage(format!(
            "--until {s:?} has already passed"
        )));
    }
    Ok(until)
}

/// `90m`, `2h`, `3d`, `1w` (or `+2h`, `2 hours`, `3 days`).
fn relative(t: &str) -> Option<i64> {
    let t = t.trim_start_matches('+').trim();
    let split = t.find(|c: char| !c.is_ascii_digit())?;
    let n: i64 = t[..split].parse().ok()?;
    let unit = match t[split..].trim() {
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600,
        "d" | "day" | "days" => 86_400,
        "w" | "week" | "weeks" => 7 * 86_400,
        _ => return None,
    };
    (n > 0).then_some(n * unit)
}

/// A time in `zone`, for people: `Mon 16 Nov 09:00`.
fn local_text(at: i64, zone: &str) -> String {
    let z = tz::parse_zone(zone).unwrap_or(chrono_tz::UTC);
    DateTime::<Utc>::from_timestamp(at, 0)
        .unwrap_or_default()
        .with_timezone(&z)
        .format("%a %-d %b %H:%M")
        .to_string()
}

/// `fam mail snooze`.
pub struct MailSnooze;

/// Input for `fam mail snooze`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailSnoozeInput {
    /// Local message IDs.
    #[schemars(extend("x-positional" = 0))]
    pub messages: Vec<i64>,
    /// When they come back: "mon 9am", "tomorrow" (9 am), "tonight" (6 pm),
    /// "weekend", "next week", "3h", "2d", "2026-11-21 08:30".
    pub until: String,
    /// Time zone for --until (IANA or Windows name; default this computer's).
    #[serde(default)]
    pub tz: Option<String>,
    /// Only show what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the move (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

/// What `fam mail snooze` did.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnoozeOutput {
    /// When the messages come back (RFC 3339).
    pub until: String,
    /// The same, in local time for people.
    pub until_local: String,
    /// The moves into Snoozed.
    pub write: WriteOutput,
}

impl Command for MailSnooze {
    type Input = MailSnoozeInput;
    type Output = SnoozeOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "snooze"],
        summary: "Hide messages in Snoozed until a time, then bring them back",
        description: "Moves messages to their account's Snoozed folder (made at the top level the first time), on this device at once and queued for the server. At --until famd moves each one back to the folder it came from, marked unread. famd must be running then; a snooze that comes due while it isn't comes back when it next starts. Snoozing a snoozed message again only changes the time. A message moved out of Snoozed by hand stays where it was put.",
        access: Access::Write,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: MailSnoozeInput) -> Result<SnoozeOutput, CommandError> {
        let zone = zone_name(i.tz.as_deref())?;
        let until = parse_until(&i.until, now(), &zone)?;
        let write = submit_at(
            &DaemonPaths::resolve()?,
            WriteRequest::Snooze {
                messages: i.messages,
                until,
            },
            &flags(i.dry_run, i.wait, i.wait_secs),
        )?;
        Ok(SnoozeOutput {
            until: crate::commands::auth::rfc3339(until as u64),
            until_local: local_text(until, &zone),
            write,
        })
    }

    fn render_text(&self, o: &SnoozeOutput) -> String {
        let mut s = o.write.render();
        if !s.ends_with('\n') {
            s.push('\n');
        }
        let verb = if o.write.dry_run {
            "would come"
        } else {
            "comes"
        };
        s.push_str(&format!("{verb} back {}\n", o.until_local));
        s
    }
}

/// `fam mail unsnooze`.
pub struct MailUnsnooze;

/// Input for `fam mail unsnooze`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailUnsnoozeInput {
    /// Local message IDs.
    #[schemars(extend("x-positional" = 0))]
    pub messages: Vec<i64>,
    /// Only show what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the move (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for MailUnsnooze {
    type Input = MailUnsnoozeInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "unsnooze"],
        summary: "Bring snoozed messages back now",
        description: "Ends the snooze of each message and moves it out of Snoozed to the folder it came from (the Inbox if that folder is gone, or if it was put in Snoozed by hand).",
        access: Access::Write,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: MailUnsnoozeInput) -> Result<WriteOutput, CommandError> {
        submit_at(
            &DaemonPaths::resolve()?,
            WriteRequest::Unsnooze {
                messages: i.messages,
                mark_unread: false,
            },
            &flags(i.dry_run, i.wait, i.wait_secs),
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

/// `fam mail snoozed`.
pub struct MailSnoozed;

/// Input for `fam mail snoozed`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailSnoozedInput {
    /// Only this account (its name in config.toml).
    #[serde(default)]
    pub account: Option<String>,
    /// Time zone for the times shown (default this computer's).
    #[serde(default)]
    pub tz: Option<String>,
}

/// One snoozed message.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnoozedView {
    /// Local message ID.
    pub id: i64,
    /// Account key.
    pub account: String,
    /// Subject.
    pub subject: String,
    /// Sender's name, or address when there's no name.
    pub from: String,
    /// When it comes back (RFC 3339).
    pub until: String,
    /// The same, in local time.
    pub until_local: String,
    /// When it comes back, Unix seconds.
    pub until_unix: i64,
}

/// What `fam mail snoozed` lists.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnoozedOutput {
    /// Soonest back first.
    pub messages: Vec<SnoozedView>,
}

/// Lists the snoozes in the store at `paths`.
pub fn snoozed_at(
    paths: &DaemonPaths,
    i: &MailSnoozedInput,
) -> Result<SnoozedOutput, CommandError> {
    let store_err = |e: familiar_store::Error| CommandError::internal(format!("store: {e}"));
    let zone = zone_name(i.tz.as_deref())?;
    let r = match Reader::open(&paths.store) {
        Ok(r) => r,
        Err(familiar_store::Error::NoStore(_)) => {
            return Ok(SnoozedOutput { messages: vec![] });
        }
        Err(e) => return Err(store_err(e)),
    };
    let accounts = r.accounts().map_err(store_err)?;
    if let Some(k) = &i.account {
        if !accounts.iter().any(|a| a.key == *k) {
            return Err(CommandError::new(
                ErrorKind::NotFound,
                format!("account {k:?} has not synced on this device"),
            ));
        }
    }
    let snoozes = r.snoozes().map_err(store_err)?;
    let ids: Vec<MessageId> = snoozes.iter().map(|s| s.message).collect();
    let msgs = r.messages_by_ids(&ids).map_err(store_err)?;
    let mut out = Vec::new();
    for s in snoozes {
        let Some(m) = msgs.iter().find(|m| m.id == s.message) else {
            continue;
        };
        let key = accounts
            .iter()
            .find(|a| a.id == m.account)
            .map(|a| a.key.clone())
            .unwrap_or_default();
        if i.account.as_ref().is_some_and(|k| *k != key) {
            continue;
        }
        out.push(SnoozedView {
            id: m.id.0,
            account: key,
            subject: m.subject.clone(),
            from: if m.from.name.is_empty() {
                m.from.addr.clone()
            } else {
                m.from.name.clone()
            },
            until: crate::commands::auth::rfc3339(s.until.max(0) as u64),
            until_local: local_text(s.until, &zone),
            until_unix: s.until,
        });
    }
    Ok(SnoozedOutput { messages: out })
}

impl Command for MailSnoozed {
    type Input = MailSnoozedInput;
    type Output = SnoozedOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "snoozed"],
        summary: "List snoozed messages and when they come back",
        description: "Lists the messages snoozed with fam mail snooze, soonest back first. Reads the store, so it works without famd.",
        access: Access::Read,
        errors: &[ErrorKind::Usage, ErrorKind::NotFound],
    };

    fn run(&self, _ctx: &Context<'_>, i: MailSnoozedInput) -> Result<SnoozedOutput, CommandError> {
        snoozed_at(&DaemonPaths::resolve()?, &i)
    }

    fn render_text(&self, o: &SnoozedOutput) -> String {
        if o.messages.is_empty() {
            return "nothing snoozed\n".into();
        }
        let mut s = String::new();
        for m in &o.messages {
            s.push_str(&format!(
                "{:>7}  {}  {}  {}: {}\n",
                m.id, m.until_local, m.account, m.from, m.subject
            ));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const Z: &str = "Pacific/Auckland";

    fn local(s: &str) -> i64 {
        let dt = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        tz::local_to_utc(dt, tz::parse_zone(Z).unwrap()).timestamp()
    }

    #[test]
    fn reads_snooze_times() {
        // Tuesday 10 November 2026, 10:00 in Auckland.
        let now = local("2026-11-10 10:00");
        let p = |s| parse_until(s, now, Z).unwrap();
        assert_eq!(p("3h"), now + 3 * 3600);
        assert_eq!(p("2 days"), now + 2 * 86_400);
        assert_eq!(p("tonight"), local("2026-11-10 18:00"));
        assert_eq!(p("tomorrow"), local("2026-11-11 09:00"));
        assert_eq!(p("mon 9am"), local("2026-11-16 09:00"));
        assert_eq!(p("Monday 9am"), local("2026-11-16 09:00"));
        assert_eq!(p("next week"), local("2026-11-16 09:00"));
        assert_eq!(p("weekend"), local("2026-11-14 09:00"));
        assert_eq!(p("2026-11-21 08:30"), local("2026-11-21 08:30"));
        assert_eq!(p("4pm"), local("2026-11-10 16:00"));
        // Today's name with a time gone by: next week.
        assert_eq!(p("tue 9am"), local("2026-11-17 09:00"));
        assert_eq!(p("tue 11am"), local("2026-11-10 11:00"));
        assert!(parse_until("9am", now, Z).is_err());
        assert!(parse_until("2026-11-01", now, Z).is_err());
        assert!(parse_until("someday", now, Z).is_err());
        assert!(parse_until("0h", now, Z).is_err());
    }
}
