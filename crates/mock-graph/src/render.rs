// SPDX-License-Identifier: MIT OR Apache-2.0
//! Graph JSON shapes, `$select`, `$filter` and `$search` evaluation.

use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use serde_json::{Map, Value, json};

use crate::model::{
    Calendar, EmailAddress, EventRecord, FolderRecord, MailboxState, MessageRecord, User, fnv1a,
};

/// Options from the `Prefer` header.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Prefer {
    pub max_page: Option<usize>,
    pub immutable_ids: bool,
    pub text_body: bool,
}

impl Prefer {
    pub fn parse(values: &[&str]) -> Self {
        let mut p = Self::default();
        for part in values.iter().flat_map(|v| v.split(',')) {
            let part = part.trim();
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            let v = v.trim().trim_matches('"');
            match k.trim().to_ascii_lowercase().as_str() {
                "odata.maxpagesize" => p.max_page = v.parse().ok(),
                "idtype" => p.immutable_ids = v.eq_ignore_ascii_case("ImmutableId"),
                "outlook.body-content-type" => p.text_body = v.eq_ignore_ascii_case("text"),
                _ => {}
            }
        }
        p
    }

    pub fn applied(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(n) = self.max_page {
            parts.push(format!("odata.maxpagesize={n}"));
        }
        if self.immutable_ids {
            parts.push("IdType=\"ImmutableId\"".to_string());
        }
        if self.text_body {
            parts.push("outlook.body-content-type=\"text\"".to_string());
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

pub(crate) fn dt(t: &DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Graph's dateTimeTimeZone format: no offset, seven fractional digits.
fn dt_tz(t: &DateTime<Utc>) -> Value {
    let ticks = t.timestamp_subsec_nanos() / 100;
    json!({
        "dateTime": format!("{}.{ticks:07}", t.format("%Y-%m-%dT%H:%M:%S")),
        "timeZone": "UTC",
    })
}

/// Parses `2026-11-04T08:00:00Z`, an offset form, or a bare local time taken as UTC.
pub(crate) fn parse_dt(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Utc));
    }
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
        .ok()
        .map(|n| n.and_utc())
}

fn addr(a: &EmailAddress) -> Value {
    json!({ "emailAddress": { "name": a.name, "address": a.address } })
}

fn etag(change_key: u64, id: &str) -> String {
    format!("W/\"CQAAAB{:012x}\"", fnv1a(id.as_bytes()) ^ change_key)
}

fn change_key(change_key: u64, id: &str) -> String {
    format!("CQAAAB{:012x}", fnv1a(id.as_bytes()) ^ change_key)
}

/// A deterministic synthetic body for messages without one.
pub(crate) fn synthetic_body(id: &str, subject: &str) -> String {
    const WORDS: &[&str] = &[
        "please", "find", "attached", "the", "notes", "from", "today", "meeting", "about", "next",
        "week", "schedule", "thanks", "for", "your", "help", "with", "this", "we", "should",
        "confirm", "budget", "before", "friday", "team", "practice", "invoice", "update", "review",
        "draft",
    ];
    let mut h = fnv1a(id.as_bytes());
    let mut words = Vec::with_capacity(60);
    for _ in 0..(30 + (h % 40) as usize) {
        h ^= h << 13;
        h ^= h >> 7;
        h ^= h << 17;
        words.push(WORDS[(h % WORDS.len() as u64) as usize]);
    }
    format!(
        "<html><body><p>{}</p><p>{}.</p></body></html>",
        html_escape(subject),
        words.join(" ")
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Crude HTML to text, enough for `outlook.body-content-type="text"` and previews.
pub(crate) fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

pub(crate) fn message_body(rec: &MessageRecord) -> String {
    rec.msg
        .body_html
        .clone()
        .unwrap_or_else(|| synthetic_body(&rec.msg.id, &rec.msg.subject))
}

pub(crate) fn message(rec: &MessageRecord, prefer: &Prefer) -> Value {
    let m = &rec.msg;
    let id = if prefer.immutable_ids {
        &m.id
    } else {
        &rec.rest_id
    };
    let html = message_body(rec);
    let text = html_to_text(&html);
    let preview: String = text.chars().take(255).collect();
    let body = if prefer.text_body {
        json!({ "contentType": "text", "content": text })
    } else {
        json!({ "contentType": "html", "content": html })
    };
    json!({
        "@odata.type": "#microsoft.graph.message",
        "@odata.etag": etag(rec.change_key, &m.id),
        "id": id,
        "createdDateTime": dt(&rec.created),
        "lastModifiedDateTime": dt(&rec.modified),
        "changeKey": change_key(rec.change_key, &m.id),
        "categories": m.categories,
        "receivedDateTime": dt(&m.received_date_time),
        "sentDateTime": dt(&m.sent_date_time.unwrap_or(m.received_date_time)),
        "hasAttachments": m.has_attachments,
        "internetMessageId": m.internet_message_id,
        "subject": m.subject,
        "bodyPreview": preview,
        "importance": m.importance,
        "parentFolderId": m.folder_id,
        "conversationId": m.conversation_id,
        "isDeliveryReceiptRequested": false,
        "isReadReceiptRequested": false,
        "isRead": m.is_read,
        "isDraft": m.is_draft,
        "webLink": format!("https://outlook.invalid/owa/?ItemID={}", m.id),
        "inferenceClassification": "focused",
        "body": body,
        "sender": addr(&m.from),
        "from": addr(&m.from),
        "toRecipients": m.to.iter().map(addr).collect::<Vec<_>>(),
        "ccRecipients": m.cc.iter().map(addr).collect::<Vec<_>>(),
        "bccRecipients": [],
        "replyTo": [],
        "flag": { "flagStatus": if m.flagged { "flagged" } else { "notFlagged" } },
    })
}

pub(crate) fn removed(odata_type: &str, id: &str) -> Value {
    json!({
        "@odata.type": odata_type,
        "id": id,
        "@removed": { "reason": "deleted" },
    })
}

pub(crate) fn folder(st: &MailboxState, rec: &FolderRecord) -> Value {
    let f = &rec.folder;
    let msgs = st.by_folder.get(&f.id);
    let total = msgs.map_or(0, |s| s.len());
    let unread = msgs.map_or(0, |s| {
        s.iter()
            .filter(|(_, id)| st.messages.get(id).is_some_and(|m| !m.msg.is_read))
            .count()
    });
    let children = st
        .folders
        .iter()
        .filter(|r| r.folder.parent_folder_id.as_deref() == Some(f.id.as_str()))
        .count();
    json!({
        "@odata.type": "#microsoft.graph.mailFolder",
        "id": f.id,
        "displayName": f.display_name,
        "parentFolderId": f.parent_folder_id,
        "childFolderCount": children,
        "unreadItemCount": unread,
        "totalItemCount": total,
        "sizeInBytes": total * 4096,
        "isHidden": f.is_hidden,
    })
}

pub(crate) fn calendar(user: &User, c: &Calendar) -> Value {
    let owner = c
        .owner
        .clone()
        .unwrap_or_else(|| EmailAddress::new(&user.display_name, &user.mail));
    json!({
        "id": c.id,
        "name": c.name,
        "color": c.color,
        "hexColor": "",
        "isDefaultCalendar": c.is_default,
        "changeKey": change_key(1, &c.id),
        "canShare": true,
        "canViewPrivateItems": true,
        "canEdit": true,
        "allowedOnlineMeetingProviders": [],
        "defaultOnlineMeetingProvider": "none",
        "isTallyingResponses": false,
        "isRemovable": !c.is_default,
        "owner": { "name": owner.name, "address": owner.address },
    })
}

pub(crate) fn event(user: &User, rec: &EventRecord, prefer: &Prefer) -> Value {
    let e = &rec.ev;
    let organizer = e
        .organizer
        .clone()
        .unwrap_or_else(|| EmailAddress::new(&user.display_name, &user.mail));
    let html = e.body_html.clone().unwrap_or_default();
    let text = html_to_text(&html);
    let body = if prefer.text_body {
        json!({ "contentType": "text", "content": text })
    } else {
        json!({ "contentType": "html", "content": html })
    };
    json!({
        "@odata.type": "#microsoft.graph.event",
        "@odata.etag": etag(rec.change_key, &e.id),
        "id": e.id,
        "createdDateTime": dt(&rec.created),
        "lastModifiedDateTime": dt(&rec.modified),
        "changeKey": change_key(rec.change_key, &e.id),
        "categories": [],
        "originalStartTimeZone": "UTC",
        "originalEndTimeZone": "UTC",
        "iCalUId": format!("040000008200E00074C5B7101A82E008{:016X}", fnv1a(e.id.as_bytes())),
        "reminderMinutesBeforeStart": 15,
        "isReminderOn": true,
        "hasAttachments": false,
        "subject": e.subject,
        "bodyPreview": text.chars().take(255).collect::<String>(),
        "importance": "normal",
        "sensitivity": "normal",
        "isAllDay": e.is_all_day,
        "isCancelled": e.is_cancelled,
        "isOrganizer": organizer.address.eq_ignore_ascii_case(&user.mail),
        "responseRequested": true,
        "seriesMasterId": e.series_master_id,
        "showAs": e.show_as,
        "type": e.event_type,
        "webLink": format!("https://outlook.invalid/calendar/item/{}", e.id),
        "body": body,
        "start": dt_tz(&e.start),
        "end": dt_tz(&e.end),
        "location": { "displayName": e.location, "locationType": "default" },
        "locations": if e.location.is_empty() { json!([]) } else { json!([{ "displayName": e.location }]) },
        "attendees": e.attendees.iter().map(|a| json!({
            "type": "required",
            "status": { "response": "none", "time": "0001-01-01T00:00:00Z" },
            "emailAddress": { "name": a.name, "address": a.address },
        })).collect::<Vec<_>>(),
        "organizer": addr(&organizer),
    })
}

pub(crate) fn user(u: &User) -> Value {
    json!({
        "id": u.id,
        "displayName": u.display_name,
        "userPrincipalName": u.user_principal_name,
        "mail": u.mail,
        "businessPhones": [],
        "givenName": u.display_name.split(' ').next().unwrap_or_default(),
        "surname": u.display_name.split(' ').skip(1).collect::<Vec<_>>().join(" "),
        "preferredLanguage": "en-NZ",
    })
}

/// `mailboxSettings`, with the fields WP 0.3 saw on Hotmail and Parklands (no signature).
pub(crate) fn mailbox_settings(u: &User) -> Value {
    json!({
        "archiveFolder": "fld-archive",
        "timeZone": u.time_zone,
        "dateFormat": "d/MM/yyyy",
        "timeFormat": "h:mm tt",
        "userPurpose": "user",
        "delegateMeetingMessageDeliveryOptions": "sendToDelegateOnly",
        "language": { "locale": "en-NZ", "displayName": "English (New Zealand)" },
        "automaticRepliesSetting": {
            "status": "disabled",
            "externalAudience": "none",
            "internalReplyMessage": "",
            "externalReplyMessage": "",
        },
        "workingHours": {
            "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"],
            "startTime": "08:00:00.0000000",
            "endTime": "17:00:00.0000000",
            "timeZone": { "name": u.time_zone },
        },
    })
}

/// Applies `$select`: keeps the listed properties plus `id` and OData annotations.
pub(crate) fn select(v: Value, fields: Option<&[String]>) -> Value {
    let Some(fields) = fields else { return v };
    let Value::Object(map) = v else { return v };
    let kept: Map<String, Value> = map
        .into_iter()
        .filter(|(k, _)| {
            k == "id" || k.starts_with('@') || fields.iter().any(|f| f.eq_ignore_ascii_case(k))
        })
        .collect();
    Value::Object(kept)
}

// ---------------------------------------------------------------------------
// $filter: `field op value [and field op value ...]`

#[derive(Debug, Clone)]
pub(crate) struct Clause {
    path: Vec<String>,
    op: String,
    value: Value,
}

/// A parsed `$filter`, a conjunction of comparisons. Enough for the provider's
/// `receivedDateTime ge ...` and `isRead eq false` style queries.
#[derive(Debug, Clone)]
pub(crate) struct Filter(Vec<Clause>);

impl Filter {
    pub fn parse(s: &str) -> Result<Self, String> {
        let tokens = tokenize(s)?;
        let mut clauses = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            if tokens.len() - i < 3 {
                return Err(format!("incomplete clause in $filter: {s}"));
            }
            let field = &tokens[i];
            let op = tokens[i + 1].to_ascii_lowercase();
            if !["eq", "ne", "gt", "ge", "lt", "le"].contains(&op.as_str()) {
                return Err(format!("unsupported operator '{op}' in $filter"));
            }
            let raw = &tokens[i + 2];
            let value = if let Some(q) = raw.strip_prefix('\'') {
                Value::String(q.trim_end_matches('\'').replace("''", "'"))
            } else if raw == "true" || raw == "false" {
                Value::Bool(raw == "true")
            } else if let Ok(n) = raw.parse::<f64>() {
                json!(n)
            } else {
                Value::String(raw.clone())
            };
            clauses.push(Clause {
                path: field.split('/').map(str::to_string).collect(),
                op,
                value,
            });
            i += 3;
            if i < tokens.len() {
                if !tokens[i].eq_ignore_ascii_case("and") {
                    return Err(format!(
                        "only 'and' is supported in $filter, got '{}'",
                        tokens[i]
                    ));
                }
                i += 1;
            }
        }
        Ok(Self(clauses))
    }

    pub fn matches(&self, item: &Value) -> bool {
        self.0.iter().all(|c| {
            let mut v = item;
            for p in &c.path {
                match v.get(p) {
                    Some(x) => v = x,
                    None => return false,
                }
            }
            compare(v, &c.value).is_some_and(|ord| match c.op.as_str() {
                "eq" => ord.is_eq(),
                "ne" => ord.is_ne(),
                "gt" => ord.is_gt(),
                "ge" => ord.is_ge(),
                "lt" => ord.is_lt(),
                "le" => ord.is_le(),
                _ => false,
            })
        })
    }
}

fn compare(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::String(x), Value::String(y)) => match (parse_dt(x), parse_dt(y)) {
            (Some(dx), Some(dy)) => Some(dx.cmp(&dy)),
            _ => Some(x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase())),
        },
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        (Value::Number(x), Value::Number(y)) => x.as_f64()?.partial_cmp(&y.as_f64()?),
        _ => None,
    }
}

