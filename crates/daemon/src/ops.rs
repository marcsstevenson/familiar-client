// SPDX-License-Identifier: MIT OR Apache-2.0
//! The op runner: sends one account's queued writes to the server.
//!
//! Ops run one at a time per account, in queue order, at user priority on
//! the account's Graph scheduler (so they go ahead of background sync).
//! An op that fails for a passing reason (offline, throttled, server
//! trouble, an expired sign-in) waits `op_retry_min` doubling up to
//! `op_retry_max` and is tried again, for as long as it takes. One the
//! server refuses for good fails: its local change is undone, and so are
//! later ops that needed it (moves into a folder that couldn't be created).
//!
//! An op claimed when `famd` stopped (or was killed) is put back at the
//! front of the queue at start. Every op is safe to send twice; see
//! `familiar_provider_graph::MailApi`'s write methods.

use std::sync::Arc;
use std::time::Duration;

use familiar_core::compose::{AttachmentData, DraftOp, DraftUpdateOp, Recipient, ResponseKind};
use familiar_core::events::{self, EventOp};
use familiar_core::ops::{OpPayload, OpResult, finish_local, undo_local};
use familiar_provider_graph::{CalendarApi, Error as GraphError, GraphEvent, MailApi, Priority};
use familiar_store::{
    AccountId, Calendar, CalendarId, EventId, FolderId, LOCAL_REMOTE_PREFIX, MessageId, NewAudit,
    Op, OpState, SharedWriter,
};
use serde_json::{Value, json};
use tokio::sync::{Notify, watch};

use crate::policy::{Clock, SyncPolicy};

/// Signals between the runners, request handlers and `ops.wait`.
#[derive(Debug)]
pub struct OpSignals {
    /// Bumped whenever an op finishes or fails.
    pub changed: watch::Sender<u64>,
}

impl Default for OpSignals {
    fn default() -> Self {
        Self {
            changed: watch::channel(0).0,
        }
    }
}

impl OpSignals {
    fn bump(&self) {
        self.changed.send_modify(|n| *n += 1);
    }
}

/// One account's op runner.
pub struct OpRunner {
    pub(crate) key: String,
    pub(crate) account: AccountId,
    pub(crate) api: MailApi,
    pub(crate) cal: CalendarApi,
    pub(crate) w: SharedWriter,
    pub(crate) policy: Arc<SyncPolicy>,
    pub(crate) clock: Clock,
    pub(crate) wake: Arc<Notify>,
    pub(crate) signals: Arc<OpSignals>,
    pub(crate) shutdown: watch::Receiver<bool>,
}

/// Why an op didn't land.
enum Failure {
    /// Try again later.
    Passing(String, Duration),
    /// The server won't take it.
    Final(String),
}

fn classify(e: &GraphError, policy: &SyncPolicy, attempts: u32) -> Failure {
    let msg = e.to_string();
    match e {
        GraphError::Network(_) | GraphError::Throttled { .. } | GraphError::Store(_) => {
            Failure::Passing(msg, policy.op_retry(attempts))
        }
        GraphError::Auth(_) => Failure::Passing(msg, policy.op_retry_max),
        GraphError::Graph { status, .. } if *status >= 500 || *status == 408 => {
            Failure::Passing(msg, policy.op_retry(attempts))
        }
        GraphError::Graph { .. }
        | GraphError::NotFound(_)
        | GraphError::Decode(_)
        | GraphError::SyncStateGone(_) => Failure::Final(msg),
    }
}

impl OpRunner {
    /// Runs until the daemon shuts down.
    pub async fn run(mut self) {
        loop {
            if *self.shutdown.borrow() {
                break;
            }
            let now = (self.clock)();
            let acc = self.account;
            let claimed = self.w.write(|tx| tx.claim_op_for(acc, now));
            let wait = match claimed {
                Ok(Some(op)) => {
                    self.run_one(op).await;
                    continue;
                }
                Ok(None) => match self.w.write(|tx| tx.next_op_due(acc)) {
                    Ok(Some(at)) => Some(Duration::from_secs((at - now).max(1) as u64)),
                    Ok(None) => None,
                    Err(e) => {
                        crate::log(&format!("{}: op queue: {e}", self.key));
                        Some(self.policy.op_retry_max)
                    }
                },
                Err(e) => {
                    crate::log(&format!("{}: op queue: {e}", self.key));
                    Some(self.policy.op_retry_max)
                }
            };
            let mut shutdown = self.shutdown.clone();
            match wait {
                Some(d) => tokio::select! {
                    _ = tokio::time::sleep(d) => {}
                    _ = self.wake.notified() => {}
                    _ = shutdown.changed() => {}
                },
                None => tokio::select! {
                    _ = self.wake.notified() => {}
                    _ = shutdown.changed() => {}
                },
            }
        }
    }

