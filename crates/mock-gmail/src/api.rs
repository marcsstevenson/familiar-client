// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Gmail API routes under `/gmail/v1/users/{userId}` (and `/upload/gmail/v1/...`).

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value, json};

use crate::mime;
use crate::model::{Change, Label, LabelColor, MailboxState, Message, MutationError, Stored};

/// Largest message Gmail takes, through the upload endpoint.
const MAX_MESSAGE: usize = 35 << 20;

/// A Google API error.
#[derive(Clone, Debug)]
pub(crate) struct ApiError {
    pub status: u16,
    pub reason: String,
    pub message: String,
    pub headers: Vec<(String, String)>,
}

impl ApiError {
    pub fn new(status: u16, reason: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            reason: reason.into(),
            message: message.into(),
            headers: Vec::new(),
        }
    }

    fn bad(message: impl Into<String>) -> Self {
        Self::new(400, "invalidArgument", message)
    }

    fn not_found() -> Self {
        Self::new(404, "notFound", "Requested entity was not found.")
    }

    pub fn body(&self) -> Value {
        let status = match self.status {
            400 => "INVALID_ARGUMENT",
            401 => "UNAUTHENTICATED",
            403 => "PERMISSION_DENIED",
            404 => "NOT_FOUND",
            409 => "ALREADY_EXISTS",
            413 => "INVALID_ARGUMENT",
            429 => "RESOURCE_EXHAUSTED",
            503 => "UNAVAILABLE",
            _ => "INTERNAL",
        };
        let domain = if self.reason.contains("RateLimit") || self.reason.contains("rateLimit") {
            "usageLimits"
        } else {
            "global"
        };
        let mut err = json!({"message": self.message, "domain": domain, "reason": self.reason});
        if self.status == 401 {
            err["location"] = json!("Authorization");
            err["locationType"] = json!("header");
        }
        json!({"error": {"code": self.status, "message": self.message, "errors": [err], "status": status}})
    }
}

impl From<MutationError> for ApiError {
    fn from(e: MutationError) -> Self {
        match e {
            MutationError::NotFound(_) => Self::not_found(),
            MutationError::Conflict(_) => {
                Self::new(409, "duplicate", "Label name exists or conflicts")
            }
            MutationError::InvalidLabel(_) | MutationError::Invalid(_) => Self::bad(e.to_string()),
        }
    }
}

pub(crate) enum Reply {
    Json(Value),
    Empty,
}

type R = Result<Reply, ApiError>;

/// One API request with the `/gmail/v1/users/{userId}` prefix removed from `path`.
pub(crate) struct Req<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub params: &'a [(String, String)],
    pub content_type: &'a str,
    pub body: &'a [u8],
    pub upload: bool,
}

impl Req<'_> {
    fn get(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn all(&self, name: &str) -> Vec<&str> {
        self.params
            .iter()
            .filter(|(k, _)| k == name)
            .flat_map(|(_, v)| v.split(','))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .collect()
    }

    fn flag(&self, name: &str) -> bool {
        self.get(name)
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }

    fn max_results(&self, default: usize, max: usize) -> Result<usize, ApiError> {
        match self.get("maxResults") {
            None => Ok(default),
            Some(v) => v
                .parse::<usize>()
                .map(|n| n.clamp(1, max))
                .map_err(|_| ApiError::bad(format!("Invalid value for maxResults: {v}"))),
        }
    }

    fn offset(&self) -> Result<usize, ApiError> {
        match self.get("pageToken") {
            None | Some("") => Ok(0),
            Some(t) => t
                .strip_prefix("mock-page-")
                .and_then(|n| n.parse().ok())
                .ok_or_else(|| ApiError::bad("Invalid pageToken")),
        }
    }

    fn json(&self) -> Result<Value, ApiError> {
        if self.body.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_slice(self.body)
            .map_err(|e| ApiError::bad(format!("Invalid JSON payload received. {e}")))
    }
}

fn segments(path: &str) -> Vec<&str> {
    path.trim_matches('/').split('/').collect()
}

