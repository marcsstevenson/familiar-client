// SPDX-License-Identifier: MIT OR Apache-2.0
//! The throttling scheduler: one per mailbox.
//!
//! Exchange allows four concurrent requests per mailbox per app, and answers
//! more with 429 `MailboxConcurrency`. Every Graph request for a mailbox takes
//! a [`Permit`] from that mailbox's [`Scheduler`] first, so at most
//! [`Scheduler::limit`] are ever in flight.
//!
//! - **User actions first.** Waiters are queued by [`Priority`]. When a slot
//!   frees up it goes to the oldest [`Priority::User`] waiter if there is one,
//!   and only then to background sync. A running request is never cancelled.
//! - **`Retry-After`.** When Graph throttles (429) or is unavailable (503), the
//!   client calls [`Scheduler::pause_for`]. No new request for the mailbox
//!   starts until the pause ends, whoever asks; requests already in flight
//!   finish. Queued work then resumes in priority order.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::Instant;

/// Exchange's per-mailbox concurrency limit for one app.
pub const MAILBOX_CONCURRENCY: usize = 4;

/// Who is waiting for the request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Priority {
    /// Someone is waiting on screen or at the prompt: opening a message,
    /// fetching an older body, a CLI read.
    User,
    /// Sync, prefetch and anything else nobody is watching.
    Background,
}

impl Priority {
    fn index(self) -> usize {
        match self {
            Priority::User => 0,
            Priority::Background => 1,
        }
    }
}

/// Counters for tests and `fam sync status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SchedulerStats {
    /// Requests running now.
    pub in_flight: usize,
    /// The most that were ever running at once.
    pub peak_in_flight: usize,
    /// Waiters queued now, user then background.
    pub queued: [usize; 2],
    /// Permits handed out so far.
    pub granted: u64,
    /// How many times Graph asked us to back off.
    pub pauses: u64,
}

struct Inner {
    limit: usize,
    in_flight: usize,
    paused_until: Option<Instant>,
    queues: [VecDeque<oneshot::Sender<Permit>>; 2],
    stats: SchedulerStats,
}

/// Admits requests for one mailbox. Cheap to clone; clones share state.
#[derive(Clone)]
pub struct Scheduler {
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("stats", &self.stats())
            .finish()
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new(MAILBOX_CONCURRENCY)
    }
}

impl Scheduler {
    /// A scheduler that lets `limit` requests run at once (at least one).
    pub fn new(limit: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                limit: limit.max(1),
                in_flight: 0,
                paused_until: None,
                queues: [VecDeque::new(), VecDeque::new()],
                stats: SchedulerStats::default(),
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// How many requests may run at once.
    pub fn limit(&self) -> usize {
        self.lock().limit
    }

    /// Current counters.
    pub fn stats(&self) -> SchedulerStats {
        let g = self.lock();
        let mut s = g.stats;
        s.in_flight = g.in_flight;
        s.queued = [g.queues[0].len(), g.queues[1].len()];
        s
    }

    /// Waits for a slot. Hold the permit for the whole request.
    pub async fn acquire(&self, priority: Priority) -> Permit {
        let rx = {
            let mut g = self.lock();
            let paused = g.paused_until.is_some_and(|t| t > Instant::now());
            // Don't jump the queue: earlier waiters of this priority or higher go first.
            let ahead = g.queues[..=priority.index()].iter().any(|q| !q.is_empty());
            if !paused && !ahead && g.in_flight < g.limit {
                return self.grant(&mut g);
            }
            let (tx, rx) = oneshot::channel();
            g.queues[priority.index()].push_back(tx);
            rx
        };
        match rx.await {
            Ok(p) => p,
            // The scheduler is never dropped while a waiter holds a clone, so
            // the sender can't go away; take a slot directly if it ever does.
            Err(_) => {
                let mut g = self.lock();
                self.grant(&mut g)
            }
        }
    }

    fn grant(&self, g: &mut Inner) -> Permit {
        g.in_flight += 1;
        g.stats.granted += 1;
        g.stats.peak_in_flight = g.stats.peak_in_flight.max(g.in_flight);
        Permit {
            scheduler: Some(self.clone()),
        }
    }

    /// Stops new requests from starting for `wait` (Graph's `Retry-After`).
    /// A longer pause already in place is kept.
    pub fn pause_for(&self, wait: Duration) {
        let until = Instant::now() + wait;
        {
            let mut g = self.lock();
            g.stats.pauses += 1;
            if g.paused_until.is_some_and(|t| t >= until) {
                return;
            }
            g.paused_until = Some(until);
        }
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep_until(until).await;
            let mut g = me.lock();
            if g.paused_until.is_some_and(|t| t <= Instant::now()) {
                g.paused_until = None;
            }
            me.dispatch(&mut g);
        });
    }

    /// When the current pause ends, if there is one.
    pub fn paused_until(&self) -> Option<Instant> {
        self.lock().paused_until.filter(|t| *t > Instant::now())
    }

    /// Hands free slots to waiters, user first.
    fn dispatch(&self, g: &mut Inner) {
        if g.paused_until.is_some_and(|t| t > Instant::now()) {
            return;
        }
        while g.in_flight < g.limit {
            let Some(tx) = g.queues[0].pop_front().or_else(|| g.queues[1].pop_front()) else {
                return;
            };
            let permit = self.grant(g);
            if let Err(mut p) = tx.send(permit) {
                // The waiter gave up. Undo without re-entering the lock.
                p.scheduler = None;
                g.in_flight -= 1;
                g.stats.granted -= 1;
            }
        }
    }

    fn release(&self) {
        let mut g = self.lock();
        g.in_flight = g.in_flight.saturating_sub(1);
        self.dispatch(&mut g);
    }
}

