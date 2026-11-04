// SPDX-License-Identifier: MIT OR Apache-2.0
//! `famd`, the sync daemon and the only writer to the store (WP 1.7).
//!
//! - [`policy`]: what to keep and how often to poll, from `[sync]`.
//! - [`engine`]: one task per account: first sync, polling, body backfill
//!   for the window, ageing and the body cache.
//! - [`service`]: requests from `fam` and the GUI (status, pin, on-demand
//!   body fetch, sync now, stop).
//! - [`ipc`]: the per-user socket or named pipe those requests arrive on.
//!
//! [`Daemon::start`] runs all of it on the current Tokio runtime; the
//! `famd` binary builds a [`DaemonOptions`] from `config.toml` and the
//! sign-in store, and tests build one against the mock Graph.

pub mod engine;
pub mod ipc;
pub mod policy;
pub mod service;

use std::collections::HashMap;
use std::sync::Arc;

use familiar_core::daemon::DaemonPaths;
use familiar_provider_graph::{GraphClient, MailApi, MailSync};
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
            let api = MailApi::new(spec.client);
            let mut sync = MailSync::new(api.clone(), id);
            if let Some(n) = opts.page_size {
                sync = sync.with_page_size(n);
            }
            let shared = Arc::new(AccountShared::new(&spec.key));
            engines.push(Engine::new(
                spec.key.clone(),
                id,
                sync,
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
        });
        let tasks = engines.into_iter().map(|e| tokio::spawn(e.run())).collect();
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