/// Gmail's quota units for a request, from Google's per-method table.
pub(crate) fn cost(method: &str, path: &str) -> u32 {
    let s = segments(path);
    match (method, s.as_slice()) {
        (_, ["profile"]) => 1,
        ("GET", ["labels", ..]) => 1,
        (_, ["labels", ..]) => 5,
        ("GET", ["messages"]) => 5,
        (_, ["messages", "send"]) => 100,
        (_, ["messages", "batchModify" | "batchDelete"]) => 50,
        ("DELETE", ["messages", _]) => 10,
        (_, ["messages", ..]) => 5,
        (_, ["threads", ..]) => 10,
        ("GET", ["drafts", ..]) => 5,
        ("POST", ["drafts", "send"]) => 100,
        ("POST", ["drafts"]) => 10,
        ("PUT", ["drafts", _]) => 15,
        ("DELETE", ["drafts", _]) => 10,
        (_, ["history"]) => 2,
        (_, ["settings", ..]) => 1,
        _ => 1,
    }
}

pub(crate) fn route(st: &mut MailboxState, r: &Req<'_>) -> R {
    let s = segments(r.path);
    match (r.method, s.as_slice()) {
        ("GET", ["profile"]) => Ok(Reply::Json(json!({
            "emailAddress": st.email,
            "messagesTotal": st.messages.len(),
            "threadsTotal": thread_count(st.messages.values()),
            "historyId": st.history_id.to_string(),
        }))),
        ("GET", ["labels"]) => Ok(Reply::Json(json!({
            "labels": st.labels.iter().map(label_json).collect::<Vec<_>>()
        }))),
        ("POST", ["labels"]) => {
            let v = r.json()?;
            let mut l = Label::user(v["name"].as_str().unwrap_or_default());
            apply_label_fields(&mut l, &v);
            let id = st.add_label(l)?;
            Ok(Reply::Json(label_json(st.label(&id).expect("just added"))))
        }
        ("GET", ["labels", id]) => {
            let l = st.label(id).ok_or_else(ApiError::not_found)?;
            let mut v = label_json(l);
            let with: Vec<&Stored> = st.messages.values().filter(|m| m.has(id)).collect();
            let unread: Vec<&Stored> = with.iter().copied().filter(|m| m.has("UNREAD")).collect();
            v["messagesTotal"] = json!(with.len());
            v["messagesUnread"] = json!(unread.len());
            v["threadsTotal"] = json!(thread_count(with.iter().copied()));
            v["threadsUnread"] = json!(thread_count(unread.iter().copied()));
            Ok(Reply::Json(v))
        }
        ("PATCH" | "PUT", ["labels", id]) => {
            let v = r.json()?;
            st.update_label(id, |l| apply_label_fields(l, &v))?;
            Ok(Reply::Json(label_json(st.label(id).expect("exists"))))
        }
        ("DELETE", ["labels", id]) => {
            st.delete_label(id)?;
            Ok(Reply::Empty)
        }
        ("GET", ["messages"]) => list_messages(st, r),
        ("POST", ["messages", "send"]) => {
            let (meta, raw) = message_input(r, false)?;
            let raw = raw.ok_or_else(missing_raw)?;
            let id = send(st, raw, meta["threadId"].as_str())?;
            Ok(Reply::Json(minimal_ref(&st.messages[&id])))
        }
        ("POST", ["messages", "batchModify"]) => {
            let v = r.json()?;
            let ids = strings(&v["ids"]);
            if ids.len() > 1000 {
                return Err(ApiError::bad("Too many ids: at most 1000"));
            }
            let (add, remove) = (strings(&v["addLabelIds"]), strings(&v["removeLabelIds"]));
            for id in &ids {
                // Gmail skips IDs it doesn't know in batch calls.
                if st.messages.contains_key(id) {
                    st.modify(id, &add, &remove)?;
                }
            }
            Ok(Reply::Empty)
        }
        ("POST", ["messages", "batchDelete"]) => {
            let v = r.json()?;
            for id in strings(&v["ids"]) {
                if st.messages.contains_key(&id) {
                    st.delete(&id)?;
                }
            }
            Ok(Reply::Empty)
        }
        ("GET", ["messages", id]) => {
            let m = st.messages.get(*id).ok_or_else(ApiError::not_found)?;
            Ok(Reply::Json(message_json(m, r)?))
        }
        ("DELETE", ["messages", id]) => {
            st.delete(id)?;
            Ok(Reply::Empty)
        }
        ("POST", ["messages", id, "modify"]) => {
            let v = r.json()?;
            st.modify(
                id,
                &strings(&v["addLabelIds"]),
                &strings(&v["removeLabelIds"]),
            )?;
            Ok(Reply::Json(minimal_ref(&st.messages[*id])))
        }
        ("POST", ["messages", id, "trash"]) => {
            st.modify(id, &["TRASH".into()], &[])?;
            Ok(Reply::Json(minimal_ref(&st.messages[*id])))
        }
        ("POST", ["messages", id, "untrash"]) => {
            st.modify(id, &[], &["TRASH".into()])?;
            Ok(Reply::Json(minimal_ref(&st.messages[*id])))
        }
        ("GET", ["messages", id, "attachments", att]) => {
            let m = st.messages.get(*id).ok_or_else(ApiError::not_found)?;
            let part = att
                .strip_prefix("mock-att-")
                .ok_or_else(ApiError::not_found)?;
            let bytes = find_part(&m.raw, "", part).ok_or_else(ApiError::not_found)?;
            Ok(Reply::Json(json!({
                "attachmentId": att,
                "size": bytes.len(),
                "data": mime::b64url(&bytes),
            })))
        }
        ("GET", ["threads", id]) => {
            let mut msgs: Vec<&Stored> = st
                .messages
                .values()
                .filter(|m| m.thread_id == *id)
                .collect();
            if msgs.is_empty() {
                return Err(ApiError::not_found());
            }
            msgs.sort_by(|a, b| (a.date_ms, &a.id).cmp(&(b.date_ms, &b.id)));
            let history = msgs.iter().map(|m| m.history_id).max().unwrap_or_default();
            let messages = msgs
                .iter()
                .map(|m| message_json(m, r))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Reply::Json(json!({
                "id": id,
                "historyId": history.to_string(),
                "messages": messages,
            })))
        }
        ("GET", ["drafts"]) => list_drafts(st, r),
        ("POST", ["drafts"]) => {
            let (meta, raw) = message_input(r, true)?;
            let raw = raw.ok_or_else(missing_raw)?;
            let msg = store_draft(st, raw, meta["message"]["threadId"].as_str())?;
            let id = st.add_draft(&msg);
            Ok(Reply::Json(draft_ref(st, &id)))
        }
        ("POST", ["drafts", "send"]) => {
            let (meta, raw) = message_input(r, true)?;
            let draft = meta["id"].as_str().unwrap_or_default().to_string();
            let old = st
                .drafts
                .get(&draft)
                .cloned()
                .ok_or_else(ApiError::not_found)?;
            let old_msg = &st.messages[&old];
            let thread = meta["message"]["threadId"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| old_msg.thread_id.clone());
            let raw = raw.unwrap_or_else(|| old_msg.raw.clone());
            check_recipients(&raw)?;
            st.delete(&old)?;
            let id = finish_send(st, raw, Some(&thread))?;
            Ok(Reply::Json(minimal_ref(&st.messages[&id])))
        }
        ("GET", ["drafts", id]) => {
            let msg = st.drafts.get(*id).ok_or_else(ApiError::not_found)?;
            let m = &st.messages[msg];
            Ok(Reply::Json(
                json!({"id": id, "message": message_json(m, r)?}),
            ))
        }
        ("PUT", ["drafts", id]) => {
            let old = st
                .drafts
                .get(*id)
                .cloned()
                .ok_or_else(ApiError::not_found)?;
            let (meta, raw) = message_input(r, true)?;
            let raw = raw.ok_or_else(missing_raw)?;
            let thread = meta["message"]["threadId"]
                .as_str()
                .map(str::to_string)
                .or_else(|| Some(st.messages[&old].thread_id.clone()));
            // Gmail gives the draft a new message ID on every update.
            st.delete(&old)?;
            let msg = store_draft(st, raw, thread.as_deref())?;
            st.drafts.insert(id.to_string(), msg);
            Ok(Reply::Json(draft_ref(st, id)))
        }
        ("DELETE", ["drafts", id]) => {
            let msg = st
                .drafts
                .get(*id)
                .cloned()
                .ok_or_else(ApiError::not_found)?;
            st.delete(&msg)?;
            Ok(Reply::Empty)
        }
        ("GET", ["history"]) => history(st, r),
        ("GET", ["settings", "sendAs"]) => Ok(Reply::Json(json!({
            "sendAs": st.send_as.iter().map(send_as_json).collect::<Vec<_>>()
        }))),
        ("GET", ["settings", "sendAs", email]) => st
            .send_as
            .iter()
            .find(|s| s.email.eq_ignore_ascii_case(email))
            .map(|s| Reply::Json(send_as_json(s)))
            .ok_or_else(ApiError::not_found),
        _ => Err(ApiError::new(
            404,
            "notFound",
            format!("The mock does not serve {} {}", r.method, r.path),
        )),
    }
}

