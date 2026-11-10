// SPDX-License-Identifier: MIT OR Apache-2.0
//! Brings snoozed messages back when their time comes.
//!
//! Every [`SyncPolicy::snooze_interval`](crate::policy::SyncPolicy) (and once
//! at start, for snoozes that came due while famd wasn't running) this task
//! asks the [`Service`] to move due messages out of Snoozed, back to where
//! they were, marked unread. The moves go through the op queue like any
//! other write.

use std::sync::Arc;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::service::Service;

/// Starts the task.
pub fn spawn(service: Arc<Service>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let every = service.policy.snooze_interval;
        loop {
            service.return_due_snoozes();
            tokio::select! {
                _ = tokio::time::sleep(every) => {}
                _ = shutdown.changed() => return,
            }
            if *shutdown.borrow() {
                return;
            }
        }
    })
}
