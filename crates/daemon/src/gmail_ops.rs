// SPDX-License-Identifier: MIT OR Apache-2.0
//! The op runner for Gmail accounts (Gmail plan WP G.6).
//!
//! The queue works as it does for Graph (see [`crate::ops`]): one op at a
//! time per account, at user priority, passing failures retried and final
//! ones undone. What differs is how each op is said to Gmail:
//!
//! - read and flag are the `UNREAD` and `STARRED` labels;
//! - a move takes the label it came from off and puts the one it goes to on
//!   (All Mail has no label, so a move there is archive), Trash is
//!   `messages.trash` and leaving it `messages.untrash`, and a report is a
//!   move in or out of Spam;
//! - folders are labels named by their path: creating, renaming or moving
//!   one names its label (and its sub-labels, which Gmail doesn't rename),
//!   and deleting one deletes the labels, after which the label sync moves
//!   their mail to the folders it is still in.
//!
//! Drafts and sending (WP G.7) are whole MIME messages built by Familiar
//! (`familiar_provider_gmail::Outgoing`): a draft update starts from the
//! draft Gmail has; a reply carries `In-Reply-To` and the thread ID; a
//! forward carries the original's files. A From that isn't a verified
//! send-as address is refused before sending, rather than letting Gmail
//! rewrite it. Gmail has no send later in its API, so that is refused.
//! Gmail can't delete mail for good with Familiar's scope, so that is
//! refused when planned.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use familiar_core::compose::{AttachmentData, DraftOp, DraftUpdateOp, Recipient, ResponseKind};
use familiar_core::events::EventOp;
use familiar_core::ops::{OpPayload, OpResult, finish_local};
use familiar_provider_gmail::{
    GmailApi, GmailSync, GoogleCalendarSync, Mailbox, Outgoing, Part, message_id, parts_of,
};
use familiar_provider_graph::{Error, Priority};
use familiar_store::{
    ALL_MAIL_REMOTE_ID, AccountId, Calendar, CalendarId, Event, EventId, Folder, FolderId,
    LOCAL_REMOTE_PREFIX, MessageId, NewAudit, Op, SharedWriter,
};
use tokio::sync::{Notify, watch};

use crate::ops::{Failure, OpSignals, classify, fail_op};
use crate::policy::{Clock, SyncPolicy};

/// Why a send later fails on Gmail.
pub const GMAIL_NO_SEND_LATER: &str = "Gmail can't hold a message to send later through its API; \
     send it now or keep the draft";

/// One Gmail account's op runner.
pub struct GmailOps {
    pub(crate) key: String,
    pub(crate) account: AccountId,
    pub(crate) api: GmailApi,
    pub(crate) sync: GmailSync,
    pub(crate) w: SharedWriter,
    pub(crate) policy: Arc<SyncPolicy>,
    pub(crate) clock: Clock,
    pub(crate) wake: Arc<Notify>,
    pub(crate) signals: Arc<OpSignals>,
    pub(crate) shutdown: watch::Receiver<bool>,
}

