// SPDX-License-Identifier: MIT OR Apache-2.0
//! Starts famd when the app opens and it isn't already running.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use familiar_core::daemon::{Client, DaemonPaths};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// How long to wait for a freshly started famd to answer.
const START_WAIT: Duration = Duration::from_secs(10);

/// What the app knows about famd. Sent to the front end as the `daemon` event.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DaemonState {
    /// Still checking or starting.
    Starting,
    /// famd answers on its socket.
    Running {
        /// True when this app started it.
        started_by_app: bool,
    },
    /// famd isn't running and couldn't be started; the app still reads the store.
    Unavailable {
        /// Why.
        message: String,
    },
}

/// The last state, shared between the starter thread and the `daemon_state` command.
#[derive(Clone)]
pub struct Shared(Arc<Mutex<DaemonState>>);

impl Default for Shared {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(DaemonState::Starting)))
    }
}

impl Shared {
    /// The last state.
    pub fn get(&self) -> DaemonState {
        self.0
            .lock()
            .map(|s| s.clone())
            .unwrap_or(DaemonState::Starting)
    }

    fn set(&self, app: &AppHandle, s: DaemonState) {
        if let Ok(mut g) = self.0.lock() {
            *g = s.clone();
        }
        let _ = app.emit("daemon", s);
    }
}

/// How often a running famd is checked, and how long to wait before trying
/// again after a start failed.
const CHECK_EVERY: Duration = Duration::from_secs(5);
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// Keeps famd running for as long as the app is open: starts it if nothing
/// answers, and starts it again if it stops.
pub fn keep_running(app: &AppHandle, shared: &Shared) {
    let paths = match DaemonPaths::resolve() {
        Ok(p) => p,
        Err(e) => {
            return shared.set(app, DaemonState::Unavailable { message: e.message });
        }
    };
    loop {
        ensure_running(app, shared, &paths);
        if matches!(shared.get(), DaemonState::Unavailable { .. }) {
            std::thread::sleep(RETRY_AFTER);
            continue;
        }
        while Client::connect_with_timeout(&paths.socket, Duration::from_millis(500)).is_ok() {
            std::thread::sleep(CHECK_EVERY);
        }
    }
}

/// Connects to famd, starting it first if nothing answers.
fn ensure_running(app: &AppHandle, shared: &Shared, paths: &DaemonPaths) {
    let answers =
        || Client::connect_with_timeout(&paths.socket, Duration::from_millis(500)).is_ok();
    if answers() {
        return shared.set(
            app,
            DaemonState::Running {
                started_by_app: false,
            },
        );
    }
    let exe = famd_path();
    let log = paths.data_dir.join("famd.log");
    let mut child = match spawn(&exe, &log) {
        Ok(c) => c,
        Err(e) => {
            let message = format!("could not start {}: {e}", exe.display());
            return shared.set(app, DaemonState::Unavailable { message });
        }
    };
    let start = Instant::now();
    while start.elapsed() < START_WAIT {
        if answers() {
            return shared.set(
                app,
                DaemonState::Running {
                    started_by_app: true,
                },
            );
        }
        if let Ok(Some(_)) = child.try_wait() {
            let message = last_line(&log).unwrap_or_else(|| "famd stopped".into());
            return shared.set(app, DaemonState::Unavailable { message });
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    shared.set(
        app,
        DaemonState::Unavailable {
            message: format!("{} started but did not answer within 10 s", exe.display()),
        },
    );
}

/// famd next to this executable if it is there (an installed or `cargo build`
/// layout), otherwise `famd` from PATH.
fn famd_path() -> PathBuf {
    let name = format!("famd{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(&name)))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Starts famd detached, with no console window, so it outlives the app. Its
/// log lines go to `famd.log` in the data directory.
fn spawn(exe: &PathBuf, log: &std::path::Path) -> std::io::Result<std::process::Child> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?;
    let mut cmd = std::process::Command::new(exe);
    cmd.stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
}

/// The last line famd wrote to its log, which says why it stopped.
fn last_line(log: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(str::to_string)
}