/// A slot for one request. Dropping it frees the slot.
#[must_use = "the slot is released as soon as the permit is dropped"]
pub struct Permit {
    scheduler: Option<Scheduler>,
}

impl std::fmt::Debug for Permit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Permit")
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(s) = self.scheduler.take() {
            s.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;

    #[tokio::test]
    async fn never_more_than_the_limit() {
        let s = Scheduler::new(4);
        let mut tasks = Vec::new();
        for _ in 0..40 {
            let s = s.clone();
            tasks.push(tokio::spawn(async move {
                let _p = s.acquire(Priority::Background).await;
                assert!(s.stats().in_flight <= 4);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let st = s.stats();
        assert_eq!(st.peak_in_flight, 4);
        assert_eq!(st.in_flight, 0);
        assert_eq!(st.granted, 40);
    }

    #[tokio::test]
    async fn user_waiters_go_before_background() {
        let s = Scheduler::new(1);
        let order = Arc::new(AsyncMutex::new(Vec::new()));
        let held = s.acquire(Priority::Background).await;
        let mut tasks = Vec::new();
        for (i, p) in [
            Priority::Background,
            Priority::Background,
            Priority::User,
            Priority::Background,
            Priority::User,
        ]
        .into_iter()
        .enumerate()
        {
            let s = s.clone();
            let order = order.clone();
            tasks.push(tokio::spawn(async move {
                let _p = s.acquire(p).await;
                order.lock().await.push(i);
            }));
            // Let each task queue up in turn.
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(s.stats().queued, [2, 3]);
        drop(held);
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(*order.lock().await, [2, 4, 0, 1, 3]);
    }

    #[tokio::test]
    async fn pause_holds_new_requests_until_retry_after() {
        let s = Scheduler::new(4);
        s.pause_for(Duration::from_millis(200));
        let start = Instant::now();
        let _p = s.acquire(Priority::User).await;
        assert!(start.elapsed() >= Duration::from_millis(190));
        // A shorter pause doesn't cut a longer one short.
        s.pause_for(Duration::from_millis(300));
        s.pause_for(Duration::from_millis(10));
        let start = Instant::now();
        let _q = s.acquire(Priority::User).await;
        assert!(start.elapsed() >= Duration::from_millis(290));
        assert_eq!(s.stats().pauses, 3);
    }

    #[tokio::test]
    async fn abandoned_waiters_do_not_leak_slots() {
        let s = Scheduler::new(1);
        let held = s.acquire(Priority::Background).await;
        let s2 = s.clone();
        let waiter = tokio::spawn(async move {
            let _p = s2.acquire(Priority::Background).await;
        });
        tokio::time::sleep(Duration::from_millis(5)).await;
        waiter.abort();
        let _ = waiter.await;
        drop(held);
        let _p = tokio::time::timeout(Duration::from_secs(1), s.acquire(Priority::User))
            .await
            .expect("slot was leaked");
        assert_eq!(s.stats().in_flight, 1);
    }
}