impl GmailOps {
    /// Runs until the daemon shuts down.
    pub async fn run(mut self) {
        loop {
            if *self.shutdown.borrow() {
                break;
            }
            let now = (self.clock)();
            let acc = self.account;
            let wait = match self.w.write(|tx| tx.claim_op_for(acc, now)) {
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
        let payload = match OpPayload::of(&op) {
            Ok(p) => p,
            Err(e) => {
                self.fail(&op, None, &e.message);
                return;
            }
        };
        match self.send(&op, &payload).await {
            Ok(result) => {
                let acc = self.account;
                let deletes_label = matches!(payload, OpPayload::DeleteFolder { .. });
                let r = self.w.write(|tx| {
                    // A deleted label's folder goes with the label sync
                    // below, which keeps its mail; deleting the folder
                    // here would delete the mail filed only there.
                    if !deletes_label {
                        finish_local(tx, &payload, acc, &result)?;
                    }
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
                    if matches!(
                        payload,
                        OpPayload::CreateEvent(_)
                            | OpPayload::UpdateEvent(_)
                            | OpPayload::RespondEvent(_)
                            | OpPayload::DeleteEvent(_)
                    ) {
                        let at = (self.clock)()
                            + self
                                .policy
                                .op_retry(op.attempts.max(1) as u32)
                                .as_secs()
                                .max(1) as i64;
                        if let Err(err) = self.w.write(|tx| tx.retry_op(op.id, &e.to_string(), at))
                        {
                            crate::log(&format!(
                                "{}: scheduling retry of op {}: {err}",
                                self.key, op.id.0
                            ));
                        }
                    }
                } else {
                    let expanded = match &payload {
                        OpPayload::CreateEvent(e) if e.fields.recurrence.is_some() => Some(e.event),
                        OpPayload::UpdateEvent(e) if e.series.is_some() => Some(e.event),
                        _ => None,
                    };
                    if let Some(id) = expanded {
                        if let Err(e) = self.refresh_calendar(id).await {
                            crate::log(&format!(
                                "{}: refreshing calendar after op {}: {e}",
                                self.key, op.id.0
                            ));
                        }
                    }
                }
                if deletes_label {
                    let mut w = self.w.clone();
                    if let Err(e) = self.sync.sync_labels(&mut w, Priority::User).await {
                        crate::log(&format!("{}: labels after a delete: {e}", self.key));
                    }
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
                Failure::Final(msg) => self.fail(&op, Some(&payload), &msg),
            },
        }
    }

    fn fail(&self, op: &Op, payload: Option<&OpPayload>, msg: &str) {
        fail_op(
            &self.w,
            &self.key,
            self.account,
            &self.signals,
            op,
            payload,
            msg,
        );
    }

    fn message_remote(&self, id: i64) -> Result<String, Error> {
        self.w
            .write(|tx| tx.message(MessageId(id)))?
            .map(|m| m.remote_id)
            .ok_or_else(|| Error::NotFound(format!("message {id} is no longer in the store")))
    }

    fn folders(&self) -> Result<Vec<Folder>, Error> {
        let acc = self.account;
        Ok(self.w.write(|tx| tx.folders(acc))?)
    }

    /// The label for a folder; `None` for All Mail, which has none.
    fn label(&self, all: &[Folder], id: i64) -> Result<Option<String>, Error> {
        let f = all
            .iter()
            .find(|f| f.id == FolderId(id))
            .ok_or_else(|| Error::NotFound(format!("folder {id} is no longer in the store")))?;
        if f.remote_id == ALL_MAIL_REMOTE_ID {
            return Ok(None);
        }
        if f.remote_id.starts_with(LOCAL_REMOTE_PREFIX) {
            return Err(Error::NotFound(format!(
                "label {:?} was never created on Gmail",
                f.name
            )));
        }
        Ok(Some(f.remote_id.clone()))
    }

    async fn move_message(&self, message: i64, from: i64, to: i64) -> Result<(), Error> {
        let pr = Priority::User;
        let all = self.folders()?;
        let id = self.message_remote(message)?;
        let from = self.label(&all, from)?;
        let to = self.label(&all, to)?;
        if to == from {
            return Ok(());
        }
        if to.as_deref() == Some("TRASH") {
            return self.api.trash(&id, pr).await;
        }
        let mut remove: Vec<&str> = Vec::new();
        if from.as_deref() == Some("TRASH") {
            self.api.untrash(&id, pr).await?;
            // Trash shows a message there only, so it may still carry SPAM.
            if to.as_deref() != Some("SPAM") {
                remove.push("SPAM");
            }
        } else if let Some(f) = from.as_deref().filter(|f| !matches!(*f, "SENT" | "DRAFT")) {
            remove.push(f);
        }
        let add: Vec<&str> = to.as_deref().into_iter().collect();
        if add.is_empty() && remove.is_empty() {
            return Ok(());
        }
        self.api.modify(&id, &add, &remove, pr).await
    }

    /// Names a folder's label, and those of the folders under it, by their
    /// paths in the local tree (already changed).
    async fn rename_tree(&self, folder: i64) -> Result<(), Error> {
        let all = self.folders()?;
        let mut todo = vec![FolderId(folder)];
        while let Some(f) = todo.pop() {
            // A sub-folder still waiting to be created gets its path then.
            let pending = all
                .iter()
                .any(|x| x.id == f && x.remote_id.starts_with(LOCAL_REMOTE_PREFIX));
            if pending {
                continue;
            }
            if let Some(label) = self.label(&all, f.0)? {
                self.api
                    .rename_label(&label, &path(&all, f), Priority::User)
                    .await?;
            }
            todo.extend(all.iter().filter(|c| c.parent == Some(f)).map(|c| c.id));
        }
        Ok(())
    }

    fn local_message(&self, id: i64) -> Result<familiar_store::Message, Error> {
        self.w
            .write(|tx| tx.message(MessageId(id)))?
            .ok_or_else(|| Error::NotFound(format!("message {id} is no longer in the store")))
    }

    /// The Gmail thread a message is in, if Gmail has it.
    fn thread_of(&self, id: i64) -> Result<Option<String>, Error> {
        Ok(self
            .w
            .write(|tx| tx.conversation_of(MessageId(id)))?
            .filter(|t| !t.starts_with(LOCAL_REMOTE_PREFIX)))
    }

    fn own_address(&self) -> Result<Mailbox, Error> {
        let acc = self.account;
        let a = self
            .w
            .write(|tx| tx.accounts())?
            .into_iter()
            .find(|a| a.id == acc)
            .ok_or_else(|| Error::NotFound(format!("account {}", acc.0)))?;
        Ok((a.display_name, a.email))
    }

    /// Saves a changed payload, so a retry sees it.
    fn save(&self, op: &Op, p: &OpPayload) {
        let r = serde_json::to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|json| {
                self.w
                    .write(|tx| tx.set_op_payload(op.id, &json))
                    .map_err(|e| e.to_string())
            });
        if let Err(e) = r {
            crate::log(&format!(
                "{}: op {}: can't save its payload: {e}",
                self.key, op.id.0
            ));
        }
    }

    /// Points the local draft at its new Gmail message.
    fn set_remote(&self, local: i64, remote: &str) {
        let r = self
            .w
            .write(|tx| tx.set_message_remote_id(MessageId(local), remote));
        if let Err(e) = r {
            crate::log(&format!(
                "{}: draft {local} is now {remote}, but: {e}",
                self.key
            ));
        }
    }

    async fn create_draft(&self, d: &DraftOp) -> Result<OpResult, Error> {
        let pr = Priority::User;
        let mut o = Outgoing {
            from: d.from.as_ref().map_or_else(
                || self.own_address(),
                |r| Ok((r.name.clone(), r.address.clone())),
            )?,
            to: mailboxes(&d.to),
            cc: mailboxes(&d.cc),
            bcc: mailboxes(&d.bcc),
            subject: d.subject.clone(),
            html: d.html.clone(),
            importance: d.importance.clone(),
            ..Outgoing::default()
        };
        let mut thread = None;
        if let Some((orig, kind)) = d.respond_to {
            let orig = self.local_message(orig)?;
            if kind == ResponseKind::Forward {
                o.parts = parts_of(&self.api.raw(&orig.remote_id, pr).await?);
            } else {
                // The quoted original may show inline images; carry those
                // (not its attachments) so they aren't broken in the reply.
                if o.html.contains("cid:") {
                    let html = &o.html;
                    o.parts = parts_of(&self.api.raw(&orig.remote_id, pr).await?)
                        .into_iter()
                        .filter(|p| {
                            p.inline
                                && p.content_id
                                    .as_ref()
                                    .is_some_and(|c| html.contains(&format!("cid:{c}")))
                        })
                        .collect();
                }
                if let Some(mid) = &orig.internet_message_id {
                    let mid = mid
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_string();
                    o.in_reply_to = Some(mid.clone());
                    o.references = vec![mid];
                }
                thread = self.thread_of(orig.id.0)?;
            }
        }
        o.parts.extend(d.attachments.iter().map(part_of));
        let r = self
            .api
            .create_draft(&o.build(), thread.as_deref(), pr)
            .await?;
        self.set_remote(d.message, &r.message);
        Ok(OpResult {
            message_remote_id: Some(r.message),
            ..OpResult::default()
        })
    }

    /// Gmail replaces a draft whole, so the update starts from the draft
    /// Gmail has and changes what the op changes.
    async fn update_draft(&self, u: &DraftUpdateOp) -> Result<OpResult, Error> {
        let pr = Priority::User;
        let m = self.local_message(u.message)?;
        if m.remote_id.starts_with(LOCAL_REMOTE_PREFIX) {
            return Err(Error::NotFound("the draft was never saved to Gmail".into()));
        }
        let mut o = Outgoing::from_raw(&self.api.raw(&m.remote_id, pr).await?);
        if let Some(f) = &u.from {
            o.from = (f.name.clone(), f.address.clone());
        }
        if let Some(s) = &u.subject {
            o.subject = s.clone();
        }
        if let Some(h) = &u.html {
            o.html = h.clone();
        }
        if let Some(v) = &u.to {
            o.to = mailboxes(v);
        }
        if let Some(v) = &u.cc {
            o.cc = mailboxes(v);
        }
        if let Some(v) = &u.bcc {
            o.bcc = mailboxes(v);
        }
        o.parts.retain(|p| !u.remove_attachments.contains(&p.name));
        // Inline images the body no longer shows go; ones it has stay once.
        let html = o.html.clone();
        o.parts.retain(|p| match (&p.content_id, p.inline) {
            (Some(cid), true) => html.contains(&format!("cid:{cid}")),
            _ => true,
        });
        for a in &u.attachments {
            let dup = a.inline
                && o.parts
                    .iter()
                    .any(|p| p.inline && p.content_id == a.content_id);
            if !dup {
                o.parts.push(part_of(a));
            }
        }
        let draft = self
            .api
            .find_draft(&m.remote_id, pr)
            .await?
            .ok_or_else(|| Error::NotFound("the draft is no longer in Gmail".into()))?;
        let r = self
            .api
            .update_draft(
                &draft,
                &o.build(),
                self.thread_of(u.message)?.as_deref(),
                pr,
            )
            .await?;
        self.set_remote(u.message, &r.message);
        Ok(OpResult {
            message_remote_id: Some(r.message),
            ..OpResult::default()
        })
    }

    async fn send_draft(&self, op: &Op, p: &OpPayload) -> Result<(), Error> {
        let OpPayload::SendDraft {
            message,
            internet_message_id,
            ..
        } = p
        else {
            unreachable!()
        };
        let message = *message;
        let mut internet_id = internet_message_id.clone();
        let pr = Priority::User;
        let m = self.local_message(message)?;
        let (_, own) = self.own_address()?;
        if !m.from.addr.eq_ignore_ascii_case(&own) {
            let ok = self
                .api
                .send_as(pr)
                .await?
                .iter()
                .any(|s| s.usable() && s.send_as_email.eq_ignore_ascii_case(&m.from.addr));
            if !ok {
                return Err(final_error(&format!(
                    "{} isn't a verified send-as address in Gmail, so Gmail would send \
                     from {own} instead; nothing was sent",
                    m.from.addr
                )));
            }
        }
        match self.api.find_draft(&m.remote_id, pr).await? {
            Some(d) => {
                // Keep the Message-ID first, so a retry after a lost answer
                // can find the sent copy.
                if internet_id.is_none() {
                    internet_id = message_id(&self.api.raw(&m.remote_id, pr).await?);
                    let mut saved = p.clone();
                    if let OpPayload::SendDraft {
                        internet_message_id,
                        ..
                    } = &mut saved
                    {
                        internet_message_id.clone_from(&internet_id);
                    }
                    self.save(op, &saved);
                }
                self.api.send_draft(&d, pr).await?;
                Ok(())
            }
            None => {
                // A retry after a send whose answer was lost: it went if
                // Gmail has it in Sent.
                if let Some(id) = &internet_id {
                    let id = id.trim_start_matches('<').trim_end_matches('>');
                    let q = format!("in:sent rfc822msgid:{id}");
                    if !self.api.search(&q, 1, pr).await?.is_empty() {
                        return Ok(());
                    }
                }
                Err(Error::NotFound("the draft is no longer in Gmail".into()))
            }
        }
    }

    fn calendar(&self, id: CalendarId) -> Result<Calendar, Error> {
        self.w
            .write(|tx| tx.calendars(self.account))?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| Error::NotFound(format!("calendar {}", id.0)))
    }

    fn event_row(&self, id: i64) -> Result<(Event, Calendar), Error> {
        let row = self
            .w
            .write(|tx| tx.event(EventId(id)))?
            .filter(|e| e.data.account == self.account)
            .ok_or_else(|| Error::NotFound(format!("event {id}")))?;
        let cal = self.calendar(row.data.calendar)?;
        if !cal.can_edit {
            return Err(final_error("this Google calendar is read-only"));
        }
        Ok((row, cal))
    }

    async fn refresh_calendar(&self, id: i64) -> Result<(), Error> {
        let (_, cal) = self.event_row(id)?;
        let now = (self.clock)();
        let window = (
            cal.window_start.unwrap_or(now - 30 * 86400),
            cal.window_end.unwrap_or(now + 365 * 86400),
        );
        GoogleCalendarSync::new(self.api.clone(), self.account)
            .sync_events(&mut self.w.clone(), &cal.remote_id, window, Priority::User)
            .await?;
        Ok(())
    }

    async fn create_event(&self, e: &EventOp) -> Result<(), Error> {
        let (row, cal) = self.event_row(e.event)?;
        if !row.data.remote_id.starts_with(LOCAL_REMOTE_PREFIX) {
            // The server ID was saved before an interruption completing this op.
            return Ok(());
        }
        let source = if let Some(id) = e.from_mail {
            let msg = self.local_message(id)?;
            let escape = familiar_core::compose::text_to_html;
            Some(format!(
                "<p>From the email <a href=\"https://mail.google.com/mail/#all/{}\">{}</a> from {}.</p>",
                escape(&msg.remote_id),
                escape(&msg.subject),
                escape(&msg.from.addr)
            ))
        } else {
            None
        };
        let created = self
            .api
            .insert_event(&cal.remote_id, e, source.as_deref())
            .await?;
        let remote = created["id"]
            .as_str()
            .ok_or_else(|| Error::Decode("created event has no id".into()))?;
        // Keep optimistic changes from later operations, updating only identity.
        self.w.write(|tx| {
            let mut current = tx
                .event(row.id)?
                .ok_or_else(|| familiar_store::Error::NotFound(format!("event {}", row.id.0)))?
                .data;
            current.remote_id = remote.to_owned();
            current.ical_uid = created["iCalUID"].as_str().map(str::to_owned);
            current.web_link = created["htmlLink"].as_str().map(str::to_owned);
            current.change_key = created["etag"].as_str().map(str::to_owned);
            current.series_master_id = created["recurringEventId"].as_str().map(str::to_owned);
            if current.series_master_id.is_some() {
                current.kind = "occurrence".into();
            }
            tx.replace_event(row.id, &current)
        })?;
        Ok(())
    }

    async fn send(&self, op: &Op, p: &OpPayload) -> Result<OpResult, Error> {
        let pr = Priority::User;
        match p {
            OpPayload::SetRead { message, read, .. } => {
                let id = self.message_remote(*message)?;
                if *read {
                    self.api.modify(&id, &[], &["UNREAD"], pr).await?;
                } else {
                    self.api.modify(&id, &["UNREAD"], &[], pr).await?;
                }
            }
            OpPayload::SetFlag {
                message, flagged, ..
            } => {
                let id = self.message_remote(*message)?;
                if *flagged {
                    self.api.modify(&id, &["STARRED"], &[], pr).await?;
                } else {
                    self.api.modify(&id, &[], &["STARRED"], pr).await?;
                }
            }
            OpPayload::MoveMessage {
                message, from, to, ..
            }
            | OpPayload::ReportMessage {
                message, from, to, ..
            } => self.move_message(*message, *from, *to).await?,
            OpPayload::CreateFolder { folder, .. } => {
                let all = self.folders()?;
                let name = path(&all, FolderId(*folder));
                let id = self.api.create_label(&name, pr).await?;
                return Ok(OpResult {
                    folder_remote_id: Some(id),
                    ..OpResult::default()
                });
            }
            OpPayload::RenameFolder { folder, .. } | OpPayload::MoveFolder { folder, .. } => {
                self.rename_tree(*folder).await?
            }
            OpPayload::DeleteFolder { folder, .. } => {
                let all = self.folders()?;
                // Deepest first, so a retry finds what's left.
                let mut order = vec![FolderId(*folder)];
                let mut i = 0;
                while i < order.len() {
                    let f = order[i];
                    order.extend(all.iter().filter(|c| c.parent == Some(f)).map(|c| c.id));
                    i += 1;
                }
                for f in order.into_iter().rev() {
                    if let Some(label) = self.label(&all, f.0)? {
                        self.api.delete_label(&label, pr).await?;
                    }
                }
            }
            OpPayload::DeleteMessage { .. } => {
                return Err(final_error(
                    "Familiar can't delete Gmail mail for good; Gmail empties Trash itself",
                ));
            }
            OpPayload::CreateDraft(d) => return self.create_draft(d).await,
            OpPayload::UpdateDraft(u) => return self.update_draft(u).await,
            OpPayload::SendDraft { send_at, .. } => {
                if send_at.is_some() {
                    return Err(final_error(GMAIL_NO_SEND_LATER));
                }
                self.send_draft(op, p).await?
            }
            OpPayload::CreateEvent(e) => self.create_event(e).await?,
            OpPayload::UpdateEvent(u) => {
                let (row, cal) = self.event_row(u.event)?;
                let id = u.series.as_deref().unwrap_or(&row.data.remote_id);
                // A later queued edit can already be visible in row.data.
                let before = u
                    .prev
                    .iter()
                    .find(|s| s.id == u.event)
                    .map(|s| serde_json::from_value(s.data.clone()))
                    .transpose()?
                    .unwrap_or(row.data.clone());
                self.api
                    .edit_event(&cal.remote_id, id, &u.changes, &before)
                    .await?;
            }
            OpPayload::RespondEvent(r) => {
                let (row, cal) = self.event_row(r.event)?;
                let id = r.series.as_deref().unwrap_or(&row.data.remote_id);
                self.api
                    .rsvp_event(
                        &cal.remote_id,
                        id,
                        r.response,
                        r.comment.as_deref(),
                        r.send_response,
                    )
                    .await?;
            }
            OpPayload::DeleteEvent(d) => {
                if d.comment.as_deref().is_some_and(|c| !c.is_empty()) {
                    return Err(final_error(
                        "Google Calendar cannot send a cancellation note; delete without a comment",
                    ));
                }
                let (row, cal) = self.event_row(d.event)?;
                let id = d.series.as_deref().unwrap_or(&row.data.remote_id);
                self.api.remove_event(&cal.remote_id, id).await?;
            }
        }
        Ok(OpResult::default())
    }
}

fn mailboxes(v: &[Recipient]) -> Vec<Mailbox> {
    v.iter()
        .map(|r| (r.name.clone(), r.address.clone()))
        .collect()
}

fn part_of(a: &AttachmentData) -> Part {
    Part {
        name: a.name.clone(),
        content_type: a.content_type.clone(),
        inline: a.inline,
        content_id: a.content_id.clone(),
        data: base64::engine::general_purpose::STANDARD
            .decode(&a.content_b64)
            .unwrap_or_default(),
    }
}

/// An error the op runner treats as final.
fn final_error(msg: &str) -> Error {
    Error::Graph {
        status: 400,
        code: "unsupported".into(),
        message: msg.into(),
        request_id: None,
    }
}

/// A folder's label name: its path from the top, by `/`. Built-in folders
/// (Inbox and the like) aren't part of a label's name.
fn path(all: &[Folder], id: FolderId) -> String {
    let mut parts = Vec::new();
    let mut at = all.iter().find(|f| f.id == id);
    while let Some(f) = at {
        if f.well_known.is_some() {
            break;
        }
        parts.push(f.name.as_str());
        at = f.parent.and_then(|p| all.iter().find(|x| x.id == p));
    }
    parts.reverse();
    parts.join("/")
}
