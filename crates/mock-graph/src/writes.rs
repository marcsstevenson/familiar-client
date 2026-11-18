// SPDX-License-Identifier: MIT OR Apache-2.0
//! Write routes: the subset of Graph's mail and calendar writes Familiar uses.
//!
//! Each route changes the mailbox model through the same mutation methods
//! tests use, so every write shows up in delta rounds like a change made
//! anywhere else.

use super::{GraphError, R, Reply, strip_prefix_ci};
use crate::model::{Attachment, EmailAddress, Folder, MailboxState, Message};
use crate::render::{self, Prefer};
use serde_json::{Value, json};

fn created(body: Value) -> Reply {
    Reply {
        status: 201,
        body,
        headers: Vec::new(),
    }
}

fn no_content() -> Reply {
    Reply {
        status: 204,
        body: Value::Null,
        headers: Vec::new(),
    }
}

fn bad_request(message: impl Into<String>) -> GraphError {
    GraphError::new(400, "ErrorInvalidRequest", message)
}

fn str_field<'a>(body: &'a Value, name: &str) -> Result<&'a str, GraphError> {
    body.get(name)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| bad_request(format!("The property '{name}' is required.")))
}

/// Splits `/v1.0/me/...` or `/v1.0/users/<me>/...` into segments after the
/// user. Writes also take `/beta/`, for `reportMessage`.
fn segments<'a>(st: &MailboxState, path: &'a str) -> Result<Vec<&'a str>, GraphError> {
    let Some(rest) = strip_prefix_ci(path, "/v1.0/").or_else(|| strip_prefix_ci(path, "/beta/"))
    else {
        return Err(GraphError::new(
            400,
            "BadRequest",
            "Invalid version: the mock serves /v1.0 (and /beta for writes) only.",
        ));
    };
    let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        ["me", tail @ ..] => Ok(tail.to_vec()),
        [u, who, tail @ ..] if u.eq_ignore_ascii_case("users") => {
            let user = &st.user;
            let me = [&user.id, &user.user_principal_name, &user.mail];
            if !me.iter().any(|m| m.eq_ignore_ascii_case(who)) {
                return Err(GraphError::new(
                    403,
                    "ErrorAccessDenied",
                    "Access is denied. Check credentials and try again.",
                ));
            }
            Ok(tail.to_vec())
        }
        _ => Err(GraphError::new(
            400,
            "BadRequest",
            "Resource not found for the segment.",
        )),
    }
}

