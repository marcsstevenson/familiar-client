// SPDX-License-Identifier: MIT OR Apache-2.0
//! `[[hooks]]`: commands famd runs when new mail arrives.
//!
//! Every [`Hooks::every`] this task asks a [`NewMailWatch`] for the mail
//! that arrived since it last looked, reads `config.toml` again (so adding
//! or changing a hook needs no restart) and runs each enabled `new_mail`
//! hook whose `match` the message fits, one at a time. Mail that arrived
//! before famd started, or while no hook was set up, never runs a hook.
//!
//! The command line goes to the system shell. The event goes to the command
//! as one JSON line on stdin and as `FAMILIAR_*` environment variables; mail
//! content is never pasted into the command line, so a subject can't run
//! anything. The command's output goes to famd's log. A command still
//! running after `timeout_seconds` (or when famd stops) is killed.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use familiar_config::Hook;
use familiar_core::daemon::DaemonPaths;
use familiar_core::watch::{MailEvent, NewMailWatch};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::log;
use crate::policy::Clock;

/// Where the hooks come from.
#[derive(Debug, Clone)]
pub struct Hooks {
    /// `config.toml`, read again every round.
    pub config: PathBuf,
    /// How often to look for new mail.
    pub every: Duration,
}

impl Hooks {
    /// Hooks from `config`, looking every 10 seconds.
    pub fn new(config: PathBuf) -> Self {
        Self {
            config,
            every: Duration::from_secs(10),
        }
    }
}

/// Starts the task.
pub fn spawn(
    opts: Hooks,
    paths: DaemonPaths,
    clock: Clock,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut state = Some((NewMailWatch::new(&paths), HashSet::new()));
        loop {
            let (mut w, mut warned) = state.take().expect("put back each round");
            let config = opts.config.clone();
            let clock = clock.clone();
            let stop = shutdown.clone();
            let res = tokio::task::spawn_blocking(move || {
                round(&config, &mut w, &clock, &stop, &mut warned);
                (w, warned)
            })
            .await;
            match res {
                Ok(s) => state = Some(s),
                Err(e) => {
                    log(&format!("hooks task: {e}"));
                    state = Some((NewMailWatch::new(&paths), HashSet::new()));
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(opts.every) => {}
                _ = shutdown.changed() => return,
            }
            if *shutdown.borrow() {
                return;
            }
        }
    })
}

fn round(
    config: &Path,
    w: &mut NewMailWatch,
    clock: &Clock,
    stop: &watch::Receiver<bool>,
    warned: &mut HashSet<String>,
) {
    let now = clock();
    // Polled whether or not there are hooks, so a hook added later starts
    // with the mail that comes after it.
    let events = match w.poll(now) {
        Ok(e) => e,
        Err(e) => {
            log(&format!("hooks: {e}"));
            return;
        }
    };
    if events.is_empty() {
        return;
    }
    // famd logged the config at start; a bad edit since then is the desktop
    // app's to report.
    let Ok(cfg) = familiar_config::load(config) else {
        return;
    };
    let ids: Vec<i64> = events.iter().map(|e| e.id).collect();
    for (i, hook) in cfg.hooks.iter().enumerate() {
        if !hook.enabled || hook.on != "new_mail" {
            continue;
        }
        let name = hook.name.clone().unwrap_or_else(|| format!("hooks[{i}]"));
        let wanted = match &hook.matches {
            None => ids.iter().copied().collect(),
            Some(q) => match w.matching(q, &ids, now) {
                Ok(m) => m,
                Err(e) => {
                    // Once per problem, not every round.
                    if warned.insert(format!("{name}: {e}")) {
                        log(&format!("hook {name}: match `{q}`: {e}; not run"));
                    }
                    continue;
                }
            },
        };
        for ev in events.iter().filter(|e| wanted.contains(&e.id)) {
            if *stop.borrow() {
                return;
            }
            run_hook(&name, hook, ev, stop);
        }
    }
}

/// Runs `hook` for `ev` and waits for it, logging how it went.
pub fn run_hook(name: &str, hook: &Hook, ev: &MailEvent, stop: &watch::Receiver<bool>) {
    let mut c = shell(&hook.run);
    let clean = |s: &str| s.replace('\0', "");
    c.env("FAMILIAR_HOOK", clean(name))
        .env("FAMILIAR_EVENT", &ev.event)
        .env("FAMILIAR_MESSAGE_ID", ev.id.to_string())
        .env("FAMILIAR_ACCOUNT", clean(&ev.account))
        .env("FAMILIAR_FOLDER", clean(&ev.folder))
        .env("FAMILIAR_FROM", clean(&ev.from.addr))
        .env("FAMILIAR_FROM_NAME", clean(&ev.from.name))
        .env("FAMILIAR_SUBJECT", clean(&ev.subject))
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            log(&format!("hook {name}: message {}: can't start: {e}", ev.id));
            return;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        // A command that doesn't read stdin may have closed it already.
        let line = serde_json::to_string(ev).unwrap_or_default();
        let _ = writeln!(stdin, "{line}");
    }
    let deadline = Instant::now() + Duration::from_secs(hook.timeout_seconds.into());
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {}
            Err(e) => {
                log(&format!("hook {name}: message {}: {e}", ev.id));
                break None;
            }
        }
        if *stop.borrow() || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let why = if *stop.borrow() {
                "famd is stopping"
            } else {
                "timed out"
            };
            log(&format!("hook {name}: message {}: stopped ({why})", ev.id));
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    match status {
        Some(s) if s.success() => log(&format!("hook {name}: message {}: ran", ev.id)),
        Some(s) => log(&format!("hook {name}: message {}: exited with {s}", ev.id)),
        None => {}
    }
}

/// The command line `run` for the system shell.
fn shell(run: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // famd has no console, so Windows would open one for each run.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut c = Command::new("cmd");
        // Passed as typed: Rust's quoting rules aren't cmd's.
        c.arg("/C").raw_arg(run).creation_flags(CREATE_NO_WINDOW);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(run);
        c
    }
}
