// SPDX-License-Identifier: MIT OR Apache-2.0
//! Google Calendar event mutations used by the account op runner.
use chrono::{DateTime, NaiveDate};
use familiar_core::compose::Recipient;
use familiar_core::events::{
    EventChanges, EventFields, EventOp, Recurrence, RepeatKind, Rsvp, instant,
};
use familiar_store::EventData;
use reqwest::Method;
use serde_json::{Value, json};

use crate::{Error, GmailApi, Priority, Result, api::enc};

fn invalid(message: &str) -> Error {
    Error::Graph {
        status: 400,
        code: "invalidEvent".into(),
        message: message.into(),
        request_id: None,
    }
}

fn time(at: i64, date: bool, zone: &str) -> Result<Value> {
    let utc = DateTime::from_timestamp(at, 0).ok_or_else(|| invalid("event time out of range"))?;
    Ok(if date {
        json!({"date": utc.date_naive()})
    } else {
        json!({"dateTime": utc.to_rfc3339(), "timeZone":zone})
    })
}

fn rule(r: &Recurrence, date: bool, zone: &str) -> Result<String> {
    let freq = match r.every {
        RepeatKind::Daily => "DAILY",
        RepeatKind::Weekly => "WEEKLY",
        RepeatKind::Monthly => "MONTHLY",
        RepeatKind::Yearly => "YEARLY",
    };
    let mut s = format!("RRULE:FREQ={freq};INTERVAL={}", r.interval);
    if r.every == RepeatKind::Weekly && !r.days.is_empty() {
        let days = r
            .days
            .iter()
            .map(|d| match d.to_ascii_lowercase().as_str() {
                "monday" => Ok("MO"),
                "tuesday" => Ok("TU"),
                "wednesday" => Ok("WE"),
                "thursday" => Ok("TH"),
                "friday" => Ok("FR"),
                "saturday" => Ok("SA"),
                "sunday" => Ok("SU"),
                _ => Err(invalid("invalid recurrence day")),
            })
            .collect::<Result<Vec<_>>>()?;
        s.push_str(&format!(";BYDAY={}", days.join(",")));
    }
    if let Some(n) = r.count {
        s.push_str(&format!(";COUNT={n}"));
    }
    if let Some(end) = &r.until {
        let d = NaiveDate::parse_from_str(end, "%Y-%m-%d")
            .map_err(|_| invalid("invalid recurrence end date"))?;
        let until = if date {
            d.format("%Y%m%d").to_string()
        } else {
            let at = instant(d, d.and_hms_opt(23, 59, 59).unwrap().time(), zone);
            DateTime::from_timestamp(at, 0)
                .unwrap()
                .format("%Y%m%dT%H%M%SZ")
                .to_string()
        };
        s.push_str(&format!(";UNTIL={until}"));
    }
    Ok(s)
}

fn guests(required: &[Recipient], optional: &[Recipient]) -> Value {
    Value::Array(
        required
            .iter()
            .map(|r| (r, false))
            .chain(optional.iter().map(|r| (r, true)))
            .map(|(r, opt)| json!({"email":r.address,"displayName":r.name,"optional":opt}))
            .collect(),
    )
}

fn settings(body: &mut Value, show: Option<&str>, minutes: Option<i64>, off: bool) -> Result<()> {
    if let Some(s) = show {
        body["transparency"] = json!(match s {
            "free" => "transparent",
            "busy" => "opaque",
            _ =>
                return Err(invalid(
                    "Google Calendar supports free or busy availability"
                )),
        });
    }
    if off || minutes.is_some() {
        let overrides = if off {
            vec![]
        } else {
            vec![json!({"method":"popup","minutes":minutes})]
        };
        body["reminders"] = json!({"useDefault":false,"overrides":overrides});
    }
    Ok(())
}

fn create_body(f: &EventFields) -> Result<Value> {
    if !f.categories.is_empty() {
        return Err(invalid(
            "Google Calendar does not support Outlook categories",
        ));
    }
    let mut body = json!({"summary":f.subject,"description":f.body_html,"location":f.location,
        "start":time(f.start_at,f.all_day,&f.time_zone)?, "end":time(f.end_at,f.all_day,&f.time_zone)?,
        "attendees":guests(&f.attendees,&f.optional)});
    settings(
        &mut body,
        f.show_as.as_deref(),
        f.reminder_minutes,
        f.no_reminder,
    )?;
    if let Some(r) = &f.recurrence {
        body["recurrence"] = json!([rule(r, f.all_day, &f.time_zone)?]);
    }
    Ok(body)
}

fn changes(c: &EventChanges, before: &EventData) -> Result<Value> {
    let mut b = json!({});
    for (key, value) in [
        ("summary", &c.subject),
        ("description", &c.body_html),
        ("location", &c.location),
    ] {
        if let Some(s) = value {
            b[key] = json!(s);
        }
    }
    if c.categories.is_some() {
        return Err(invalid(
            "Google Calendar does not support Outlook categories",
        ));
    }
    if let Some(a) = &c.attendees {
        b["attendees"] = guests(a, &[]);
    }
    for (key, at, old, zone) in [
        ("start", c.start_at, before.start_at, &before.start_tz),
        ("end", c.end_at, before.end_at, &before.end_tz),
    ] {
        if at.is_some() || c.all_day.is_some() || c.time_zone.is_some() {
            b[key] = time(
                at.unwrap_or(old),
                c.all_day.unwrap_or(before.is_all_day),
                c.time_zone.as_deref().or(zone.as_deref()).unwrap_or("UTC"),
            )?;
        }
    }
    settings(
        &mut b,
        c.show_as.as_deref(),
        c.reminder_minutes,
        c.no_reminder,
    )?;
    Ok(b)
}