fn missing_raw() -> ApiError {
    ApiError::bad(
        "'raw' RFC822 payload message string or uploading message via /upload/* URL required",
    )
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn thread_count<'a>(msgs: impl Iterator<Item = &'a Stored>) -> usize {
    let mut t: Vec<&str> = msgs.map(|m| m.thread_id.as_str()).collect();
    t.sort_unstable();
    t.dedup();
    t.len()
}

fn apply_label_fields(l: &mut Label, v: &Value) {
    if let Some(n) = v["name"].as_str() {
        l.name = n.to_string();
    }
    if let Some(x) = v["messageListVisibility"].as_str() {
        l.message_list_visibility = Some(x.into());
    }
    if let Some(x) = v["labelListVisibility"].as_str() {
        l.label_list_visibility = Some(x.into());
    }
    if let (Some(t), Some(b)) = (
        v["color"]["textColor"].as_str(),
        v["color"]["backgroundColor"].as_str(),
    ) {
        l.color = Some(LabelColor {
            text_color: t.into(),
            background_color: b.into(),
        });
    }
}

fn label_json(l: &Label) -> Value {
    let mut v = json!({
        "id": l.id,
        "name": l.name,
        "type": if l.system { "system" } else { "user" },
    });
    if let Some(x) = &l.message_list_visibility {
        v["messageListVisibility"] = json!(x);
    }
    if let Some(x) = &l.label_list_visibility {
        v["labelListVisibility"] = json!(x);
    }
    if let Some(c) = &l.color {
        v["color"] = json!({"textColor": c.text_color, "backgroundColor": c.background_color});
    }
    v
}