    async fn run_one(&mut self, op: Op) {
        let mut payload = match OpPayload::of(&op) {
            Ok(p) => p,
            Err(e) => {
                self.fail(&op, None, &e.message).await;
                return;
            }
        };
        match self.send(&op, &mut payload).await {
            Ok(result) => {
                let acc = self.account;
                let r = self.w.write(|tx| {
                    finish_local(tx, &payload, acc, &result)?;
                    tx.complete_op(op.id)?;
                    tx.add_audit(&NewAudit {
                        account: Some(acc),
                        source: op.source.clone(),
                        command: op.kind.clone(),
                        outcome: "done".into(),
                        op: Some(op.id),
                        detail: op.summary.clone(),
                    })?;
                    Ok(())
                });
                if let Err(e) = r {
                    crate::log(&format!("{}: op {} landed but: {e}", self.key, op.id.0));
                }
                self.signals.bump();
            }
            Err(e) => match classify(&e, &self.policy, op.attempts.max(1) as u32) {
                Failure::Passing(msg, wait) => {
                    crate::log(&format!(
                        "{}: op {} ({}): {msg}; retrying in {}s",
                        self.key,
                        op.id.0,
                        op.kind,
                        wait.as_secs()
                    ));
                    let at = (self.clock)() + wait.as_secs().max(1) as i64;
                    if let Err(e) = self.w.write(|tx| tx.retry_op(op.id, &msg, at)) {
                        crate::log(&format!("{}: op {}: {e}", self.key, op.id.0));
                    }
                    self.signals.bump();
                }
                Failure::Final(msg) => self.fail(&op, Some(&payload), &msg).await,
            },
        }
    }

    /// Gives up on an op: undoes its change, and cancels and undoes later
    /// ops that depend on a folder it was creating.
    async fn fail(&mut self, op: &Op, payload: Option<&OpPayload>, msg: &str) {
        crate::log(&format!(
            "{}: op {} ({}) failed: {msg}",
            self.key, op.id.0, op.kind
        ));
        let acc = self.account;
        let r = self.w.write(|tx| {
            let created = match payload {
                Some(OpPayload::CreateDraft(d)) => {
                    Some(familiar_store::message_target(MessageId(d.message)))
                }
                Some(OpPayload::CreateEvent(e)) => {
                    Some(familiar_store::event_target(EventId(e.event)))
                }
                _ => None,
            };
            if let Some(target) = created {
                for o in tx.open_ops()? {
                    if o.account == acc
                        && o.id > op.id
                        && o.state == OpState::Pending
                        && o.target == target
                    {
                        tx.fail_op(o.id, "the item it changes couldn't be created")?;
                    }
                }
            }
            if let Some(OpPayload::CreateFolder { folder, .. }) = payload {
                let mut later: Vec<Op> = tx
                    .open_ops()?
                    .into_iter()
                    .filter(|o| o.account == acc && o.id > op.id && o.state == OpState::Pending)
                    .collect();
                later.reverse();
                for o in later {
                    let Ok(p) = OpPayload::of(&o) else { continue };
                    if p.uses_folder(*folder) {
                        undo_local(tx, &p)?;
                        tx.fail_op(o.id, "the folder it needed couldn't be created")?;
                    }
                }
            }
            if let Some(p) = payload {
                undo_local(tx, p)?;
            }
            tx.fail_op(op.id, msg)?;
            tx.add_audit(&NewAudit {
                account: Some(acc),
                source: op.source.clone(),
                command: op.kind.clone(),
                outcome: "failed".into(),
                op: Some(op.id),
                detail: format!("{}: {msg}", op.summary),
            })?;
            Ok(())
        });
        if let Err(e) = r {
            crate::log(&format!("{}: op {}: {e}", self.key, op.id.0));
        }
        self.signals.bump();
    }

