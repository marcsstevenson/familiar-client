// SPDX-License-Identifier: MIT OR Apache-2.0
//! The sync engine: one task per account deciding what to sync when.
//!
//! Each step does one piece of work and returns, so the loop can re-check
//! what is due between pieces:
//!
//! 1. **Folder tree**, at start and every `tree_interval` (5 min).
//! 2. **Folders that are due.** On the first sync every folder is due; they
//!    run one at a time, Inbox first (with one page of its newest headers
//!    stored before its delta round, so new mail shows at once), then Sent
//!    Items, other folders, and Deleted Items and Junk last. Each folder's
//!    delta round lists all history without bodies. Once every folder has
//!    finished a round the first sync is recorded in the store
//!    ([`FIRST_SYNC_RESOURCE`]). After that, due folders are polled together
//!    in one call (so a move between them settles locally): Inbox every
//!    `inbox_interval` (30 s), the rest every `folder_interval` (5 min).
//! 3. **Bodies for the window**, newest first, `body_batch` at a time, after
//!    any round that brought new or changed mail.
//! 4. **Ageing**, at start and every `ageing_interval` (1 h): bodies that
//!    have left the window are dropped unless pinned or cached, headers
//!    older than `sync.headers` are pruned, the on-demand cache is trimmed
//!    to `sync.cache_limit` and unreferenced body files are deleted.
//!
//! **Calendars** run between steps 1 and 2, at start and every
//! `calendar_interval` (5 min), so a long first mail sync doesn't hold them
//! up: the calendar list, then each calendar's events by calendarView delta
//! for the window around today (`[calendar] past` and `future`). A calendar
//! that fails (a shared calendar that refuses delta, say) records the error
//! on its `events:<id>` sync state and the others carry on.
//!
//! A failed step is recorded on the folder's sync state and in the live
//! status, and the account waits `backoff_min` doubling to `backoff_max`
//! before trying again. Throttling never gets this far: the provider's
//! scheduler waits out `Retry-After` itself.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use familiar_core::daemon::{AccountLive, FIRST_SYNC_RESOURCE, SyncPhase};
use familiar_provider_graph::{
    CalendarSync, ContactSync, Error as GraphError, MailSync, Priority, events_resource,
    messages_resource,
};
use familiar_store::{AccountId, Cursor, Folder, Reader, SharedWriter};
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

use crate::policy::{Clock, SyncPolicy};

/// True for an error that is about the account or the connection (offline,
/// throttled, signed out, Graph itself in trouble), not one folder or message.
/// Those back the whole account off; the rest only skip the item.
fn account_wide(e: &GraphError) -> bool {
    match e {
        GraphError::Network(_)
        | GraphError::Throttled { .. }
        | GraphError::Auth(_)
        | GraphError::Store(_) => true,
        GraphError::Graph { status, .. } => *status >= 500 || *status == 408,
        GraphError::NotFound(_) | GraphError::Decode(_) | GraphError::SyncStateGone(_) => false,
    }
}

/// How long a done op stays in the queue before ageing deletes it.
const DONE_OP_RETENTION_SECS: i64 = 7 * 86_400;

/// Sync state row for an account's contacts.
pub const CONTACTS_RESOURCE: &str = "contacts";

/// Shared between an account's engine and the request handlers.
#[derive(Debug)]
pub struct AccountShared {
    /// What `daemon.status` reports.
    pub live: Mutex<AccountLive>,
    /// Wakes the engine early.
    pub wake: Notify,
    /// Set by `sync.now`: everything is due.
    pub force: AtomicBool,
    /// `sync.now` requests so far; each one's number is its ticket.
    requested: AtomicU64,
    /// The highest ticket whose round has finished and gone idle.
    served: AtomicU64,
}

impl AccountShared {
    /// A fresh status for `key`.
    pub fn new(key: &str) -> Self {
        Self {
            live: Mutex::new(AccountLive {
                account: key.to_string(),
                phase: SyncPhase::Starting,
                detail: None,
                last_round_at: None,
                next_poll_at: None,
                headers_synced: 0,
                bodies_fetched: 0,
                failures: 0,
            }),
            wake: Notify::new(),
            force: AtomicBool::new(false),
            requested: AtomicU64::new(0),
            served: AtomicU64::new(0),
        }
    }

    /// Changes the live status.
    pub fn update(&self, f: impl FnOnce(&mut AccountLive)) {
        f(&mut self.live.lock().unwrap_or_else(|p| p.into_inner()));
    }