fn send_as_json(s: &crate::model::SendAs) -> Value {
    let mut v = json!({
        "sendAsEmail": s.email,
        "displayName": s.display_name,
        "replyToAddress": s.reply_to,
        "signature": s.signature,
        "isPrimary": s.is_primary,
        "isDefault": s.is_default,
        "treatAsAlias": s.treat_as_alias,
    });
    if let Some(vs) = &s.verification_status {
        v["verificationStatus"] = json!(vs);
    }
    v
}

fn minimal_ref(m: &Stored) -> Value {
    json!({"id": m.id, "threadId": m.thread_id, "labelIds": m.labels})
}

fn draft_ref(st: &MailboxState, id: &str) -> Value {
    let m = &st.messages[&st.drafts[id]];
    json!({"id": id, "message": minimal_ref(m)})
}

fn message_json(m: &Stored, r: &Req<'_>) -> Result<Value, ApiError> {
    let mut v = json!({
        "id": m.id,
        "threadId": m.thread_id,
        "labelIds": m.labels,
        "snippet": mime::snippet(&m.text),
        "sizeEstimate": m.raw.len(),
        "historyId": m.history_id.to_string(),
        "internalDate": m.date_ms.to_string(),
    });
    match r.get("format").unwrap_or("full") {
        "minimal" => {}
        "raw" => v["raw"] = json!(mime::b64url(&m.raw)),
        "metadata" => {
            let wanted = r.all("metadataHeaders");
            let headers: Vec<Value> = m
                .headers
                .iter()
                .filter(|(k, _)| {
                    wanted.is_empty() || wanted.iter().any(|w| w.eq_ignore_ascii_case(k))
                })
                .map(|(k, val)| json!({"name": k, "value": val}))
                .collect();
            v["payload"] = json!({
                "partId": "",
                "mimeType": mime::mime_type(&m.headers),
                "filename": "",
                "headers": headers,
                "body": {"size": 0},
            });
        }
        "full" => v["payload"] = payload(&m.raw, ""),
        other => return Err(ApiError::bad(format!("Invalid value for format: {other}"))),
    }
    Ok(v)
}