    fn message_remote(&self, id: i64) -> Result<String, GraphError> {
        self.w
            .write(|tx| tx.message(MessageId(id)))?
            .map(|m| m.remote_id)
            .ok_or_else(|| GraphError::NotFound(format!("message {id} is no longer in the store")))
    }

    fn folder_remote(&self, id: i64) -> Result<String, GraphError> {
        let f = self.w.write(|tx| tx.folder(FolderId(id)))?.ok_or_else(|| {
            GraphError::NotFound(format!("folder {id} is no longer in the store"))
        })?;
        if f.remote_id.starts_with(LOCAL_REMOTE_PREFIX) {
            return Err(GraphError::NotFound(format!(
                "folder {:?} was never created on the server",
                f.name
            )));
        }
        Ok(f.remote_id)
    }

    /// Saves an op's progress, so a retry carries on from there.
    fn save(&self, op: &Op, p: &OpPayload) -> Result<(), GraphError> {
        let json = serde_json::to_string(p)?;
        self.w.write(|tx| tx.set_op_payload(op.id, &json))?;
        Ok(())
    }

    async fn add_attachments(
        &self,
        op: &Op,
        p: &mut OpPayload,
        remote: &str,
    ) -> Result<(), GraphError> {
        loop {
            let (next, done) = match p {
                OpPayload::CreateDraft(d) => (
                    d.attachments.get(d.attachments_done).cloned(),
                    &mut d.attachments_done,
                ),
                OpPayload::UpdateDraft(d) => (
                    d.attachments.get(d.attachments_done).cloned(),
                    &mut d.attachments_done,
                ),
                _ => return Ok(()),
            };
            let Some(a) = next else { return Ok(()) };
            *done += 1;
            self.api
                .add_attachment(remote, &attachment_json(&a), Priority::User)
                .await?;
            self.save(op, p)?;
        }
    }