    /// A copy of the live status.
    pub fn snapshot(&self) -> AccountLive {
        self.live.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Makes every folder due and wakes the engine. Returns a ticket for
    /// [`AccountShared::served`].
    pub fn sync_now(&self) -> u64 {
        let ticket = self.requested.fetch_add(1, Ordering::SeqCst) + 1;
        self.force.store(true, Ordering::SeqCst);
        self.wake.notify_one();
        ticket
    }

    /// Whether the engine has gone idle after a round that started after
    /// `sync_now` returned `ticket` (everything on the server up to that
    /// request is in the store, bodies included), or that round failed.
    pub fn served(&self, ticket: u64) -> bool {
        self.served.load(Ordering::SeqCst) >= ticket
    }
}

/// Order in which a first sync visits folders: lower first.
fn first_sync_rank(f: &Folder) -> u8 {
    match f.well_known.as_deref() {
        Some("inbox") => 0,
        Some("sentitems") => 1,
        Some("drafts") => 2,
        Some("archive") => 3,
        Some("deleteditems") => 8,
        Some("junkemail") => 9,
        _ => 5,
    }
}

/// What a step decided.
enum Next {
    /// More work is ready; step again now.
    Now,
    /// Nothing to do until this instant.
    At(Instant),
}

struct FolderPlan {
    remote: String,
    is_inbox: bool,
    rank: u8,
    due: Instant,
    /// A delta round has finished for it (this run or before).
    synced: bool,
}

/// One account's sync loop.
pub struct Engine {
    key: String,
    account: AccountId,
    sync: MailSync,
    calendar: CalendarSync,
    contacts: ContactSync,
    w: SharedWriter,
    policy: Arc<SyncPolicy>,
    clock: Clock,
    shared: Arc<AccountShared>,
    shutdown: watch::Receiver<bool>,