/// Answers one write.
pub(super) fn route(
    st: &mut MailboxState,
    _base: &str,
    method: &str,
    path: &str,
    body: &Value,
    prefer: &Prefer,
) -> R {
    let segs = segments(st, path)?;
    let lower: Vec<String> = segs.iter().map(|s| s.to_ascii_lowercase()).collect();
    let l: Vec<&str> = lower.iter().map(String::as_str).collect();
    match (method, l.as_slice()) {
        // Messages.
        ("PATCH", ["messages", _]) | ("PATCH", ["mailfolders", _, "messages", _]) => {
            update_message(st, segs[segs.len() - 1], body, prefer)
        }
        ("POST", ["messages", _, "move"]) => move_message(st, segs[1], body, prefer),
        ("POST", ["messages", _, "reportmessage"]) => report_message(st, segs[1], body),
        ("POST", ["messages"]) | ("POST", ["mailfolders", _, "messages"]) => {
            create_draft(st, body, prefer)
        }
        ("POST", ["messages", _, "attachments"]) => add_attachment(st, segs[1], body),
        ("DELETE", ["messages", _, "attachments", _]) => delete_attachment(st, segs[1], segs[3]),
        ("POST", ["messages", _, "createreply"]) => {
            create_response(st, segs[1], Response::Reply, body, prefer)
        }
        ("POST", ["messages", _, "createreplyall"]) => {
            create_response(st, segs[1], Response::ReplyAll, body, prefer)
        }
        ("POST", ["messages", _, "createforward"]) => {
            create_response(st, segs[1], Response::Forward, body, prefer)
        }
        ("POST", ["messages", _, "send"]) => send_draft(st, segs[1]),
        ("DELETE", ["messages", _]) => {
            st.delete_message(segs[1])
                .map_err(|_| GraphError::not_found())?;
            Ok(no_content())
        }

        // Folders.
        ("POST", ["mailfolders"]) => create_folder(st, None, body),
        ("POST", ["mailfolders", _, "childfolders"]) => create_folder(st, Some(segs[1]), body),
        ("PATCH", ["mailfolders", _]) => {
            let id = st
                .resolve_folder(segs[1])
                .ok_or_else(GraphError::not_found)?;
            let name = str_field(body, "displayName")?.to_string();
            let parent = st
                .folder(&id)
                .and_then(|r| r.folder.parent_folder_id.clone());
            ensure_unique_name(st, parent.as_deref(), &name, Some(&id))?;
            st.update_folder(&id, |f| f.display_name = name)
                .map_err(|_| GraphError::not_found())?;
            Ok(Reply::ok(render::folder(
                st,
                st.folder(&id).expect("exists"),
            )))
        }
        ("POST", ["mailfolders", _, "move"]) => {
            let id = st
                .resolve_folder(segs[1])
                .ok_or_else(GraphError::not_found)?;
            let dest = st
                .resolve_folder(str_field(body, "destinationId")?)
                .ok_or_else(GraphError::not_found)?;
            refuse_well_known(st, &id)?;
            move_folder(st, &id, &dest)?;
            Ok(created(render::folder(st, st.folder(&id).expect("exists"))))
        }
        ("DELETE", ["mailfolders", _]) => {
            let id = st
                .resolve_folder(segs[1])
                .ok_or_else(GraphError::not_found)?;
            refuse_well_known(st, &id)?;
            // As in Exchange: a folder deleted outside Deleted Items moves
            // there; one already inside it is deleted for good.
            let deleted = st.resolve_folder("deleteditems");
            if let Some(d) = deleted.filter(|d| !is_under(st, &id, d)) {
                move_folder(st, &id, &d)?;
            } else {
                st.delete_folder(&id).map_err(|_| GraphError::not_found())?;
            }
            Ok(no_content())
        }

        // Events.
        ("POST", ["events"]) | ("POST", ["calendar", "events"]) => {
            events::create(st, "", body, prefer)
        }
        ("POST", ["calendars", _, "events"]) => events::create(st, segs[1], body, prefer),
        ("PATCH", ["events", _])
        | ("PATCH", ["calendar", "events", _])
        | ("PATCH", ["calendars", _, "events", _]) => {
            events::update(st, segs[segs.len() - 1], body, prefer)
        }
        ("DELETE", ["events", _])
        | ("DELETE", ["calendar", "events", _])
        | ("DELETE", ["calendars", _, "events", _]) => {
            events::delete(st, segs[segs.len() - 1])?;
            Ok(no_content())
        }
        ("POST", ["events", _, "accept"]) => events::respond(st, segs[1], "accepted"),
        ("POST", ["events", _, "tentativelyaccept"]) => {
            events::respond(st, segs[1], "tentativelyAccepted")
        }
        ("POST", ["events", _, "decline"]) => events::respond(st, segs[1], "declined"),
        ("POST", ["events", _, "cancel"]) => events::cancel(st, segs[1]),

        _ => Err(GraphError::new(
            405,
            "ErrorInvalidRequest",
            format!("The mock does not serve {method} on '{}'.", segs.join("/")),
        )),
    }
}

// --- messages ---------------------------------------------------------------

fn message_reply(st: &MailboxState, id: &str, prefer: &Prefer) -> Value {
    let imm = st.lookup_message(id).expect("exists");
    render::message(&st.messages[imm], prefer)
}

fn update_message(st: &mut MailboxState, id: &str, body: &Value, prefer: &Prefer) -> R {
    let imm = st
        .lookup_message(id)
        .ok_or_else(GraphError::not_found)?
        .to_string();
    let obj = body
        .as_object()
        .ok_or_else(|| bad_request("The request body must be a JSON object."))?;
    let is_draft = st.messages[&imm].msg.is_draft;
    for key in obj.keys() {
        let allowed = matches!(
            key.as_str(),
            "isRead" | "flag" | "categories" | "importance" | "inferenceClassification"
        ) || (is_draft
            && matches!(
                key.as_str(),
                "subject"
                    | "body"
                    | "toRecipients"
                    | "ccRecipients"
                    | "bccRecipients"
                    | "from"
                    // Send later (PidTagDeferredSendTime); the mock accepts
                    // it and still sends at once.
                    | "singleValueExtendedProperties"
            ));
        if !allowed {
            return Err(bad_request(format!(
                "The property '{key}' can't be updated on this message."
            )));
        }
    }
    let read = obj.get("isRead").and_then(Value::as_bool);
    let flagged = match obj.get("flag").map(|f| f["flagStatus"].as_str()) {
        None => None,
        Some(Some("flagged")) => Some(true),
        Some(Some("notFlagged" | "complete")) => Some(false),
        Some(_) => {
            return Err(bad_request(
                "flag.flagStatus must be flagged, notFlagged or complete.",
            ));
        }
    };
    let categories: Option<Vec<String>> = obj.get("categories").map(|c| {
        c.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    });
    let importance = obj
        .get("importance")
        .and_then(Value::as_str)
        .map(str::to_string);
    let subject = obj
        .get("subject")
        .and_then(Value::as_str)
        .map(str::to_string);
    let html = obj
        .get("body")
        .and_then(|b| b["content"].as_str())
        .map(str::to_string);
    let to = obj.get("toRecipients").map(recipients);
    let cc = obj.get("ccRecipients").map(recipients);
    let bcc = obj.get("bccRecipients").map(recipients);
    let from = obj.get("from").and_then(recipient);
    st.update_message(&imm, |m| {
        if let Some(f) = from {
            m.from = f;
        }
        if let Some(r) = read {
            m.is_read = r;
        }
        if let Some(f) = flagged {
            m.flagged = f;
        }
        if let Some(c) = categories {
            m.categories = c;
        }
        if let Some(i) = importance {
            m.importance = i;
        }
        if let Some(s) = subject {
            m.subject = s;
        }
        if let Some(h) = html {
            m.body_html = Some(h);
        }
        if let Some(t) = to {
            m.to = t;
        }
        if let Some(c) = cc {
            m.cc = c;
        }
        if let Some(b) = bcc {
            m.bcc = b;
        }
    })
    .map_err(|_| GraphError::not_found())?;
    Ok(Reply::ok(message_reply(st, &imm, prefer)))
}