    async fn send(&self, op: &Op, p: &mut OpPayload) -> Result<OpResult, GraphError> {
        let pr = Priority::User;
        let api = &self.api;
        match p {
            OpPayload::CreateDraft(d) => {
                let remote = match d.remote_id.clone() {
                    Some(r) => r,
                    None => {
                        let v = match d.respond_to {
                            Some((orig, kind)) => {
                                let action = match kind {
                                    ResponseKind::Reply => "createReply",
                                    ResponseKind::ReplyAll => "createReplyAll",
                                    ResponseKind::Forward => "createForward",
                                };
                                api.create_response(&self.message_remote(orig)?, action, pr)
                                    .await?
                            }
                            None => api.create_draft(&draft_json(d), pr).await?,
                        };
                        let r = v["id"]
                            .as_str()
                            .ok_or_else(|| GraphError::Decode("draft without an id".into()))?
                            .to_string();
                        d.remote_id = Some(r.clone());
                        let local = MessageId(d.message);
                        self.w.write(|tx| tx.set_message_remote_id(local, &r))?;
                        let snapshot = p.clone();
                        self.save(op, &snapshot)?;
                        r
                    }
                };
                if let OpPayload::CreateDraft(d) = &*p {
                    if d.respond_to.is_some() {
                        // The server's draft has the threading; Familiar's
                        // body and recipients replace its defaults.
                        api.update_draft(&remote, &draft_json(d), pr).await?;
                    }
                }
                self.add_attachments(op, p, &remote).await?;
                return Ok(OpResult {
                    message_remote_id: Some(remote),
                    ..OpResult::default()
                });
            }
            OpPayload::UpdateDraft(d) => {
                let remote = self.message_remote(d.message)?;
                if !d.pruned && (d.html.is_some() || !d.remove_attachments.is_empty()) {
                    let existing = api.list_attachments(&remote, pr).await?;
                    for a in prune_attachments(d, &existing) {
                        match api.delete_attachment(&remote, &a, pr).await {
                            Ok(()) | Err(GraphError::NotFound(_)) => {}
                            Err(e) => return Err(e),
                        }
                    }
                    d.pruned = true;
                    let snapshot = p.clone();
                    self.save(op, &snapshot)?;
                }
                let OpPayload::UpdateDraft(d) = &*p else {
                    unreachable!()
                };
                let mut fields = serde_json::Map::new();
                if let Some(s) = &d.subject {
                    fields.insert("subject".into(), json!(s));
                }
                if let Some(h) = &d.html {
                    fields.insert("body".into(), json!({"contentType": "HTML", "content": h}));
                }
                if let Some(v) = &d.to {
                    fields.insert("toRecipients".into(), recipients_json(v));
                }
                if let Some(v) = &d.cc {
                    fields.insert("ccRecipients".into(), recipients_json(v));
                }
                if let Some(v) = &d.bcc {
                    fields.insert("bccRecipients".into(), recipients_json(v));
                }
                if !fields.is_empty() && d.attachments_done == 0 {
                    api.update_draft(&remote, &Value::Object(fields), pr)
                        .await?;
                }
                self.add_attachments(op, p, &remote).await?;
            }
            OpPayload::SendDraft {
                message,
                internet_message_id,
            } => {
                let remote = self.message_remote(*message)?;
                if internet_message_id.is_none() {
                    match api.internet_message_id(&remote, pr).await {
                        Ok(id) => {
                            *internet_message_id = id;
                            let snapshot = p.clone();
                            self.save(op, &snapshot)?;
                        }
                        Err(GraphError::NotFound(_)) => {}
                        Err(e) => return Err(e),
                    }
                }
                let OpPayload::SendDraft {
                    internet_message_id,
                    ..
                } = &*p
                else {
                    unreachable!()
                };
                match api.send_draft(&remote, pr).await {
                    Err(GraphError::NotFound(e)) => {
                        // Gone: sent by a try whose answer was lost, or
                        // deleted. Sent Items tells which.
                        let sent = match internet_message_id {
                            Some(id) => api.find_sent(id, pr).await?.is_some(),
                            None => false,
                        };
                        if !sent {
                            return Err(GraphError::NotFound(e));
                        }
                    }
                    r => r?,
                }
            }
            OpPayload::CreateEvent(e) => self.create_event(op, e).await?,
            OpPayload::UpdateEvent(u) => {
                let (row, cal) = self.event_row(u.event)?;
                let target = match &u.series {
                    Some(m) => m.clone(),
                    None => remote_of(&row.data.remote_id)?,
                };
                let body = events::changes_json(&u.changes, &row.data);
                let ge = self.cal.update_event(&target, &body, pr).await?;
                if u.series.is_some() {
                    self.refresh_series(&cal, &target, None).await?;
                } else {
                    self.store_event(&cal, row.id, &ge)?;
                }
            }
            OpPayload::RespondEvent(r) => {
                let (row, _) = self.event_row(r.event)?;
                let target = match &r.series {
                    Some(m) => m.clone(),
                    None => remote_of(&row.data.remote_id)?,
                };
                self.cal
                    .respond(
                        &target,
                        r.response.action(),
                        r.comment.as_deref(),
                        r.send_response,
                        pr,
                    )
                    .await?;
            }
            OpPayload::DeleteEvent(d) => {
                let (row, _) = self.event_row(d.event)?;
                let target = match &d.series {
                    Some(m) => m.clone(),
                    None => remote_of(&row.data.remote_id)?,
                };
                let r = match &d.comment {
                    Some(c) => self.cal.cancel_event(&target, c, pr).await,
                    None => self.cal.delete_event(&target, pr).await,
                };
                match r {
                    // Already gone: the retry of a delete that landed.
                    Err(GraphError::NotFound(_)) => {}
                    r => r?,
                }
            }
            OpPayload::SetRead { message, read, .. } => {
                api.set_read(&self.message_remote(*message)?, *read, pr)
                    .await?
            }
            OpPayload::SetFlag {
                message, flagged, ..
            } => {
                api.set_flag(&self.message_remote(*message)?, *flagged, pr)
                    .await?
            }
            OpPayload::MoveMessage { message, to, .. } => {
                let dest = self.folder_remote(*to)?;
                api.move_message(&self.message_remote(*message)?, &dest, pr)
                    .await?;
            }
            OpPayload::DeleteMessage { message } => {
                match api
                    .delete_message(&self.message_remote(*message)?, pr)
                    .await
                {
                    // Already gone: the retry of a delete that landed.
                    Err(GraphError::NotFound(_)) => {}
                    r => r?,
                }
            }
            OpPayload::CreateFolder { parent, name, .. } => {
                let parent = parent.map(|p| self.folder_remote(p)).transpose()?;
                let f = api.create_folder(parent.as_deref(), name, pr).await?;
                return Ok(OpResult {
                    folder_remote_id: Some(f.id),
                    ..OpResult::default()
                });
            }
            OpPayload::RenameFolder { folder, name, .. } => {
                api.rename_folder(&self.folder_remote(*folder)?, name, pr)
                    .await?
            }
            OpPayload::MoveFolder { folder, to, .. } => {
                let dest = match to {
                    Some(t) => self.folder_remote(*t)?,
                    None => "msgfolderroot".to_string(),
                };
                api.move_folder(&self.folder_remote(*folder)?, &dest, pr)
                    .await?
            }
            OpPayload::DeleteFolder { folder, .. } => {
                match api.delete_folder(&self.folder_remote(*folder)?, pr).await {
                    Err(GraphError::NotFound(_)) => {}
                    r => r?,
                }
            }
        }
        Ok(OpResult::default())
    }
}

