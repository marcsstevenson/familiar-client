// SPDX-License-Identifier: MIT OR Apache-2.0
//! The background spam and scam check (`spam_auto = true` on an account).
//!
//! Every [`SpamAuto::every`] this task reads `config.toml` again (so turning
//! it on or off needs no restart), asks for the new Inbox and Junk mail that
//! hasn't been checked ([`auto_candidates`]) and checks it one message at a
//! time, exactly as `fam mail check-spam` would. A graded result is saved
//! where the desktop app reads it. A message whose check gives no grade (the
//! AI tool failed) isn't tried again until famd restarts.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use familiar_core::commands::check_spam::{CheckSpamInput, auto_candidates, check_at};
use familiar_core::daemon::DaemonPaths;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::log;
use crate::policy::Clock;

/// Where the background check gets its settings.
#[derive(Debug, Clone)]
pub struct SpamAuto {
    /// `config.toml`, read again every round.
    pub config: PathBuf,
    /// How often to look for new mail.
    pub every: Duration,
}

impl SpamAuto {
    /// The background check for `config`, looking once a minute.
    pub fn new(config: PathBuf) -> Self {
        Self {
            config,
            every: Duration::from_secs(60),
        }
    }
}

/// Starts the task.
pub fn spawn(
    opts: SpamAuto,
    paths: DaemonPaths,
    clock: Clock,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tried = HashSet::new();
        loop {
            tokio::select! {
                _ = tokio::time::sleep(opts.every) => {}
                _ = shutdown.changed() => return,
            }
            let config = opts.config.clone();
            let paths = paths.clone();
            let clock = clock.clone();
            let stop = shutdown.clone();
            let res = tokio::task::spawn_blocking(move || {
                round(&config, &paths, &clock, &stop, &mut tried);
                tried
            })
            .await;
            match res {
                Ok(t) => tried = t,
                Err(e) => {
                    log(&format!("spam check task: {e}"));
                    tried = HashSet::new();
                }
            }
            if *shutdown.borrow() {
                return;
            }
        }
    })
}

fn round(
    config: &std::path::Path,
    paths: &DaemonPaths,
    clock: &Clock,
    stop: &watch::Receiver<bool>,
    tried: &mut HashSet<i64>,
) {
    let Ok(cfg) = familiar_config::load(config) else {
        // famd logged the config at start; a bad edit since then is the
        // desktop app's to report.
        return;
    };
    if !cfg.accounts.values().any(|a| a.spam_auto) {
        return;
    }
    let ids = match auto_candidates(paths, &cfg, clock()) {
        Ok(ids) => ids,
        Err(e) => {
            log(&format!("spam check: {}", e.message));
            return;
        }
    };
    for id in ids {
        if *stop.borrow() {
            return;
        }
        if !tried.insert(id) {
            continue;
        }
        let input = CheckSpamInput {
            message: id,
            no_ai: false,
            saved: false,
        };
        // Checks famd makes on its own are never an agent's, whatever
        // environment famd was started from.
        let res = familiar_core::agent::with_source("famd", || {
            check_at(paths, Some(&cfg), &input, clock())
        });
        match res {
            Ok(out) => match &out.ai {
                Some(g) => log(&format!(
                    "spam check: message {id}: {} (spam {}, scam {})",
                    g.headline, g.spam, g.scam
                )),
                None => log(&format!(
                    "spam check: message {id}: not graded: {}",
                    out.notes.join("; ")
                )),
            },
            Err(e) => log(&format!("spam check: message {id}: {}", e.message)),
        }
    }
}
