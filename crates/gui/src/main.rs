// SPDX-License-Identifier: MIT OR Apache-2.0
//! Familiar desktop app: a Tauri 2 window running the SolidJS front end in `ui/`.
//!
//! The front end reaches the engine through one IPC command, `run`, which calls
//! the same command registry as `fam` and the MCP server, so every surface
//! shares one implementation. famd is started in the background if it isn't
//! running. This is the only crate allowed to depend on a GUI or webview library.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod daemon;

use familiar_core::registry::Registry;
use familiar_core::{CommandError, agent};
use serde::Serialize;
use serde_json::Value;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// The source recorded for changes made from the desktop app.
const SOURCE_GUI: &str = "gui";

/// Commands the window can't run: they need a terminal of their own.
const NOT_FROM_GUI: &[&str] = &["mcp.serve"];

/// Facts the front end needs before it draws anything.
#[derive(Debug, Serialize)]
struct UiInfo {
    version: &'static str,
    /// `windows`, `macos` or `linux`: picks the title bar style.
    platform: &'static str,
    /// The `[app] theme` from config.toml: `system`, `light` or `dark`.
    theme: &'static str,
    /// Why config.toml could not be read, if it couldn't.
    config_error: Option<String>,
}

#[tauri::command]
fn ui_info() -> UiInfo {
    let (theme, config_error) = match familiar_config::paths::resolve(None) {
        None => ("system", None),
        Some((path, _)) if !path.exists() => ("system", None),
        Some((path, _)) => match familiar_config::load(&path) {
            Ok(c) => (theme_name(c.app.theme), None),
            Err(e) => ("system", Some(format!("{}: {e}", path.display()))),
        },
    };
    UiInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        theme,
        config_error,
    }
}

fn theme_name(t: familiar_config::model::Theme) -> &'static str {
    use familiar_config::model::Theme;
    match t {
        Theme::System => "system",
        Theme::Light => "light",
        Theme::Dark => "dark",
    }
}

/// Runs a registered command with JSON input, off the UI thread.
#[tauri::command]
async fn run(
    name: String,
    input: Value,
    registry: tauri::State<'_, std::sync::Arc<Registry>>,
) -> Result<Value, CommandError> {
    if NOT_FROM_GUI.contains(&name.as_str()) {
        return Err(CommandError::usage(format!(
            "`{name}` runs from a terminal, not the app"
        )));
    }
    let registry = std::sync::Arc::clone(&registry);
    tauri::async_runtime::spawn_blocking(move || {
        agent::with_source(SOURCE_GUI, || registry.run(&name, input)).map(|o| o.json)
    })
    .await
    .map_err(|e| CommandError::internal(format!("command task failed: {e}")))?
}

/// The daemon's state as last seen.
#[tauri::command]
fn daemon_state(state: tauri::State<'_, daemon::Shared>) -> daemon::DaemonState {
    state.get()
}

fn main() {
    tauri::Builder::default()
        .manage(std::sync::Arc::new(Registry::builtin()))
        .manage(daemon::Shared::default())
        .invoke_handler(tauri::generate_handler![ui_info, run, daemon_state])
        .setup(|app| {
            let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Familiar")
                .inner_size(1280.0, 800.0)
                .min_inner_size(720.0, 420.0);
            // macOS keeps its traffic lights over our own title bar; elsewhere the
            // app draws the whole bar, including the window buttons.
            #[cfg(target_os = "macos")]
            let builder = builder
                .title_bar_style(tauri::TitleBarStyle::Overlay)
                .hidden_title(true);
            #[cfg(not(target_os = "macos"))]
            let builder = builder.decorations(false);
            builder.build()?;
            let handle = app.handle().clone();
            let shared = app.state::<daemon::Shared>().inner().clone();
            std::thread::spawn(move || daemon::keep_running(&handle, &shared));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Familiar app");
}