fn remote_of(id: &str) -> Result<String, GraphError> {
    if id.starts_with(LOCAL_REMOTE_PREFIX) {
        return Err(GraphError::NotFound(
            "the event was never created on the server".into(),
        ));
    }
    Ok(id.to_string())
}

impl OpRunner {
    fn calendar(&self, id: CalendarId) -> Result<Calendar, GraphError> {
        let acc = self.account;
        self.w
            .write(|tx| tx.calendars(acc))?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| {
                GraphError::NotFound(format!("calendar {} is no longer in the store", id.0))
            })
    }

    fn event_row(&self, id: i64) -> Result<(familiar_store::Event, Calendar), GraphError> {
        let e = self
            .w
            .write(|tx| tx.event(EventId(id)))?
            .ok_or_else(|| GraphError::NotFound(format!("event {id} is no longer in the store")))?;
        let cal = self.calendar(e.data.calendar)?;
        Ok((e, cal))
    }

    /// Writes the server's copy of an event over its local row.
    fn store_event(
        &self,
        cal: &Calendar,
        local: EventId,
        ge: &GraphEvent,
    ) -> Result<(), GraphError> {
        let Some(d) = ge.to_event_data(self.account, cal.id) else {
            return Ok(());
        };
        self.w.write(|tx| match tx.replace_event(local, &d) {
            // Gone locally meanwhile: keep the server's copy.
            Err(familiar_store::Error::NotFound(_)) => tx.upsert_event(&d, cal.round).map(|_| ()),
            r => r,
        })?;
        Ok(())
    }

    /// Re-reads a series' occurrences in the calendar's window, replacing
    /// the local rows (and `placeholder`, the row a new series showed as).
    async fn refresh_series(
        &self,
        cal: &Calendar,
        master: &str,
        placeholder: Option<EventId>,
    ) -> Result<(), GraphError> {
        let pr = Priority::User;
        let now = (self.clock)();
        let window = match (cal.window_start, cal.window_end) {
            (Some(s), Some(e)) => (s, e),
            _ => (now - 30 * 86_400, now + 365 * 86_400),
        };
        let mut items = self.cal.instances(master, window, pr).await?;
        if items.iter().any(GraphEvent::is_sparse) {
            if let Some(m) = self.cal.event(master, pr).await? {
                for i in items.iter_mut().filter(|i| i.is_sparse()) {
                    i.fill_from_master(&m);
                }
            }
        }
        let acc = self.account;
        let rows: Vec<_> = items
            .iter()
            .filter_map(|g| g.to_event_data(acc, cal.id))
            .collect();
        self.w.write(|tx| {
            if let Some(p) = placeholder {
                tx.delete_event(p)?;
            }
            let keep: std::collections::HashSet<&str> =
                rows.iter().map(|d| d.remote_id.as_str()).collect();
            for old in tx.events_of_series(acc, master)? {
                if !keep.contains(old.data.remote_id.as_str()) {
                    tx.delete_event(old.id)?;
                }
            }
            for d in &rows {
                tx.upsert_event(d, cal.round)?;
            }
            Ok(())
        })?;
        Ok(())
    }

    /// The email an event is made from, as a line linking back to it.
    async fn mail_link(&self, id: i64) -> Option<String> {
        let m = self.w.write(|tx| tx.message(MessageId(id))).ok()??;
        let link = match m.remote_id.starts_with(LOCAL_REMOTE_PREFIX) {
            true => None,
            false => self
                .api
                .raw(
                    familiar_provider_graph::Method::GET,
                    &format!("/messages/{}?$select=webLink", m.remote_id),
                    None,
                    Priority::User,
                )
                .await
                .ok()
                .and_then(|v| v["webLink"].as_str().map(str::to_string)),
        };
        let esc = familiar_core::compose::text_to_html;
        let subject = if m.subject.is_empty() {
            "(no subject)"
        } else {
            &m.subject
        };
        let title = match link {
            Some(l) => format!("<a href=\"{}\">{}</a>", esc(&l), esc(subject)),
            None => format!("\u{201c}{}\u{201d}", esc(subject)),
        };
        let from = if m.from.name.is_empty() {
            &m.from.addr
        } else {
            &m.from.name
        };
        Some(format!(
            "<p style=\"color:#666\">From the email {title} from {}.</p>",
            esc(from)
        ))
    }

    async fn create_event(&self, op: &Op, e: &mut EventOp) -> Result<(), GraphError> {
        let pr = Priority::User;
        let cal = self.calendar(CalendarId(e.calendar))?;
        let local = EventId(e.event);
        let ge = match e.remote_id.clone() {
            Some(r) => match self.cal.event(&r, pr).await? {
                Some(g) => g,
                None => return Err(GraphError::NotFound("the new event has gone".into())),
            },
            None => {
                let link = match e.from_mail {
                    Some(m) => self.mail_link(m).await,
                    None => None,
                };
                let body = events::event_json(&e.fields, &e.transaction_id, link.as_deref());
                let g = self.cal.create_event(&cal.remote_id, &body, pr).await?;
                e.remote_id = Some(g.id.clone());
                self.save(op, &OpPayload::CreateEvent(e.clone()))?;
                g
            }
        };
        if e.fields.recurrence.is_some() {
            self.refresh_series(&cal, &ge.id, Some(local)).await
        } else {
            self.store_event(&cal, local, &ge)
        }
    }
}

