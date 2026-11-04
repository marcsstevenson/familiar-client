// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sharing the one writer between the daemon's tasks.
//!
//! `famd` syncs several accounts at once and answers requests from the CLI
//! and GUI while it does, but there is still exactly one [`Writer`]. A
//! [`SharedWriter`] wraps it in a mutex that is held only for the length of
//! one transaction, never across an `.await`, so a long first sync of one
//! account does not hold up a pin request or another account's poll.
//!
//! Code that writes, such as the Graph provider's `MailSync`, takes any
//! [`StoreWriter`], so it works on a plain [`Writer`] in tests and on a
//! [`SharedWriter`] in the daemon.

use crate::{Result, StorePaths, WriteTx, Writer};
use std::sync::{Arc, Mutex, MutexGuard};

/// Something that can run a write transaction on the store.
pub trait StoreWriter {
    /// Runs `f` in one write transaction; see [`Writer::write`].
    fn write<T>(&mut self, f: impl FnOnce(&WriteTx<'_>) -> Result<T>) -> Result<T>;
    /// The store's paths.
    fn paths(&self) -> &StorePaths;
}

impl StoreWriter for Writer {
    fn write<T>(&mut self, f: impl FnOnce(&WriteTx<'_>) -> Result<T>) -> Result<T> {
        Writer::write(self, f)
    }
    fn paths(&self) -> &StorePaths {
        Writer::paths(self)
    }
}

/// A [`Writer`] that many tasks can use, one transaction at a time. Cloning
/// it shares the same writer.
#[derive(Clone)]
pub struct SharedWriter {
    inner: Arc<Mutex<Writer>>,
    paths: Arc<StorePaths>,
}

impl std::fmt::Debug for SharedWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedWriter")
            .field("root", &self.paths.root)
            .finish()
    }
}

impl SharedWriter {
    /// Shares `writer`.
    pub fn new(writer: Writer) -> Self {
        let paths = Arc::new(writer.paths().clone());
        Self {
            inner: Arc::new(Mutex::new(writer)),
            paths,
        }
    }

    /// The writer itself, for maintenance such as [`Writer::gc_bodies`].
    /// Don't hold the guard across an `.await`.
    pub fn lock(&self) -> MutexGuard<'_, Writer> {
        // A panic inside a transaction rolled it back, so the writer is
        // still consistent; carry on with it.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The store's paths.
    pub fn paths(&self) -> &StorePaths {
        &self.paths
    }

    /// Runs `f` in one write transaction.
    pub fn write<T>(&self, f: impl FnOnce(&WriteTx<'_>) -> Result<T>) -> Result<T> {
        self.lock().write(f)
    }
}

impl StoreWriter for SharedWriter {
    fn write<T>(&mut self, f: impl FnOnce(&WriteTx<'_>) -> Result<T>) -> Result<T> {
        SharedWriter::write(self, f)
    }
    fn paths(&self) -> &StorePaths {
        SharedWriter::paths(self)
    }
}