fn tokenize(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '\'' {
            let mut tok = String::from("'");
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '\'' {
                    if chars.peek() == Some(&'\'') {
                        tok.push_str("''");
                        chars.next();
                    } else {
                        closed = true;
                        break;
                    }
                } else {
                    tok.push(c);
                }
            }
            if !closed {
                return Err("unterminated string in $filter".into());
            }
            tok.push('\'');
            out.push(tok);
        } else {
            let mut tok = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                tok.push(c);
                chars.next();
            }
            out.push(tok);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// $search (KQL subset)

/// A `$search` query: terms that must all match. A term is `word`, `"a phrase"`, or
/// `field:value` with field one of `from`, `to`, `cc`, `subject`, `body`, `participants`.
#[derive(Debug, Clone)]
pub(crate) struct Search(Vec<(Option<String>, String)>);

impl Search {
    pub fn parse(raw: &str) -> Self {
        let s = raw.trim();
        let s = s
            .strip_prefix('"')
            .and_then(|x| x.strip_suffix('"'))
            .unwrap_or(s);
        let mut terms = Vec::new();
        let mut chars = s.chars().peekable();
        while chars.peek().is_some() {
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            let mut tok = String::new();
            let mut quoted = false;
            while let Some(&c) = chars.peek() {
                if c == '"' || c == '\'' {
                    quoted = !quoted;
                    chars.next();
                    continue;
                }
                if c.is_whitespace() && !quoted {
                    break;
                }
                tok.push(c);
                chars.next();
            }
            if tok.is_empty() || tok.eq_ignore_ascii_case("and") {
                continue;
            }
            let (field, value) = match tok.split_once(':') {
                Some((f, v))
                    if ["from", "to", "cc", "subject", "body", "participants"]
                        .contains(&f.to_ascii_lowercase().as_str()) =>
                {
                    (Some(f.to_ascii_lowercase()), v.to_string())
                }
                _ => (None, tok),
            };
            terms.push((field, value.to_lowercase()));
        }
        Self(terms)
    }