    tree_due: Option<Instant>,
    folders: HashMap<String, FolderPlan>,
    first_sync_done: bool,
    bodies_pending: bool,
    body_cursor: Option<Cursor>,
    ageing_due: Instant,
    calendar_due: Instant,
    contacts_due: Instant,
    failures: u32,
    /// The `sync.now` ticket the current work answers.
    serving: u64,
}

impl Engine {
    /// An engine for `account` (already in the store) syncing through `sync`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: String,
        account: AccountId,
        sync: MailSync,
        calendar: CalendarSync,
        contacts: ContactSync,
        w: SharedWriter,
        policy: Arc<SyncPolicy>,
        clock: Clock,
        shared: Arc<AccountShared>,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            key,
            account,
            sync,
            calendar,
            contacts,
            w,
            policy,
            clock,
            shared,
            shutdown,
            tree_due: None,
            folders: HashMap::new(),
            first_sync_done: false,
            bodies_pending: true,
            body_cursor: None,
            ageing_due: Instant::now(),
            calendar_due: Instant::now(),
            contacts_due: Instant::now(),
            failures: 0,
            serving: 0,
        }
    }

    fn reader(&self) -> familiar_store::Result<Reader> {
        Reader::open(&self.w.paths().root)
    }

    fn phase(&self, phase: SyncPhase) {
        self.shared.update(|l| l.phase = phase);
    }

    /// Runs until the daemon shuts down.
    pub async fn run(mut self) {
        if let Ok(r) = self.reader() {
            self.first_sync_done = r
                .sync_state(self.account, FIRST_SYNC_RESOURCE)
                .ok()
                .flatten()
                .is_some_and(|s| s.synced_at.is_some());
        }
        crate::log(&format!(
            "{}: starting ({})",
            self.key,
            if self.first_sync_done {
                "incremental"
            } else {
                "first sync"
            }
        ));
        loop {
            if *self.shutdown.borrow() {
                break;
            }
            let seen = self.shared.requested.load(Ordering::SeqCst);
            if self.shared.force.swap(false, Ordering::SeqCst) {
                self.serving = seen;
                let now = Instant::now();
                self.tree_due = Some(now);
                self.calendar_due = now;
                self.contacts_due = now;
                for f in self.folders.values_mut() {
                    f.due = now;
                }
            }
            let wait_until = match self.step().await {
                Ok(Next::Now) => {
                    self.failures = 0;
                    continue;
                }
                Ok(Next::At(t)) => {
                    self.failures = 0;
                    t
                }
                Err(e) => {
                    self.failures += 1;
                    let auth = matches!(e, GraphError::Auth(_));
                    let wait = if auth {
                        self.policy.backoff_max.max(Duration::from_secs(300))
                    } else {
                        self.policy.backoff(self.failures)
                    };
                    let msg = e.to_string();
                    crate::log(&format!(
                        "{}: {msg}; retrying in {}s",
                        self.key,
                        wait.as_secs()
                    ));
                    let at = (self.clock)() + wait.as_secs() as i64;
                    self.shared.update(|l| {
                        l.phase = if auth {
                            SyncPhase::AuthRequired
                        } else {
                            SyncPhase::Backoff
                        };
                        l.detail = Some(msg);
                        l.failures += 1;
                        l.next_poll_at = Some(at);
                    });
                    // The round was tried: a check waiting on it goes ahead
                    // rather than waiting out the backoff.
                    self.shared.served.store(self.serving, Ordering::SeqCst);
                    Instant::now() + wait
                }
            };
            let mut shutdown = self.shutdown.clone();
            tokio::select! {
                _ = tokio::time::sleep_until(wait_until) => {}
                _ = self.shared.wake.notified() => {}
                _ = shutdown.changed() => {}
            }
        }
        crate::log(&format!("{}: stopped", self.key));
    }

    async fn step(&mut self) -> Result<Next, GraphError> {
        let now = Instant::now();

        // 1. Folder tree.
        if self.tree_due.is_none_or(|t| t <= now) {
            if self.tree_due.is_none() {
                self.phase(SyncPhase::FolderTree);
            }
            self.sync
                .sync_folder_tree(&mut self.w, Priority::Background)
                .await?;
            self.tree_due = Some(Instant::now() + self.policy.tree_interval);
            self.plan_folders()?;
            return Ok(Next::Now);
        }

        // Calendars, between the tree and the folders.
        if self.calendar_due <= now {
            self.sync_calendars().await?;
            self.calendar_due = Instant::now() + self.policy.calendar_interval;
            return Ok(Next::Now);
        }
        if self.contacts_due <= now {
            self.sync_contacts().await?;
            self.contacts_due = Instant::now() + self.policy.contacts_interval;
            return Ok(Next::Now);
        }

        // 2. Due folders.
        let mut due: Vec<&FolderPlan> = self.folders.values().filter(|f| f.due <= now).collect();
        if !due.is_empty() {
            due.sort_by(|a, b| a.rank.cmp(&b.rank).then(a.remote.cmp(&b.remote)));
            let batch: Vec<String> = if self.first_sync_done {
                due.iter().map(|f| f.remote.clone()).collect()
            } else {
                self.phase(SyncPhase::Headers);
                vec![due[0].remote.clone()]
            };
            self.sync_round(&batch).await?;
            return Ok(Next::Now);
        }
        if !self.first_sync_done && self.folders.values().all(|f| f.synced) {
            let at = (self.clock)();
            let acc = self.account;
            self.w
                .write(|tx| tx.set_sync_state(acc, FIRST_SYNC_RESOURCE, None, None, at))?;
            self.first_sync_done = true;
            crate::log(&format!("{}: headers for all history are in", self.key));
        }

        // 3. Bodies for the window, newest first.
        if self.bodies_pending {
            if self.fetch_body_batch().await? {
                return Ok(Next::Now);
            }
            self.bodies_pending = false;
            self.body_cursor = None;
        }

        // 4. Ageing.
        if self.ageing_due <= now {
            self.age()?;
            self.ageing_due = Instant::now() + self.policy.ageing_interval;
        }

        // Idle until the next thing is due.
        let mut next = self
            .ageing_due
            .min(self.calendar_due)
            .min(self.contacts_due);
        if let Some(t) = self.tree_due {
            next = next.min(t);
        }
        for f in self.folders.values() {
            next = next.min(f.due);
        }
        let at = (self.clock)() + next.saturating_duration_since(Instant::now()).as_secs() as i64;
        self.shared.update(|l| {
            l.phase = SyncPhase::Idle;
            l.detail = None;
            l.next_poll_at = Some(at);
        });
        self.shared.served.store(self.serving, Ordering::SeqCst);
        Ok(Next::At(next))
    }

    /// Brings the folder plan in line with the stored folder tree.
    fn plan_folders(&mut self) -> Result<(), GraphError> {
        let r = self.reader()?;
        let local = r.folders(self.account)?;
        let now = Instant::now();
        let mut keep: HashMap<String, FolderPlan> = HashMap::new();
        for f in &local {
            let plan = match self.folders.remove(&f.remote_id) {
                Some(p) => p,
                None => {
                    let state = r.sync_state(self.account, &messages_resource(&f.remote_id))?;
                    // A delta link (not a nextLink mid-round) means a round finished.
                    let synced = state
                        .and_then(|s| s.delta_link)
                        .is_some_and(|l| l.contains("deltatoken"));
                    FolderPlan {
                        remote: f.remote_id.clone(),
                        is_inbox: f.well_known.as_deref() == Some("inbox"),
                        rank: first_sync_rank(f),
                        due: now,
                        synced,
                    }
                }
            };
            keep.insert(f.remote_id.clone(), plan);
        }
        self.folders = keep;
        Ok(())
    }

    async fn sync_round(&mut self, batch: &[String]) -> Result<(), GraphError> {
        let acc = self.account;
        // Show the newest Inbox mail before its first (possibly long) delta round.
        for remote in batch {
            let plan = &self.folders[remote];
            if plan.is_inbox && !plan.synced && !self.first_sync_done {
                let started = self
                    .reader()?
                    .sync_state(acc, &messages_resource(remote))?
                    .is_some();
                if !started {
                    let (n, _) = self
                        .sync
                        .store_headers_page(&mut self.w, remote, 50, None, Priority::Background)
                        .await?;
                    self.shared.update(|l| l.headers_synced += n as u64);
                }
            }
        }
        let refs: Vec<&str> = batch.iter().map(String::as_str).collect();
        let (upserted, moved) = match self
            .sync
            .sync_folders(&mut self.w, &refs, Priority::Background)
            .await
        {
            Ok(r) => (r.upserted, r.moved),
            Err(e) if account_wide(&e) => {
                self.folder_error(batch, &e);
                return Err(e);
            }
            Err(e) if batch.len() == 1 => {
                // This folder refuses; the others carry on. It is tried
                // again when it is next due.
                self.folder_error(batch, &e);
                (0, 0)
            }
            Err(_) => {
                // One of them refuses. Run them one at a time so it doesn't
                // hold up the rest; folders already done just confirm their
                // bookmark.
                let (mut upserted, mut moved) = (0, 0);
                for remote in batch {
                    match self
                        .sync
                        .sync_folders(&mut self.w, &[remote.as_str()], Priority::Background)
                        .await
                    {
                        Ok(r) => {
                            upserted += r.upserted;
                            moved += r.moved;
                        }
                        Err(e) => {
                            self.folder_error(std::slice::from_ref(remote), &e);
                            if account_wide(&e) {
                                return Err(e);
                            }
                        }
                    }
                }
                (upserted, moved)
            }
        };
        let now = Instant::now();
        for remote in batch {
            if let Some(p) = self.folders.get_mut(remote) {
                p.synced = true;
                p.due = now
                    + if p.is_inbox {
                        self.policy.inbox_interval
                    } else {
                        self.policy.folder_interval
                    };
            }
        }
        if upserted + moved > 0 {
            self.bodies_pending = true;
            self.body_cursor = None;
        }
        if let Some(before) = self.policy.headers_start((self.clock)()) {
            self.w.write(|tx| tx.prune_headers(acc, before))?;
        }
        let at = (self.clock)();
        self.shared.update(|l| {
            l.headers_synced += upserted as u64;
            l.last_round_at = Some(at);
            l.detail = None;
        });
        Ok(())
    }

    /// Records a failed round on the folders' sync states and in the log.
    fn folder_error(&mut self, folders: &[String], e: &GraphError) {
        let acc = self.account;
        let msg = e.to_string();
        let _ = self.w.write(|tx| {
            for remote in folders {
                tx.set_sync_error(acc, &messages_resource(remote), &msg)?;
            }
            Ok(())
        });
        if !account_wide(e) {
            crate::log(&format!(
                "{}: folder {} skipped this round: {msg}",
                self.key,
                folders.join(", ")
            ));
        }
        if matches!(e, GraphError::NotFound(_)) {
            // A folder went away under us; re-read the tree first.
            self.tree_due = Some(Instant::now());
        }
    }

    /// Fetches the next batch of window bodies. False when none are left.
    async fn fetch_body_batch(&mut self) -> Result<bool, GraphError> {
        let since = self.policy.window_start((self.clock)());
        let r = self.reader()?;
        let ids = r.bodies_wanted(
            self.account,
            since,
            self.body_cursor,
            self.policy.body_batch,
        )?;
        let Some(last) = ids.last() else {
            return Ok(false);
        };
        if let Some(m) = r.message(*last)? {
            self.body_cursor = Some(Cursor {
                received_at: m.received_at,
                id: m.id.0,
            });
        }
        drop(r);
        self.phase(SyncPhase::Bodies);
        let results = self
            .sync
            .fetch_bodies(&mut self.w, &ids, Priority::Background)
            .await?;
        let mut ok = 0u64;
        let mut first_err = None;
        for (id, r) in results {
            match r {
                Ok(_) => ok += 1,
                // Deleted since the round: the next round removes it.
                Err(GraphError::NotFound(_)) => {}
                Err(e) if account_wide(&e) => {
                    first_err.get_or_insert(e);
                }
                // This one message refuses: skip it rather than retry the
                // batch for ever. It is tried again on a later pass.
                Err(e) => crate::log(&format!(
                    "{}: body of message {} skipped: {e}",
                    self.key, id.0
                )),
            }
        }
        self.shared.update(|l| l.bodies_fetched += ok);
        if let Some(e) = first_err {
            // Try this batch again after the backoff.
            self.body_cursor = None;
            return Err(e);
        }
        Ok(true)
    }

    /// The calendar list, then every calendar's events for the window.
    async fn sync_calendars(&mut self) -> Result<(), GraphError> {
        let prev = self.shared.snapshot().phase;
        self.phase(SyncPhase::Calendar);
        let acc = self.account;
        self.calendar
            .sync_calendars(&mut self.w, Priority::Background)
            .await?;
        let window = self.policy.calendar_window((self.clock)());
        let calendars = self.w.write(|tx| tx.calendars(acc))?;
        let (mut up, mut rm) = (0, 0);
        for cal in calendars {
            match self
                .calendar
                .sync_events(&mut self.w, &cal.remote_id, window, Priority::Background)
                .await
            {
                Ok(r) => {
                    up += r.upserted;
                    rm += r.removed;
                }
                Err(
                    e @ (GraphError::Auth(_)
                    | GraphError::Network(_)
                    | GraphError::Throttled { .. }),
                ) => return Err(e),
                Err(e) => {
                    let msg = e.to_string();
                    crate::log(&format!("{}: calendar {:?}: {msg}", self.key, cal.name));
                    self.w.write(|tx| {
                        tx.set_sync_error(acc, &events_resource(&cal.remote_id), &msg)
                    })?;
                }
            }
        }
        if up + rm > 0 {
            crate::log(&format!(
                "{}: calendars: {up} events added or changed, {rm} removed",
                self.key
            ));
        }
        self.phase(prev);
        Ok(())
    }

    /// Outlook contacts, in full. A mailbox that refuses (no consent for
    /// contacts, say) is logged and left until next time.
    async fn sync_contacts(&mut self) -> Result<(), GraphError> {
        let acc = self.account;
        match self.contacts.sync(&mut self.w, Priority::Background).await {
            Ok((n, rm)) => {
                let at = (self.clock)();
                self.w
                    .write(|tx| tx.set_sync_state(acc, CONTACTS_RESOURCE, None, None, at))?;
                if rm > 0 {
                    crate::log(&format!("{}: contacts: {n} listed, {rm} removed", self.key));
                }
                Ok(())
            }
            Err(
                e @ (GraphError::Auth(_) | GraphError::Network(_) | GraphError::Throttled { .. }),
            ) => Err(e),
            Err(e) => {
                let msg = e.to_string();
                crate::log(&format!("{}: contacts: {msg}", self.key));
                self.w
                    .write(|tx| tx.set_sync_error(acc, CONTACTS_RESOURCE, &msg))?;
                Ok(())
            }
        }
    }

    fn age(&mut self) -> Result<(), GraphError> {
        let acc = self.account;
        let now = (self.clock)();
        let window = self.policy.window_start(now);
        let headers = self.policy.headers_start(now);
        let limit = i64::try_from(self.policy.cache_limit).unwrap_or(i64::MAX);
        let (aged, pruned, evicted) = self.w.write(|tx| {
            let aged = tx.age_out_bodies(acc, window)?;
            let pruned = match headers {
                Some(h) => tx.prune_headers(acc, h)?,
                None => 0,
            };
            let ev = tx.evict_cached_bodies(limit)?;
            // Done ops keep their payload (a draft's attachments, in base64);
            // a week is plenty for `fam ops list` and `ops.wait`.
            tx.prune_done_ops(now - DONE_OP_RETENTION_SECS)?;
            Ok((aged, pruned, ev.evicted))
        })?;
        let gc = {
            let mut w = self.w.lock();
            let gc = w.gc_bodies()?;
            w.checkpoint()?;
            gc
        };
        if aged + pruned + evicted + gc.rows > 0 {
            crate::log(&format!(
                "{}: aged out {aged} bodies, pruned {pruned} headers, evicted {evicted} cached bodies, freed {} bodies",
                self.key, gc.rows
            ));
        }
        Ok(())
    }
}