/// Gmail's `format=full` payload tree. Attachment bodies carry an `attachmentId` instead
/// of their data.
fn payload(part: &[u8], part_id: &str) -> Value {
    let (h, body) = mime::split(part);
    let t = mime::mime_type(&h);
    let headers: Vec<Value> = h
        .iter()
        .map(|(k, v)| json!({"name": k, "value": v}))
        .collect();
    let mut v = json!({
        "partId": part_id,
        "mimeType": t,
        "filename": mime::filename(&h).unwrap_or_default(),
        "headers": headers,
    });
    if let Some(b) = multipart_boundary(&h) {
        let parts: Vec<Value> = mime::multipart(body, &b)
            .iter()
            .enumerate()
            .map(|(i, p)| payload(p, &child_id(part_id, i)))
            .collect();
        v["body"] = json!({"size": 0});
        v["parts"] = json!(parts);
    } else {
        let data = mime::decode(mime::header(&h, "content-transfer-encoding"), body);
        if mime::filename(&h).is_some() {
            v["body"] = json!({"attachmentId": format!("mock-att-{part_id}"), "size": data.len()});
        } else if data.is_empty() {
            v["body"] = json!({"size": 0});
        } else {
            v["body"] = json!({"size": data.len(), "data": mime::b64url(&data)});
        }
    }
    v
}

fn multipart_boundary(h: &[(String, String)]) -> Option<String> {
    if !mime::mime_type(h).starts_with("multipart/") {
        return None;
    }
    mime::header(h, "content-type").and_then(|v| mime::param(v, "boundary"))
}

fn child_id(parent: &str, i: usize) -> String {
    if parent.is_empty() {
        i.to_string()
    } else {
        format!("{parent}.{i}")
    }
}

fn find_part(part: &[u8], id: &str, want: &str) -> Option<Vec<u8>> {
    let (h, body) = mime::split(part);
    if id == want {
        return Some(mime::decode(
            mime::header(&h, "content-transfer-encoding"),
            body,
        ));
    }
    let b = multipart_boundary(&h)?;
    mime::multipart(body, &b)
        .iter()
        .enumerate()
        .find_map(|(i, p)| find_part(p, &child_id(id, i), want))
}

// ---------------------------------------------------------------------------
// Listing and search

fn list_messages(st: &MailboxState, r: &Req<'_>) -> R {
    let max = r.max_results(100, 500)?;
    let offset = r.offset()?;
    let label_ids = r.all("labelIds");
    for l in &label_ids {
        if st.label(l).is_none() {
            return Err(ApiError::bad(format!("Invalid label: {l}")));
        }
    }
    let query = Query::parse(r.get("q").unwrap_or(""), st);
    let spam_trash = r.flag("includeSpamTrash")
        || label_ids.iter().any(|l| *l == "SPAM" || *l == "TRASH")
        || query.wants_spam_trash();
    let matches: Vec<&Stored> = st
        .ordered()
        .into_iter()
        .filter(|m| spam_trash || !(m.has("SPAM") || m.has("TRASH")))
        .filter(|m| label_ids.iter().all(|l| m.has(l)))
        .filter(|m| query.matches(m))
        .collect();
    let mut v = json!({"resultSizeEstimate": matches.len()});
    let page: Vec<Value> = matches
        .iter()
        .skip(offset)
        .take(max)
        .map(|m| json!({"id": m.id, "threadId": m.thread_id}))
        .collect();
    if !page.is_empty() {
        v["messages"] = json!(page);
    }
    if offset + max < matches.len() {
        v["nextPageToken"] = json!(format!("mock-page-{}", offset + max));
    }
    Ok(Reply::Json(v))
}