    pub fn matches(&self, rec: &MessageRecord) -> bool {
        let m = &rec.msg;
        let addrs = |list: &[EmailAddress]| {
            list.iter()
                .map(|a| format!("{} {}", a.name, a.address))
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let from = addrs(std::slice::from_ref(&m.from));
        let to = addrs(&m.to);
        let cc = addrs(&m.cc);
        let subject = m.subject.to_lowercase();
        let mut body: Option<String> = None;
        let mut body_text = || {
            body.get_or_insert_with(|| html_to_text(&message_body(rec)).to_lowercase())
                .clone()
        };
        self.0.iter().all(|(field, v)| match field.as_deref() {
            Some("from") => from.contains(v),
            Some("to") => to.contains(v),
            Some("cc") => cc.contains(v),
            Some("subject") => subject.contains(v),
            Some("body") => body_text().contains(v),
            Some("participants") => from.contains(v) || to.contains(v) || cc.contains(v),
            _ => {
                subject.contains(v)
                    || from.contains(v)
                    || to.contains(v)
                    || cc.contains(v)
                    || body_text().contains(v)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_parses_and_matches() {
        let f =
            Filter::parse("isRead eq false and receivedDateTime ge 2026-01-01T00:00:00Z").unwrap();
        assert!(f.matches(&json!({"isRead": false, "receivedDateTime": "2026-02-01T00:00:00Z"})));
        assert!(!f.matches(&json!({"isRead": true, "receivedDateTime": "2026-02-01T00:00:00Z"})));
        assert!(!f.matches(&json!({"isRead": false, "receivedDateTime": "2025-12-31T00:00:00Z"})));
        let f = Filter::parse("flag/flagStatus eq 'flagged'").unwrap();
        assert!(f.matches(&json!({"flag": {"flagStatus": "flagged"}})));
        assert!(Filter::parse("a eq 1 or b eq 2").is_err());
        assert!(Filter::parse("a eq").is_err());
    }

    #[test]
    fn prefer_parses() {
        let p = Prefer::parse(&[
            "odata.maxpagesize=50, IdType=\"ImmutableId\"",
            "outlook.body-content-type=\"text\"",
        ]);
        assert_eq!(p.max_page, Some(50));
        assert!(p.immutable_ids && p.text_body);
    }

    #[test]
    fn html_to_text_strips_tags() {
        assert_eq!(html_to_text("<p>Hi &amp; <b>bye</b></p>"), "Hi & bye");
    }
}