fn recipients_json(v: &[Recipient]) -> Value {
    Value::Array(
        v.iter()
            .map(|r| json!({"emailAddress": {"name": r.name, "address": r.address}}))
            .collect(),
    )
}

fn draft_json(d: &DraftOp) -> Value {
    let mut v = json!({
        "subject": d.subject,
        "body": {"contentType": "HTML", "content": d.html},
        "toRecipients": recipients_json(&d.to),
        "ccRecipients": recipients_json(&d.cc),
        "bccRecipients": recipients_json(&d.bcc),
    });
    if let Some(i) = &d.importance {
        v["importance"] = json!(i);
    }
    v
}

/// Works out what an update does to the draft's attachments (Graph JSON in
/// `existing`): returns the IDs to delete (asked for by name or ID, or inline
/// images with a content ID the new body no longer shows), and drops from
/// `d.attachments` the inline images the draft already has.
fn prune_attachments(d: &mut DraftUpdateOp, existing: &[Value]) -> Vec<String> {
    let mut delete = Vec::new();
    let mut kept_cids = Vec::new();
    for a in existing {
        let id = a["id"].as_str().unwrap_or_default();
        let name = a["name"].as_str().unwrap_or_default();
        let cid = a["contentId"].as_str().filter(|c| !c.is_empty());
        let asked = d
            .remove_attachments
            .iter()
            .any(|r| r == id || r.eq_ignore_ascii_case(name));
        let unshown = match (&d.html, cid) {
            (Some(h), Some(c)) if a["isInline"].as_bool() == Some(true) => {
                !h.contains(&format!("cid:{c}"))
            }
            _ => false,
        };
        if id.is_empty() {
            continue;
        }
        if asked || unshown {
            delete.push(id.to_string());
        } else if let Some(c) = cid {
            kept_cids.push(c.to_string());
        }
    }
    d.attachments
        .retain(|a| !(a.inline && a.content_id.as_ref().is_some_and(|c| kept_cids.contains(c))));
    delete
}

fn attachment_json(a: &AttachmentData) -> Value {
    let mut v = json!({
        "@odata.type": "#microsoft.graph.fileAttachment",
        "name": a.name,
        "contentType": a.content_type,
        "contentBytes": a.content_b64,
        "isInline": a.inline,
    });
    if let Some(c) = &a.content_id {
        v["contentId"] = json!(c);
    }
    v
}
