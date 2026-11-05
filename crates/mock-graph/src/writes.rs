// SPDX-License-Identifier: MIT OR Apache-2.0
//! Write routes: the subset of Graph's mail and calendar writes Familiar uses.
//!
//! Each route changes the mailbox model through the same mutation methods
//! tests use, so every write shows up in delta rounds like a change made
//! anywhere else.

use super::{GraphError, R, Reply, strip_prefix_ci};
use crate::model::{Folder, MailboxState};
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

/// Splits `/v1.0/me/...` or `/v1.0/users/<me>/...` into segments after the user.
fn segments<'a>(st: &MailboxState, path: &'a str) -> Result<Vec<&'a str>, GraphError> {
    let Some(rest) = strip_prefix_ci(path, "/v1.0/") else {
        return Err(GraphError::new(
            400,
            "BadRequest",
            "Invalid version: the mock serves /v1.0 only.",
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
                "subject" | "body" | "toRecipients" | "ccRecipients" | "bccRecipients" | "from"
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
    st.update_message(&imm, |m| {
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
    })
    .map_err(|_| GraphError::not_found())?;
    Ok(Reply::ok(message_reply(st, &imm, prefer)))
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
