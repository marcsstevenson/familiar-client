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

use familiar_core::ops::{OpPayload, OpResult, finish_local, undo_local};
use familiar_provider_graph::{Error as GraphError, MailApi, Priority};
use familiar_store::{
    AccountId, FolderId, LOCAL_REMOTE_PREFIX, MessageId, NewAudit, Op, OpState, SharedWriter,
};
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
        let payload = match OpPayload::of(&op) {
            Ok(p) => p,
            Err(e) => {
                self.fail(&op, None, &e.message).await;
                return;
            }
        };
        match self.send(&payload).await {
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

    async fn send(&self, p: &OpPayload) -> Result<OpResult, GraphError> {
        let pr = Priority::User;
        let api = &self.api;
        match p {
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
