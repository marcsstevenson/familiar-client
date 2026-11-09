// SPDX-License-Identifier: MIT OR Apache-2.0
//! `famd`, the sync daemon and the only writer to the store (WP 1.7).
//!
//! - [`policy`]: what to keep and how often to poll, from `[sync]`.
//! - [`engine`]: one task per account: first sync, polling, body backfill
//!   for the window, ageing and the body cache, and calendars.
//! - [`service`]: requests from `fam` and the GUI (status, pin, on-demand
//!   body fetch, server search, sync now, the consistency check, stop).
//! - [`indexer`]: keeps the search index beside the store up to date.
//! - [`check`]: the store-versus-server consistency check (WP 1.12).
//! - [`spam`]: the background spam and scam check (`spam_auto`).
//! - [`ipc`]: the per-user socket or named pipe those requests arrive on.
//!
//! [`Daemon::start`] runs all of it on the current Tokio runtime; the
//! `famd` binary builds a [`DaemonOptions`] from `config.toml` and the
//! sign-in store, and tests build one against the mock Graph.

pub mod check;
pub mod engine;
pub mod indexer;
pub mod ipc;
pub mod ops;
pub mod policy;
pub mod service;
pub mod spam;

use std::collections::HashMap;
use std::sync::Arc;

use familiar_core::daemon::DaemonPaths;
use familiar_provider_graph::{CalendarApi, CalendarSync, GraphClient, MailApi, MailSync};
use familiar_store::{NewAccount, SharedWriter, Writer};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::engine::{AccountShared, Engine};
use crate::policy::{Clock, SyncPolicy, system_clock};
use crate::service::{AccountHandle, Service};

/// The crate name, used by the workspace smoke tests.
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Writes a line to stderr with a UTC timestamp.
pub fn log(line: &str) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (h, m, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    eprintln!("famd {h:02}:{m:02}:{s:02}Z {line}");
}

/// One account to sync.
pub struct AccountSpec {
    /// Its key in `config.toml`.
    pub key: String,
    /// The mailbox address.
    pub email: String,
    /// The name shown in the UI.
    pub display_name: String,
    /// Directory tenant for work accounts.
    pub tenant_id: Option<String>,
    /// A Graph client for the mailbox.
    pub client: GraphClient,
}

/// Everything the daemon needs.
pub struct DaemonOptions {
    /// Store directory and socket.
    pub paths: DaemonPaths,
    /// Sync settings.
    pub policy: SyncPolicy,
    /// Accounts to sync.
    pub accounts: Vec<AccountSpec>,
    /// The clock (tests pin it).
    pub clock: Clock,
    /// Messages per delta page; `None` for the provider's default.
    pub page_size: Option<usize>,
    /// What agent sessions may do (WP 2.6).
    pub agent: AgentGrants,
    /// The background spam and scam check; `None` leaves it off.
    pub spam: Option<spam::SpamAuto>,
}

/// Where famd gets the agent grants from.
#[derive(Debug, Clone)]
pub enum AgentGrants {
    /// These, for good (tests).
    Fixed(familiar_core::agent::Grants),
    /// `[agent]` of this config file, read again for each agent write so a
    /// change applies without a restart.
    File(std::path::PathBuf),
}

impl AgentGrants {
    /// The grants now.
    pub fn get(&self) -> familiar_core::agent::Grants {
        match self {
            AgentGrants::Fixed(g) => g.clone(),
            AgentGrants::File(p) => familiar_core::agent::Grants::load_from(p),
        }
    }
}

impl DaemonOptions {
    /// Options with the default policy and system clock.
    pub fn new(paths: DaemonPaths, accounts: Vec<AccountSpec>) -> Self {
        Self {
            paths,
            policy: SyncPolicy::default(),
            accounts,
            clock: system_clock(),
            page_size: None,
            agent: AgentGrants::Fixed(familiar_core::agent::Grants::default()),
            spam: None,
        }
    }
}

/// Why the daemon could not start.
#[derive(Debug)]
pub enum StartError {
    /// Another `famd` holds the store.
    AlreadyRunning(std::path::PathBuf),
    /// The store could not be opened or written.
    Store(familiar_store::Error),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::AlreadyRunning(p) => {
                write!(
                    f,
                    "famd is already running for the store at {}",
                    p.display()
                )
            }
            StartError::Store(e) => write!(f, "store: {e}"),
        }
    }
}

impl std::error::Error for StartError {}

/// A running daemon.
pub struct Daemon {
    shutdown: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    listener: JoinHandle<std::io::Result<()>>,
    service: Arc<Service>,
    writer: SharedWriter,
}

