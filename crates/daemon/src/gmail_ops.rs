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
//! Gmail drafts and sending are WP G.7; until then those ops fail with their
//! local change undone. Gmail can't delete mail for good with Familiar's
//! scope, so that is refused when planned.

use std::sync::Arc;
use std::time::Duration;

use familiar_core::ops::{OpPayload, OpResult, finish_local};
use familiar_provider_gmail::{GmailApi, GmailSync};
use familiar_provider_graph::{Error, Priority};
use familiar_store::{
    ALL_MAIL_REMOTE_ID, AccountId, Folder, FolderId, LOCAL_REMOTE_PREFIX, MessageId, NewAudit, Op,
    SharedWriter,
};
use tokio::sync::{Notify, watch};

use crate::ops::{Failure, OpSignals, classify, fail_op};
use crate::policy::{Clock, SyncPolicy};

/// Why a draft or send op fails for now.
pub const GMAIL_NO_COMPOSE: &str =
    "Gmail drafts and sending come in a later update of Familiar; nothing was sent";

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
        match self.send(&payload).await {
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
        } else if let Some(f) = from.as_deref() {
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

    async fn send(&self, p: &OpPayload) -> Result<OpResult, Error> {
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
            OpPayload::CreateDraft(_) | OpPayload::UpdateDraft(_) | OpPayload::SendDraft { .. } => {
                return Err(final_error(GMAIL_NO_COMPOSE));
            }
            OpPayload::CreateEvent(_)
            | OpPayload::UpdateEvent(_)
            | OpPayload::RespondEvent(_)
            | OpPayload::DeleteEvent(_) => {
                return Err(final_error(
                    "Gmail accounts have no calendar in Familiar yet",
                ));
            }
        }
        Ok(OpResult::default())
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