fn list_drafts(st: &MailboxState, r: &Req<'_>) -> R {
    let max = r.max_results(100, 500)?;
    let offset = r.offset()?;
    let query = Query::parse(r.get("q").unwrap_or(""), st);
    let mut drafts: Vec<(&String, &Stored)> = st
        .drafts
        .iter()
        .filter_map(|(d, m)| st.messages.get(m).map(|m| (d, m)))
        .filter(|(_, m)| query.matches(m))
        .collect();
    drafts.sort_by(|a, b| (b.1.date_ms, b.0).cmp(&(a.1.date_ms, a.0)));
    let mut v = json!({"resultSizeEstimate": drafts.len()});
    let page: Vec<Value> = drafts
        .iter()
        .skip(offset)
        .take(max)
        .map(|(d, m)| json!({"id": d, "message": {"id": m.id, "threadId": m.thread_id}}))
        .collect();
    if !page.is_empty() {
        v["drafts"] = json!(page);
    }
    if offset + max < drafts.len() {
        v["nextPageToken"] = json!(format!("mock-page-{}", offset + max));
    }
    Ok(Reply::Json(v))
}

enum Term {
    Header(&'static [&'static str], String),
    Label(Option<String>),
    In(String),
    Is(String),
    HasAttachment,
    After(i64),
    Before(i64),
    Text(String),
}

/// The subset of Gmail's search syntax the provider translates to.
struct Query {
    terms: Vec<(bool, Term)>,
}

impl Query {
    fn parse(q: &str, st: &MailboxState) -> Self {
        let mut terms = Vec::new();
        for raw in tokens(q) {
            let (neg, tok) = match raw.strip_prefix('-') {
                Some(t) if !t.is_empty() => (true, t.to_string()),
                _ => (false, raw.clone()),
            };
            let term = match tok.split_once(':') {
                Some((op, val)) => {
                    let val = val.trim_matches('"').to_lowercase();
                    match op.to_ascii_lowercase().as_str() {
                        "from" => Term::Header(&["from"], val),
                        "to" => Term::Header(&["to", "cc", "bcc"], val),
                        "cc" => Term::Header(&["cc"], val),
                        "bcc" => Term::Header(&["bcc"], val),
                        "subject" => Term::Header(&["subject"], val),
                        "rfc822msgid" => Term::Header(&["message-id"], val),
                        "label" => Term::Label(resolve_label(st, &val)),
                        "in" => Term::In(val),
                        "is" => Term::Is(val),
                        "has" if val == "attachment" => Term::HasAttachment,
                        "after" | "newer" => Term::After(parse_date(&val).unwrap_or(i64::MIN)),
                        "before" | "older" => Term::Before(parse_date(&val).unwrap_or(i64::MAX)),
                        _ => Term::Text(tok.trim_matches('"').to_lowercase()),
                    }
                }
                None => Term::Text(tok.trim_matches('"').to_lowercase()),
            };
            terms.push((neg, term));
        }
        Self { terms }
    }

    fn wants_spam_trash(&self) -> bool {
        self.terms.iter().any(|(neg, t)| {
            !neg && match t {
                Term::In(v) => matches!(v.as_str(), "spam" | "trash" | "anywhere"),
                Term::Label(Some(l)) => l == "SPAM" || l == "TRASH",
                _ => false,
            }
        })
    }

    fn matches(&self, m: &Stored) -> bool {
        self.terms.iter().all(|(neg, t)| term_matches(t, m) != *neg)
    }
}

fn term_matches(t: &Term, m: &Stored) -> bool {
    match t {
        Term::Header(names, val) => names
            .iter()
            .any(|n| mime::header(&m.headers, n).is_some_and(|h| h.to_lowercase().contains(val))),
        Term::Label(l) => l.as_deref().is_some_and(|l| m.has(l)),
        Term::In(v) => match v.as_str() {
            "anywhere" => true,
            "inbox" => m.has("INBOX"),
            "sent" => m.has("SENT"),
            "drafts" | "draft" => m.has("DRAFT"),
            "trash" => m.has("TRASH"),
            "spam" => m.has("SPAM"),
            "starred" => m.has("STARRED"),
            "important" => m.has("IMPORTANT"),
            _ => false,
        },
        Term::Is(v) => match v.as_str() {
            "unread" => m.has("UNREAD"),
            "read" => !m.has("UNREAD"),
            "starred" => m.has("STARRED"),
            "important" => m.has("IMPORTANT"),
            _ => false,
        },
        Term::HasAttachment => m.has_attachment,
        Term::After(t) => m.date_ms >= *t,
        Term::Before(t) => m.date_ms < *t,
        Term::Text(w) => {
            m.text.to_lowercase().contains(w)
                || ["subject", "from", "to"].iter().any(|n| {
                    mime::header(&m.headers, n).is_some_and(|h| h.to_lowercase().contains(w))
                })
        }
    }
}