impl Daemon {
    /// Opens the store (taking the writer lock), adds the accounts and
    /// starts the engines and the socket. Must be called inside a Tokio
    /// runtime.
    pub fn start(opts: DaemonOptions) -> Result<Self, StartError> {
        let writer = match Writer::open(&opts.paths.store) {
            Ok(w) => w,
            Err(familiar_store::Error::WriterLocked(p)) => {
                return Err(StartError::AlreadyRunning(p));
            }
            Err(e) => return Err(StartError::Store(e)),
        };
        let w = SharedWriter::new(writer);
        // Ops claimed when famd last stopped go back to the front of the queue.
        match w.write(|t| t.requeue_in_flight()) {
            Ok(0) => {}
            Ok(n) => log(&format!(
                "{n} op(s) left in flight last time are queued again"
            )),
            Err(e) => return Err(StartError::Store(e)),
        }
        let signals = Arc::new(ops::OpSignals::default());
        let mut runners = Vec::new();
        let (tx, rx) = watch::channel(false);
        let policy = Arc::new(opts.policy);
        let mut accounts = HashMap::new();
        let mut engines = Vec::new();
        for spec in opts.accounts {
            let id = w
                .write(|t| {
                    t.upsert_account(&NewAccount {
                        key: spec.key.clone(),
                        email: spec.email.clone(),
                        display_name: spec.display_name.clone(),
                        provider: "graph".into(),
                        tenant_id: spec.tenant_id.clone(),
                    })
                })
                .map_err(StartError::Store)?;
            let cal_api = CalendarApi::new(spec.client.clone());
            let calendar = CalendarSync::new(cal_api.clone(), id);
            let contacts = familiar_provider_graph::ContactSync::new(
                familiar_provider_graph::ContactsApi::new(spec.client.clone()),
                id,
            );
            let api = MailApi::new(spec.client);
            let mut sync = MailSync::new(api.clone(), id);
            if let Some(n) = opts.page_size {
                sync = sync.with_page_size(n);
            }
            let shared = Arc::new(AccountShared::new(&spec.key));
            let ops_wake = Arc::new(tokio::sync::Notify::new());
            runners.push(ops::OpRunner {
                key: spec.key.clone(),
                account: id,
                api: api.clone(),
                cal: cal_api,
                w: w.clone(),
                policy: policy.clone(),
                clock: opts.clock.clone(),
                wake: ops_wake.clone(),
                signals: signals.clone(),
                shutdown: rx.clone(),
            });
            engines.push(Engine::new(
                spec.key.clone(),
                id,
                sync,
                calendar,
                contacts,
                w.clone(),
                policy.clone(),
                opts.clock.clone(),
                shared.clone(),
                rx.clone(),
            ));
            accounts.insert(
                id,
                AccountHandle {
                    key: spec.key,
                    api,
                    shared,
                    ops_wake,
                },
            );
        }
        let service = Arc::new(Service {
            w: w.clone(),
            accounts,
            policy,
            clock: opts.clock.clone(),
            started_at: (opts.clock)(),
            shutdown: tx.clone(),
            signals,
            agent: opts.agent.clone(),
        });
        let mut tasks: Vec<JoinHandle<()>> =
            engines.into_iter().map(|e| tokio::spawn(e.run())).collect();
        tasks.extend(runners.into_iter().map(|r| tokio::spawn(r.run())));
        if let Some(t) = indexer::spawn(&opts.paths.index, w.clone(), rx.clone()) {
            tasks.push(t);
        }
        if let Some(s) = opts.spam {
            tasks.push(spam::spawn(
                s,
                opts.paths.clone(),
                opts.clock.clone(),
                rx.clone(),
            ));
        }
        let socket = opts.paths.socket.clone();
        let svc = service.clone();
        let listener = tokio::spawn(async move {
            let r = ipc::listen(&socket, svc, rx).await;
            if let Err(e) = &r {
                log(&format!("socket {}: {e}", socket.display()));
            }
            r
        });
        Ok(Self {
            shutdown: tx,
            tasks,
            listener,
            service,
            writer: w,
        })
    }

    /// The request handlers, for tests that call them without a socket.
    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }

    /// The store's writer, for tests.
    pub fn writer(&self) -> &SharedWriter {
        &self.writer
    }

    /// Resolves when something asks the daemon to stop (`daemon.stop`).
    pub async fn stopped(&self) {
        let mut rx = self.shutdown.subscribe();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    /// Stops every task and waits for them; the writer lock is released when
    /// this returns.
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        for t in self.tasks {
            let _ = t.await;
        }
        match self.listener.await {
            Ok(Err(e)) => log(&format!("socket: {e}")),
            Err(e) => log(&format!("socket task: {e}")),
            Ok(Ok(())) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn name_is_set() {
        assert_eq!(super::NAME, "familiar-daemon");
    }
}