/// `{"emailAddress": {"name", "address"}}` as a model address.
fn recipient(v: &Value) -> Option<crate::model::EmailAddress> {
    let e = v.get("emailAddress")?;
    Some(crate::model::EmailAddress::new(
        e["name"].as_str().unwrap_or(""),
        e["address"].as_str()?,
    ))
}

/// `[{"emailAddress": {"name", "address"}}]` as model addresses.
pub(super) fn recipients(v: &Value) -> Vec<crate::model::EmailAddress> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|r| {
                    let e = &r["emailAddress"];
                    crate::model::EmailAddress::new(
                        e["name"].as_str().unwrap_or(""),
                        e["address"].as_str().unwrap_or(""),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn move_message(st: &mut MailboxState, id: &str, body: &Value, prefer: &Prefer) -> R {
    let imm = st
        .lookup_message(id)
        .ok_or_else(GraphError::not_found)?
        .to_string();
    let dest = st
        .resolve_folder(str_field(body, "destinationId")?)
        .ok_or_else(GraphError::not_found)?;
    st.move_message(&imm, &dest)
        .map_err(|_| GraphError::not_found())?;
    Ok(created(message_reply(st, &imm, prefer)))
}

/// Beta `reportMessage`: junk and phish go to Junk Email, notJunk to the
/// Inbox, when `IsMessageMoveRequested` is true. Like Outlook.com, it
/// answers without the message's `id`.
fn report_message(st: &mut MailboxState, id: &str, body: &Value) -> R {
    let imm = st
        .lookup_message(id)
        .ok_or_else(GraphError::not_found)?
        .to_string();
    let dest = match str_field(body, "ReportAction")? {
        "junk" | "phish" => "junkemail",
        "notJunk" => "inbox",
        other => return Err(bad_request(format!("Invalid ReportAction '{other}'."))),
    };
    if body["IsMessageMoveRequested"].as_bool() == Some(true) {
        let dest = st.resolve_folder(dest).ok_or_else(GraphError::not_found)?;
        st.move_message(&imm, &dest)
            .map_err(|_| GraphError::not_found())?;
    }
    Ok(Reply::ok(
        serde_json::json!({ "@odata.type": "#microsoft.graph.message" }),
    ))
}

// --- folders ----------------------------------------------------------------

fn ensure_unique_name(
    st: &MailboxState,
    parent: Option<&str>,
    name: &str,
    except: Option<&str>,
) -> Result<(), GraphError> {
    let clash = st.folders.iter().any(|r| {
        r.folder.parent_folder_id.as_deref() == parent
            && r.folder.display_name.eq_ignore_ascii_case(name)
            && Some(r.folder.id.as_str()) != except
    });
    if clash {
        return Err(GraphError::new(
            409,
            "ErrorFolderExists",
            "A folder with the specified name already exists.",
        ));
    }
    Ok(())
}

fn refuse_well_known(st: &MailboxState, id: &str) -> Result<(), GraphError> {
    if st
        .folder(id)
        .is_some_and(|r| r.folder.well_known_name.is_some())
    {
        return Err(GraphError::new(
            403,
            "ErrorDeleteDistinguishedFolder",
            "Distinguished folders cannot be deleted or moved.",
        ));
    }
    Ok(())
}

/// Whether `id` is `ancestor` or inside it.
fn is_under(st: &MailboxState, id: &str, ancestor: &str) -> bool {
    let mut cur = Some(id.to_string());
    let mut guard = 0;
    while let Some(c) = cur {
        if c == ancestor {
            return true;
        }
        guard += 1;
        if guard > 64 {
            return false;
        }
        cur = st
            .folder(&c)
            .and_then(|r| r.folder.parent_folder_id.clone());
    }
    false
}

fn move_folder(st: &mut MailboxState, id: &str, dest: &str) -> Result<(), GraphError> {
    if is_under(st, dest, id) {
        return Err(bad_request("A folder can't be moved into itself."));
    }
    let name = st
        .folder(id)
        .map(|r| r.folder.display_name.clone())
        .ok_or_else(GraphError::not_found)?;
    ensure_unique_name(st, Some(dest), &name, Some(id))?;
    let dest = dest.to_string();
    st.update_folder(id, |f| f.parent_folder_id = Some(dest))
        .map_err(|_| GraphError::not_found())
}

fn create_folder(st: &mut MailboxState, parent: Option<&str>, body: &Value) -> R {
    let name = str_field(body, "displayName")?.to_string();
    let parent_id = match parent {
        Some(p) => st.resolve_folder(p).ok_or_else(GraphError::not_found)?,
        None => crate::model::ROOT_ID.to_string(),
    };
    ensure_unique_name(st, Some(&parent_id), &name, None)?;
    let mut f = Folder::new(name).under(parent_id);
    f.is_hidden = body
        .get("isHidden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let id = st.insert_folder(f);
    Ok(created(json!(render::folder(
        st,
        st.folder(&id).expect("inserted")
    ))))
}

// --- compose ------------------------------------------------------------------

/// Graph's limit for an attachment added in one request.
pub(super) const MAX_SIMPLE_ATTACHMENT: usize = 3 * 1024 * 1024;

fn accepted() -> Reply {
    Reply {
        status: 202,
        body: Value::Null,
        headers: Vec::new(),
    }
}

fn me(st: &MailboxState) -> EmailAddress {
    EmailAddress::new(st.user.display_name.clone(), st.user.mail.clone())
}

fn create_draft(st: &mut MailboxState, body: &Value, prefer: &Prefer) -> R {
    let mut m = Message::new(
        "drafts",
        body["subject"].as_str().unwrap_or(""),
        me(st),
        chrono::Utc::now(),
    );
    m.is_draft = true;
    m.is_read = true;
    if let Some(f) = recipient(&body["from"]) {
        m.from = f;
    }
    m.to = recipients(&body["toRecipients"]);
    m.cc = recipients(&body["ccRecipients"]);
    m.bcc = recipients(&body["bccRecipients"]);
    m.body_html = Some(body["body"]["content"].as_str().unwrap_or("").to_string());
    if let Some(i) = body["importance"].as_str() {
        m.importance = i.to_string();
    }
    let id = st.insert_message(m).map_err(|_| GraphError::not_found())?;
    Ok(created(message_reply(st, &id, prefer)))
}

fn add_attachment(st: &mut MailboxState, id: &str, body: &Value) -> R {
    let imm = st
        .lookup_message(id)
        .ok_or_else(GraphError::not_found)?
        .to_string();
    if !st.messages[&imm].msg.is_draft {
        return Err(bad_request("Attachments can only be added to drafts."));
    }
    let bytes = body["contentBytes"].as_str().unwrap_or("");
    if bytes.len() / 4 * 3 > MAX_SIMPLE_ATTACHMENT {
        return Err(GraphError::new(
            413,
            "ErrorRequestEntityTooLarge",
            "Attachments over 3 MB need an upload session.",
        ));
    }
    let att_id = st.new_id("att");
    let att = Attachment {
        id: att_id.clone(),
        name: str_field(body, "name")?.to_string(),
        content_type: body["contentType"]
            .as_str()
            .unwrap_or("application/octet-stream")
            .to_string(),
        is_inline: body["isInline"].as_bool().unwrap_or(false),
        content_id: body["contentId"].as_str().map(str::to_string),
        content_bytes: bytes.to_string(),
    };
    let reply = json!({
        "@odata.type": "#microsoft.graph.fileAttachment",
        "id": att_id,
        "name": att.name,
        "contentType": att.content_type,
        "isInline": att.is_inline,
        "contentId": att.content_id,
        "size": bytes.len() / 4 * 3,
    });
    st.update_message(&imm, |m| {
        if !att.is_inline {
            m.has_attachments = true;
        }
        m.attachments.push(att);
    })
    .map_err(|_| GraphError::not_found())?;
    Ok(created(reply))
}

fn delete_attachment(st: &mut MailboxState, id: &str, att: &str) -> R {
    let imm = st
        .lookup_message(id)
        .ok_or_else(GraphError::not_found)?
        .to_string();
    if !st.messages[&imm]
        .msg
        .attachments
        .iter()
        .any(|a| a.id == att)
    {
        return Err(GraphError::not_found());
    }
    st.update_message(&imm, |m| {
        m.attachments.retain(|a| a.id != att);
        m.has_attachments = m.attachments.iter().any(|a| !a.is_inline);
    })
    .map_err(|_| GraphError::not_found())?;
    Ok(no_content())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Response {
    Reply,
    ReplyAll,
    Forward,
}

fn create_response(
    st: &mut MailboxState,
    id: &str,
    kind: Response,
    body: &Value,
    prefer: &Prefer,
) -> R {
    let imm = st
        .lookup_message(id)
        .ok_or_else(GraphError::not_found)?
        .to_string();
    let orig = st.messages[&imm].msg.clone();
    let quoted = render::message_body(&st.messages[&imm]);
    let me = me(st);
    let prefix = if kind == Response::Forward {
        "FW: "
    } else {
        "RE: "
    };
    let subject = if orig.subject.to_ascii_uppercase().starts_with(prefix.trim()) {
        orig.subject.clone()
    } else {
        format!("{prefix}{}", orig.subject)
    };
    let mut m = Message::new("drafts", subject, me.clone(), chrono::Utc::now());
    m.is_draft = true;
    m.is_read = true;
    m.conversation_id = orig.conversation_id.clone();
    let not_me = |a: &EmailAddress| !a.address.eq_ignore_ascii_case(&me.address);
    match kind {
        Response::Reply => m.to = vec![orig.from.clone()],
        Response::ReplyAll => {
            m.to = std::iter::once(orig.from.clone())
                .chain(orig.to.iter().cloned())
                .filter(not_me)
                .collect();
            m.cc = orig.cc.iter().filter(|a| not_me(a)).cloned().collect();
        }
        Response::Forward => {
            m.to = recipients(&body["toRecipients"]);
            m.attachments = orig.attachments.clone();
            m.has_attachments = orig.has_attachments;
        }
    }
    let comment = body["comment"].as_str().unwrap_or("");
    m.body_html = Some(format!(
        "<html><body><div>{comment}</div><hr><div id=\"divRplyFwdMsg\"><b>From:</b> {} &lt;{}&gt;<br><b>Subject:</b> {}</div>{quoted}</body></html>",
        orig.from.name, orig.from.address, orig.subject
    ));
    let new_id = st.insert_message(m).map_err(|_| GraphError::not_found())?;
    Ok(created(message_reply(st, &new_id, prefer)))
}

fn send_draft(st: &mut MailboxState, id: &str) -> R {
    let imm = st
        .lookup_message(id)
        .ok_or_else(GraphError::not_found)?
        .to_string();
    let draft = st.messages[&imm].msg.clone();
    if !draft.is_draft {
        return Err(bad_request("Only drafts can be sent."));
    }
    if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
        return Err(GraphError::new(
            400,
            "ErrorInvalidRecipients",
            "At least one recipient isn't valid.",
        ));
    }
    let now = chrono::Utc::now();
    st.update_message(&imm, |m| {
        m.is_draft = false;
        m.sent_date_time = Some(now);
        m.received_date_time = now;
    })
    .map_err(|_| GraphError::not_found())?;
    st.move_message(&imm, "sentitems")
        .map_err(|_| GraphError::not_found())?;
    // Mail to the mailbox's own address arrives in its Inbox.
    let mine = st.user.mail.clone();
    let to_me = draft
        .to
        .iter()
        .chain(&draft.cc)
        .chain(&draft.bcc)
        .any(|a| a.address.eq_ignore_ascii_case(&mine));
    if to_me {
        let mut copy = draft.clone();
        copy.id = String::new();
        copy.folder_id = "inbox".into();
        copy.is_draft = false;
        copy.is_read = false;
        copy.bcc.clear();
        copy.received_date_time = now;
        copy.sent_date_time = Some(now);
        let internet_id = st.messages[&imm].msg.internet_message_id.clone();
        copy.internet_message_id = internet_id;
        st.insert_message(copy)
            .map_err(|_| GraphError::not_found())?;
    }
    Ok(accepted())
}

// --- events -------------------------------------------------------------------

/// Event writes: create (with recurrence expanded into occurrences),
/// update, delete, respond and cancel.
mod events {
    use super::{GraphError, R, accepted, bad_request, created};
    use crate::model::{Event, MailboxState};
    use crate::render::{self, Prefer};
    use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
    use chrono_tz::Tz;
    use serde_json::Value;

    /// Most occurrences one series expands to.
    const MAX_OCCURRENCES: usize = 520;

    /// The Windows zone names the tests use, plus IANA names.
    fn zone(name: &str) -> Result<Tz, GraphError> {
        let iana = match name {
            "" | "UTC" | "Coordinated Universal Time" => "UTC",
            "New Zealand Standard Time" => "Pacific/Auckland",
            "AUS Eastern Standard Time" => "Australia/Sydney",
            "GMT Standard Time" => "Europe/London",
            "Pacific Standard Time" => "America/Los_Angeles",
            "Eastern Standard Time" => "America/New_York",
            other => other,
        };
        iana.parse::<Tz>().map_err(|_| {
            GraphError::new(
                400,
                "TimeZoneNotSupportedException",
                format!("A valid TimeZone value must be specified. The following TimeZone value is not supported: '{name}'."),
            )
        })
    }

    fn local_to_utc(t: NaiveDateTime, tz: Tz) -> DateTime<Utc> {
        for m in (0..=180).step_by(15) {
            if let Some(d) = tz
                .from_local_datetime(&(t + Duration::minutes(m)))
                .earliest()
            {
                return d.with_timezone(&Utc);
            }
        }
        Utc.from_utc_datetime(&t)
    }

    /// Reads a `dateTimeTimeZone`: the wall-clock time and the zone name.
    fn date_time(v: &Value, name: &str) -> Result<(NaiveDateTime, String), GraphError> {
        let raw = v["dateTime"]
            .as_str()
            .ok_or_else(|| bad_request(format!("The property '{name}' is required.")))?;
        let zname = v["timeZone"].as_str().unwrap_or("UTC").to_string();
        zone(&zname)?;
        let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
            .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S"))
            .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M"))
            .map_err(|_| bad_request(format!("Cannot convert the literal '{raw}' to a date.")))?;
        Ok((naive, zname))
    }

    /// The instant a time stands for; all-day times are dates, kept as
    /// midnight UTC like every all-day event in the mock.
    fn instant(t: NaiveDateTime, zname: &str, all_day: bool) -> Result<DateTime<Utc>, GraphError> {
        if all_day {
            return Ok(Utc.from_utc_datetime(&t));
        }
        Ok(local_to_utc(t, zone(zname)?))
    }

    fn is_me(st: &MailboxState, ev: &Event) -> bool {
        ev.organizer
            .as_ref()
            .is_none_or(|o| o.address.eq_ignore_ascii_case(&st.user.mail))
    }

    /// Applies the fields Graph lets a PATCH or POST set.
    fn apply(st: &MailboxState, ev: &mut Event, body: &Value) -> Result<(), GraphError> {
        if let Some(s) = body.get("subject").and_then(Value::as_str) {
            ev.subject = s.to_string();
        }
        if let Some(b) = body.get("body") {
            ev.body_html = Some(b["content"].as_str().unwrap_or("").to_string());
        }
        if let Some(a) = body.get("isAllDay").and_then(Value::as_bool) {
            ev.is_all_day = a;
        }
        if let Some(v) = body.get("start").filter(|v| !v.is_null()) {
            let (t, z) = date_time(v, "start")?;
            ev.start = instant(t, &z, ev.is_all_day)?;
            ev.time_zone = Some(z);
        }
        if let Some(v) = body.get("end").filter(|v| !v.is_null()) {
            let (t, z) = date_time(v, "end")?;
            ev.end = instant(t, &z, ev.is_all_day)?;
        }
        if let Some(l) = body.get("location") {
            ev.location = l["displayName"].as_str().unwrap_or("").to_string();
        }
        if let Some(a) = body.get("attendees") {
            ev.attendees = super::recipients(a)
                .into_iter()
                .filter(|a| !a.address.eq_ignore_ascii_case(&st.user.mail) || !is_me(st, ev))
                .collect();
        }
        if let Some(s) = body.get("showAs").and_then(Value::as_str) {
            ev.show_as = s.to_string();
        }
        if let Some(on) = body.get("isReminderOn").and_then(Value::as_bool) {
            ev.reminder_minutes = on.then(|| ev.reminder_minutes.unwrap_or(15));
        }
        if let Some(m) = body
            .get("reminderMinutesBeforeStart")
            .and_then(Value::as_i64)
        {
            if ev.reminder_minutes.is_some() || body.get("isReminderOn").is_none() {
                ev.reminder_minutes = Some(m);
            }
        }
        if let Some(c) = body.get("categories").and_then(Value::as_array) {
            ev.categories = c
                .iter()
                .filter_map(|c| c.as_str().map(str::to_string))
                .collect();
        }
        if let Some(r) = body.get("recurrence") {
            ev.recurrence = (!r.is_null()).then(|| r.clone());
        }
        if ev.end < ev.start {
            return Err(bad_request(
                "The end of the event must come after its start.",
            ));
        }
        if ev.is_all_day {
            let midnight = |t: &DateTime<Utc>| t.timestamp() % 86_400 == 0;
            if !midnight(&ev.start) || !midnight(&ev.end) || ev.end == ev.start {
                return Err(bad_request(
                    "An all-day event's start and end must be at midnight, at least a day apart.",
                ));
            }
        }
        Ok(())
    }

    fn weekday(name: &str) -> Option<chrono::Weekday> {
        use chrono::Weekday::*;
        Some(match name.to_ascii_lowercase().as_str() {
            "monday" => Mon,
            "tuesday" => Tue,
            "wednesday" => Wed,
            "thursday" => Thu,
            "friday" => Fri,
            "saturday" => Sat,
            "sunday" => Sun,
            _ => return None,
        })
    }

    fn add_months(d: NaiveDate, n: i32) -> Option<NaiveDate> {
        let m0 = d.year() * 12 + d.month0() as i32 + n;
        NaiveDate::from_ymd_opt(m0.div_euclid(12), m0.rem_euclid(12) as u32 + 1, d.day())
    }

    /// The local start dates a `patternedRecurrence` gives, from the master's start.
    fn dates(rec: &Value, first: NaiveDate) -> Result<Vec<NaiveDate>, GraphError> {
        let p = &rec["pattern"];
        let r = &rec["range"];
        let interval = p["interval"].as_i64().unwrap_or(1).max(1);
        let start = r["startDate"]
            .as_str()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .unwrap_or(first);
        let until = match r["type"].as_str().unwrap_or("noEnd") {
            "endDate" => Some(
                r["endDate"]
                    .as_str()
                    .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                    .ok_or_else(|| bad_request("The recurrence range needs an endDate."))?,
            ),
            _ => None,
        };
        let count = match r["type"].as_str() {
            Some("numbered") => r["numberOfOccurrences"].as_u64().unwrap_or(1) as usize,
            _ => MAX_OCCURRENCES,
        }
        .min(MAX_OCCURRENCES);
        let horizon = until.unwrap_or(start + Duration::days(366 * 2));
        let mut out = Vec::new();
        let ok = |d: NaiveDate, out: &Vec<NaiveDate>| d <= horizon && out.len() < count;
        match p["type"].as_str().unwrap_or("") {
            "daily" => {
                let mut d = start;
                while ok(d, &out) {
                    out.push(d);
                    d += Duration::days(interval);
                }
            }
            "weekly" => {
                let mut days: Vec<chrono::Weekday> = p["daysOfWeek"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|d| d.as_str().and_then(weekday))
                            .collect()
                    })
                    .unwrap_or_default();
                if days.is_empty() {
                    days.push(start.weekday());
                }
                let week0 = start - Duration::days(start.weekday().num_days_from_sunday().into());
                let mut d = start;
                while ok(d, &out) {
                    let week = (d - week0).num_days() / 7;
                    if week % interval == 0 && days.contains(&d.weekday()) {
                        out.push(d);
                    }
                    d += Duration::days(1);
                }
            }
            "absoluteMonthly" => {
                let mut n = 0;
                while let Some(d) = add_months(start, n) {
                    if !ok(d, &out) {
                        break;
                    }
                    out.push(d);
                    n += interval as i32;
                }
            }
            "absoluteYearly" => {
                let mut n = 0;
                while let Some(d) = add_months(start, n) {
                    if !ok(d, &out) {
                        break;
                    }
                    out.push(d);
                    n += 12 * interval as i32;
                }
            }
            other => {
                return Err(bad_request(format!(
                    "The mock does not expand '{other}' recurrences."
                )));
            }
        }
        Ok(out)
    }

    /// Replaces a master's occurrences (not its exceptions) with a fresh expansion.
    fn expand(st: &mut MailboxState, master: &str) -> Result<(), GraphError> {
        let m = st.events[master].ev.clone();
        let Some(rec) = m.recurrence.clone() else {
            return Ok(());
        };
        let old: Vec<String> = st
            .events
            .values()
            .filter(|r| r.ev.series_master_id.as_deref() == Some(master))
            .map(|r| r.ev.id.clone())
            .collect();
        for id in old {
            st.delete_event(&id).map_err(|_| GraphError::not_found())?;
        }
        let tz = if m.is_all_day {
            Tz::UTC
        } else {
            zone(m.time_zone.as_deref().unwrap_or("UTC"))?
        };
        let local = m.start.with_timezone(&tz);
        let length = m.end - m.start;
        for d in dates(&rec, local.date_naive())? {
            let start = local_to_utc(d.and_time(local.time()), tz);
            let mut occ = m.clone();
            occ.id = String::new();
            occ.event_type = "occurrence".into();
            occ.series_master_id = Some(master.to_string());
            occ.recurrence = None;
            occ.transaction_id = None;
            occ.start = start;
            occ.end = start + length;
            st.insert_event(occ).map_err(|_| GraphError::not_found())?;
        }
        Ok(())
    }

    fn reply(st: &MailboxState, id: &str, prefer: &Prefer) -> Value {
        render::event(&st.user, &st.events[id], prefer)
    }

    pub(super) fn create(st: &mut MailboxState, cal: &str, body: &Value, prefer: &Prefer) -> R {
        let cal = st.resolve_calendar(cal).ok_or_else(GraphError::not_found)?;
        let tid = body["transactionId"].as_str().map(str::to_string);
        if let Some(t) = &tid {
            // A retried POST: the event it made already.
            if let Some(r) = st
                .events
                .values()
                .find(|r| r.ev.transaction_id.as_deref() == Some(t))
            {
                let id = r.ev.id.clone();
                return Ok(created(reply(st, &id, prefer)));
            }
        }
        if body.get("start").is_none() || body.get("end").is_none() {
            return Err(bad_request("An event needs a start and an end."));
        }
        let now = Utc::now();
        let mut ev = Event::new("", now, now);
        ev.calendar_id = cal;
        ev.transaction_id = tid;
        ev.reminder_minutes = Some(15);
        apply(st, &mut ev, body)?;
        let recurring = ev.recurrence.is_some();
        if recurring {
            ev.event_type = "seriesMaster".into();
        }
        let id = st.insert_event(ev).map_err(|_| GraphError::not_found())?;
        if recurring {
            expand(st, &id)?;
        }
        Ok(created(reply(st, &id, prefer)))
    }

    const ORGANIZER_ONLY: [&str; 6] = [
        "subject",
        "start",
        "end",
        "location",
        "attendees",
        "recurrence",
    ];

    pub(super) fn update(st: &mut MailboxState, id: &str, body: &Value, prefer: &Prefer) -> R {
        let ev = st
            .events
            .get(id)
            .ok_or_else(GraphError::not_found)?
            .ev
            .clone();
        if !is_me(st, &ev) && ORGANIZER_ONLY.iter().any(|k| body.get(*k).is_some()) {
            return Err(GraphError::new(
                400,
                "ErrorInvalidRequest",
                "Your request can't be completed. Only the organizer can change this property.",
            ));
        }
        let mut next = ev.clone();
        apply(st, &mut next, body)?;
        let retime =
            next.start != ev.start || next.end != ev.end || next.recurrence != ev.recurrence;
        if ev.event_type == "occurrence" && (retime || body.get("subject").is_some()) {
            next.event_type = "exception".into();
        }
        st.update_event(id, |e| *e = next.clone())
            .map_err(|_| GraphError::not_found())?;
        if ev.event_type == "seriesMaster" {
            if retime {
                expand(st, id)?;
            } else {
                let occs: Vec<String> = st
                    .events
                    .values()
                    .filter(|r| {
                        r.ev.series_master_id.as_deref() == Some(id)
                            && r.ev.event_type == "occurrence"
                    })
                    .map(|r| r.ev.id.clone())
                    .collect();
                for o in occs {
                    st.update_event(&o, |e| {
                        let (s, en) = (e.start, e.end);
                        let mut copy = next.clone();
                        copy.start = s;
                        copy.end = en;
                        copy.event_type = "occurrence".into();
                        copy.series_master_id = Some(id.to_string());
                        copy.recurrence = None;
                        copy.transaction_id = None;
                        *e = copy;
                    })
                    .map_err(|_| GraphError::not_found())?;
                }
            }
        }
        Ok(super::Reply::ok(reply(st, id, prefer)))
    }

    /// Deletes an event, or a series master with all its instances.
    pub(super) fn delete(st: &mut MailboxState, id: &str) -> Result<(), GraphError> {
        if !st.events.contains_key(id) {
            return Err(GraphError::not_found());
        }
        let kids: Vec<String> = st
            .events
            .values()
            .filter(|r| r.ev.series_master_id.as_deref() == Some(id))
            .map(|r| r.ev.id.clone())
            .collect();
        for k in kids {
            st.delete_event(&k).map_err(|_| GraphError::not_found())?;
        }
        st.delete_event(id).map_err(|_| GraphError::not_found())
    }

    pub(super) fn respond(st: &mut MailboxState, id: &str, response: &str) -> R {
        let ev = st
            .events
            .get(id)
            .ok_or_else(GraphError::not_found)?
            .ev
            .clone();
        if is_me(st, &ev) {
            return Err(GraphError::new(
                400,
                "ErrorInvalidRequest",
                "Your request can't be completed. You can't respond to this meeting because you're the meeting organizer.",
            ));
        }
        if response == "declined" {
            // Declining takes the meeting off the calendar.
            delete(st, id)?;
            return Ok(accepted());
        }
        let ids: Vec<String> = st
            .events
            .values()
            .filter(|r| r.ev.id == id || r.ev.series_master_id.as_deref() == Some(id))
            .map(|r| r.ev.id.clone())
            .collect();
        for i in ids {
            st.update_event(&i, |e| e.response = Some(response.to_string()))
                .map_err(|_| GraphError::not_found())?;
        }
        Ok(accepted())
    }

    pub(super) fn cancel(st: &mut MailboxState, id: &str) -> R {
        let ev = st
            .events
            .get(id)
            .ok_or_else(GraphError::not_found)?
            .ev
            .clone();
        if !is_me(st, &ev) {
            return Err(GraphError::new(
                400,
                "ErrorInvalidRequest",
                "Your request can't be completed. Only the meeting organizer can cancel a meeting.",
            ));
        }
        delete(st, id)?;
        Ok(accepted())
    }
}
