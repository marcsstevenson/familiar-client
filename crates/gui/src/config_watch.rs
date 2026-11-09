// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tells the window when config.toml changes, so it can offer a restart.
//! The app and famd read the config only when they start.

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use familiar_core::daemon::{Client, DaemonPaths, METHOD_STOP};
use tauri::{AppHandle, Emitter};

/// How often the file's modified time is checked.
const CHECK_EVERY: Duration = Duration::from_secs(2);

/// Emits `config_changed` each time config.toml's modified time changes.
pub fn watch(app: AppHandle) {
    let Some((path, _)) = familiar_config::paths::resolve(None) else {
        return;
    };
    std::thread::spawn(move || {
        let mut last = modified(&path);
        loop {
            std::thread::sleep(CHECK_EVERY);
            let now = modified(&path);
            if now != last {
                last = now;
                let _ = app.emit("config_changed", ());
            }
        }
    });
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Stops famd, so it starts again with the new config, then restarts the app.
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    if let Ok(paths) = DaemonPaths::resolve() {
        let answers =
            || Client::connect_with_timeout(&paths.socket, Duration::from_millis(500)).is_ok();
        if let Ok(mut client) = Client::connect_with_timeout(&paths.socket, Duration::from_secs(2))
        {
            let _: Result<serde_json::Value, _> = client.call(METHOD_STOP, &serde_json::json!({}));
            let start = Instant::now();
            while answers() && start.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    app.restart();
}
