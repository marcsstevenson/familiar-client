// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keeps the search index in step with the store.
//!
//! Every message change the index cares about lands in the store's search
//! queue (by trigger), whoever made it. This task drains the queue every
//! [`INDEX_INTERVAL`] on a blocking thread, so a burst of sync writes is
//! indexed a couple of seconds later, in batches. A missing or outdated
//! index is rebuilt from the store at start.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use familiar_search::{Indexer, Opened};
use familiar_store::SharedWriter;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::log;

/// Batches indexed before checking for shutdown.
const BATCHES_PER_STEP: usize = 10;

/// How often the queue is checked.
pub const INDEX_INTERVAL: Duration = Duration::from_secs(2);

/// Opens (or rebuilds) the index in `dir` and starts the task that keeps it
/// up to date. Returns `None`, after logging, if the index can't be opened;
/// sync carries on without search.
pub fn spawn(
    dir: &Path,
    mut w: SharedWriter,
    mut shutdown: watch::Receiver<bool>,
) -> Option<JoinHandle<()>> {
    let ix = match Indexer::open_for(dir, &mut w) {
        Ok((ix, Opened::Existing)) => ix,
        Ok((ix, Opened::Rebuilt)) => {
            log(&format!("search index at {} is being built", dir.display()));
            ix
        }
        Err(e) => {
            log(&format!(
                "search index at {}: {e}; search is off",
                dir.display()
            ));
            return None;
        }
    };
    let ix = Arc::new(Mutex::new(ix));
    Some(tokio::spawn(async move {
        loop {
            let ix = ix.clone();
            let mut w = w.clone();
            let res = tokio::task::spawn_blocking(move || {
                let mut ix = ix.lock().unwrap_or_else(|p| p.into_inner());
                ix.drain_some(&mut w, BATCHES_PER_STEP)
            })
            .await;
            let mut more = false;
            match res {
                Ok(Ok((n, m))) => {
                    more = m;
                    if n >= familiar_search::BATCH {
                        log(&format!("indexed {n} messages"));
                    }
                }
                Ok(Err(e)) => log(&format!("search index: {e}")),
                Err(e) => log(&format!("search index task: {e}")),
            }
            if *shutdown.borrow() {
                return;
            }
            if more {
                continue;
            }
            tokio::select! {
                _ = tokio::time::sleep(INDEX_INTERVAL) => {}
                _ = shutdown.changed() => return,
            }
        }
    }))
}