fn collection(calendar: &str) -> String {
    format!("/calendars/{}/events", enc(calendar))
}
fn event(calendar: &str, id: &str) -> String {
    format!("{}/{}", collection(calendar), enc(id))
}

impl GmailApi {
    /// Insert once across retries, returning the first instance for a series.
    pub async fn insert_event(
        &self,
        calendar: &str,
        op: &EventOp,
        source: Option<&str>,
    ) -> Result<Value> {
        // The transaction ID is hex plus a hyphen. 'g' is legal base32hex
        // and distinct from every hex digit, preserving the separator.
        let id = op.transaction_id.replace('-', "g");
        let mut b = create_body(&op.fields)?;
        b["id"] = json!(id);
        if let Some(html) = source {
            b["description"] = json!(format!("{}{html}", op.fields.body_html));
        }
        let path = format!("{}?sendUpdates=all", collection(calendar));
        let pr = Priority::User;
        let created = match self
            .client()
            .calendar_request(Method::POST, &path, Some(&b), pr)
            .await
        {
            Err(Error::Graph { status: 409, .. }) => {
                self.client()
                    .calendar_request(Method::GET, &event(calendar, &id), None, pr)
                    .await?
            }
            r => r?,
        };
        if op.fields.recurrence.is_none() {
            return Ok(created);
        }
        let path = format!("{}/instances?maxResults=1", event(calendar, &id));
        let result = self
            .client()
            .calendar_request(Method::GET, &path, None, pr)
            .await?;
        result["items"]
            .get(0)
            .cloned()
            .ok_or_else(|| Error::Decode("created series has no instances".into()))
    }

    /// Patch only fields selected by the queued edit.
    pub async fn edit_event(
        &self,
        cal: &str,
        id: &str,
        c: &EventChanges,
        before: &EventData,
    ) -> Result<()> {
        let b = changes(c, before)?;
        self.client()
            .calendar_request(
                Method::PATCH,
                &format!("{}?sendUpdates=all", event(cal, id)),
                Some(&b),
                Priority::User,
            )
            .await?;
        Ok(())
    }

    /// Change only the attendee marked as self on this calendar copy.
    pub async fn rsvp_event(
        &self,
        cal: &str,
        id: &str,
        response: Rsvp,
        comment: Option<&str>,
        notify: bool,
    ) -> Result<()> {
        let path = event(cal, id);
        let copy = self
            .client()
            .calendar_request(Method::GET, &path, None, Priority::User)
            .await?;
        let email = copy["attendees"]
            .as_array()
            .and_then(|a| a.iter().find(|v| v["self"] == true))
            .and_then(|v| v["email"].as_str())
            .ok_or_else(|| invalid("no attendee marked as self on this calendar"))?;
        let b = response_body(email, response, comment);
        let updates = if notify { "all" } else { "none" };
        self.client()
            .calendar_request(
                Method::PATCH,
                &format!("{path}?sendUpdates={updates}"),
                Some(&b),
                Priority::User,
            )
            .await?;
        Ok(())
    }

    /// Delete an occurrence or a series, accepting an already deleted event.
    pub async fn remove_event(&self, cal: &str, id: &str) -> Result<()> {
        match self
            .client()
            .calendar_request(
                Method::DELETE,
                &format!("{}?sendUpdates=all", event(cal, id)),
                None,
                Priority::User,
            )
            .await
        {
            Ok(_)
            | Err(
                Error::NotFound(_) | Error::SyncStateGone(_) | Error::Graph { status: 410, .. },
            ) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

fn response_body(email: &str, response: Rsvp, comment: Option<&str>) -> Value {
    let status = match response {
        Rsvp::Accept => "accepted",
        Rsvp::Tentative => "tentative",
        Rsvp::Decline => "declined",
    };
    let mut own = json!({"email":email,"responseStatus":status});
    if let Some(text) = comment {
        own["comment"] = json!(text);
    }
    json!({"attendeesOmitted":true,"attendees":[own]})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn weekly_until_is_inclusive_in_local_zone() {
        let r = Recurrence {
            every: RepeatKind::Weekly,
            interval: 2,
            days: vec!["monday".into(), "friday".into()],
            until: Some("2026-12-31".into()),
            count: None,
        };
        assert_eq!(
            rule(&r, false, "Pacific/Auckland").unwrap(),
            "RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,FR;UNTIL=20261231T105959Z"
        );
        assert!(
            rule(&r, true, "Pacific/Auckland")
                .unwrap()
                .ends_with("UNTIL=20261231")
        );
    }
    #[test]
    fn rsvp_does_not_replace_other_guests() {
        assert_eq!(
            response_body("me@example.org", Rsvp::Tentative, Some("maybe")),
            json!({"attendeesOmitted":true,"attendees":[{"email":"me@example.org","responseStatus":"tentative","comment":"maybe"}]})
        );
    }
    #[test]
    fn all_day_uses_exclusive_end_and_encodes_calendar_ids() {
        let f = EventFields {
            all_day: true,
            start_at: 0,
            end_at: 86400,
            time_zone: "America/Chicago".into(),
            ..Default::default()
        };
        let b = create_body(&f).unwrap();
        assert_eq!(b["start"], json!({"date":"1970-01-01"}));
        assert_eq!(b["end"], json!({"date":"1970-01-02"}));
        assert_eq!(
            event("a/b@example.org", "iid"),
            "/calendars/a%2Fb%40example.org/events/iid"
        );
    }
}