/// A label by ID or by name, where Gmail also accepts `-` for spaces and `/`.
fn resolve_label(st: &MailboxState, val: &str) -> Option<String> {
    let norm = |s: &str| s.to_lowercase().replace([' ', '/'], "-");
    st.labels
        .iter()
        .find(|l| l.id.eq_ignore_ascii_case(val) || norm(&l.name) == norm(val))
        .map(|l| l.id.clone())
}

/// `2026/11/01`, `2026-11-01` or seconds since the epoch, as milliseconds.
fn parse_date(v: &str) -> Option<i64> {
    if let Ok(secs) = v.parse::<i64>() {
        return Some(secs * 1000);
    }
    let d = NaiveDate::parse_from_str(&v.replace('-', "/"), "%Y/%m/%d").ok()?;
    Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis())
}

fn tokens(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in q.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ---------------------------------------------------------------------------
// History

fn history(st: &MailboxState, r: &Req<'_>) -> R {
    let start: u64 = match r.get("startHistoryId") {
        None => return Err(ApiError::bad("Missing required parameter: startHistoryId")),
        Some(v) => v
            .parse()
            .map_err(|_| ApiError::bad(format!("Invalid startHistoryId: {v}")))?,
    };
    if start < st.history_floor {
        return Err(ApiError::not_found());
    }
    let max = r.max_results(100, 500)?;
    let offset = r.offset()?;
    let label = r.get("labelId");
    let types = r.all("historyTypes");
    let wanted =
        |kind: &str| types.is_empty() || types.iter().any(|t| t.eq_ignore_ascii_case(kind));
    let records: Vec<Value> = st
        .history
        .iter()
        .filter(|h| h.id > start)
        .filter(|h| {
            label.is_none_or(|l| {
                h.labels.iter().any(|x| x == l)
                    || matches!(&h.change, Change::LabelsAdded(c) | Change::LabelsRemoved(c) if c.iter().any(|x| x == l))
            })
        })
        .filter_map(|h| {
            let msg = json!({"id": h.msg_id, "threadId": h.thread_id});
            let full = json!({"id": h.msg_id, "threadId": h.thread_id, "labelIds": h.labels});
            let mut v = Map::new();
            v.insert("id".into(), json!(h.id.to_string()));
            v.insert("messages".into(), json!([msg]));
            match &h.change {
                Change::Added if wanted("messageAdded") => {
                    v.insert("messagesAdded".into(), json!([{"message": full}]));
                }
                Change::Deleted if wanted("messageDeleted") => {
                    v.insert("messagesDeleted".into(), json!([{"message": msg}]));
                }
                Change::LabelsAdded(l) if wanted("labelAdded") => {
                    v.insert("labelsAdded".into(), json!([{"message": full, "labelIds": l}]));
                }
                Change::LabelsRemoved(l) if wanted("labelRemoved") => {
                    v.insert("labelsRemoved".into(), json!([{"message": full, "labelIds": l}]));
                }
                _ => return None,
            }
            Some(Value::Object(v))
        })
        .collect();
    let mut v = json!({"historyId": st.history_id.to_string()});
    let page: Vec<Value> = records.iter().skip(offset).take(max).cloned().collect();
    if !page.is_empty() {
        v["history"] = json!(page);
    }
    if offset + max < records.len() {
        v["nextPageToken"] = json!(format!("mock-page-{}", offset + max));
    }
    Ok(Reply::Json(v))
}

// ---------------------------------------------------------------------------
// Sending and drafts

/// The request's metadata and RFC 5322 message, from JSON `raw` (inside `message` for
/// drafts) or the `/upload` endpoint (`uploadType=media` or `multipart`).
fn message_input(r: &Req<'_>, nested: bool) -> Result<(Value, Option<Vec<u8>>), ApiError> {
    let (meta, raw) = if r.upload {
        match r.get("uploadType") {
            Some("media") => (json!({}), Some(r.body.to_vec())),
            Some("multipart") => {
                let b = mime::param(r.content_type, "boundary")
                    .ok_or_else(|| ApiError::bad("Missing multipart boundary"))?;
                let parts = mime::multipart(r.body, &b);
                if parts.len() != 2 {
                    return Err(ApiError::bad(
                        "Multipart upload needs metadata and media parts",
                    ));
                }
                let (_, meta) = mime::split(parts[0]);
                let (_, media) = mime::split(parts[1]);
                let meta: Value = if meta.iter().all(u8::is_ascii_whitespace) {
                    json!({})
                } else {
                    serde_json::from_slice(meta)
                        .map_err(|e| ApiError::bad(format!("Invalid metadata part: {e}")))?
                };
                (meta, Some(media.to_vec()))
            }
            other => {
                return Err(ApiError::bad(format!(
                    "uploadType {} is not served by the mock (use media or multipart)",
                    other.unwrap_or("(none)")
                )));
            }
        }
    } else {
        let meta = r.json()?;
        let raw_field = if nested {
            &meta["message"]["raw"]
        } else {
            &meta["raw"]
        };
        let raw = match raw_field.as_str() {
            None => None,
            Some(s) => Some(
                mime::from_b64url(s)
                    .ok_or_else(|| ApiError::bad("Invalid value for ByteString"))?,
            ),
        };
        (meta, raw)
    };
    if let Some(raw) = &raw
        && raw.len() > MAX_MESSAGE
    {
        return Err(ApiError::new(413, "uploadTooLarge", "Request too large."));
    }
    Ok((meta, raw))
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

fn store_draft(
    st: &mut MailboxState,
    raw: Vec<u8>,
    thread: Option<&str>,
) -> Result<String, ApiError> {
    let mut m = Message::from_raw(raw).labels(&["DRAFT"]).at(now());
    if let Some(t) = thread {
        m = m.thread(t);
    }
    Ok(st.insert(m)?)
}

/// Checks a message can be sent (it has recipients) before anything changes.
fn check_recipients(raw: &[u8]) -> Result<(), ApiError> {
    let (h, _) = mime::split(raw);
    let recipients: Vec<String> = ["to", "cc", "bcc"]
        .iter()
        .filter_map(|n| mime::header(&h, n))
        .flat_map(mime::addresses)
        .collect();
    if recipients.is_empty() {
        return Err(ApiError::bad("Recipient address required"));
    }
    Ok(())
}

fn send(st: &mut MailboxState, raw: Vec<u8>, thread: Option<&str>) -> Result<String, ApiError> {
    check_recipients(&raw)?;
    finish_send(st, raw, thread)
}

/// Sends as Gmail does: a `From` that isn't the account or a verified alias is rewritten
/// to the account's address, `Message-ID` and `Date` are added if missing, and mail to
/// one of the account's own addresses also lands in its Inbox.
fn finish_send(
    st: &mut MailboxState,
    mut raw: Vec<u8>,
    thread: Option<&str>,
) -> Result<String, ApiError> {
    let (h, _) = mime::split(&raw);
    let from = mime::header(&h, "from")
        .map(mime::addresses)
        .and_then(|a| a.into_iter().next());
    let verified = st.own_addresses(true);
    if !from.as_ref().is_some_and(|f| verified.contains(f)) {
        let primary = format!("{} <{}>", st.name, st.email);
        raw = mime::set_header(&raw, "From", &primary);
    }
    if mime::header(&h, "message-id").is_none() {
        let mid = format!("<mock-{}-{}@mail.gmail.com>", st.history_id, st.sent.len());
        raw = mime::set_header(&raw, "Message-ID", &mid);
    }
    if mime::header(&h, "date").is_none() {
        raw = mime::set_header(&raw, "Date", &now().to_rfc2822());
    }
    let own = st.own_addresses(false);
    let to_self = ["to", "cc", "bcc"]
        .iter()
        .filter_map(|n| mime::header(&h, n))
        .flat_map(mime::addresses)
        .any(|a| own.contains(&a));
    let labels: &[&str] = if to_self {
        &["SENT", "INBOX", "UNREAD"]
    } else {
        &["SENT"]
    };
    let mut m = Message::from_raw(raw).labels(labels).at(now());
    if let Some(t) = thread {
        m = m.thread(t);
    }
    let id = st.insert(m)?;
    st.sent.push(id.clone());
    Ok(id)
}
