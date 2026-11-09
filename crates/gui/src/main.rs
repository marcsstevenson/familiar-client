// SPDX-License-Identifier: MIT OR Apache-2.0
//! Familiar desktop app: a Tauri 2 window running the SolidJS front end in `ui/`.
//!
//! The front end reaches the engine through one IPC command, `run`, which calls
//! the same command registry as `fam` and the MCP server, so every surface
//! shares one implementation. famd is started in the background if it isn't
//! running. This is the only crate allowed to depend on a GUI or webview library.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod compose;
mod daemon;
mod list;
mod reading;
mod ui_socket;

use familiar_core::registry::Registry;
use familiar_core::{CommandError, agent};
use serde::Serialize;
use serde_json::Value;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// The source recorded for changes made from the desktop app.
const SOURCE_GUI: &str = "gui";

/// Commands the window can't run: they need a terminal of their own.
const NOT_FROM_GUI: &[&str] = &["mcp.serve"];

/// One saved search.
#[derive(Debug, Serialize)]
struct View {
    name: String,
    query: String,
}

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
    /// `[keys]` from config.toml: action name to key, overriding the defaults.
    keys: std::collections::BTreeMap<String, String>,
    /// `[[views]]` from config.toml: saved searches shown as smart folders.
    views: Vec<View>,
    /// Each account's `processed_folder` from config.toml, by account key:
    /// the list shows its tick button for these accounts.
    processed: std::collections::BTreeMap<String, String>,
    /// `[ai] tool` when `[ai]` is on (`auto`, `claude`, `codex` or a path):
    /// the Add to calendar dialog names it while it waits.
    ai_tool: Option<String>,
    /// The accounts whose mail `[ai]` may send to that tool.
    ai_accounts: Vec<String>,
    /// True when `FAMILIAR_BENCH_OUT` names a file: the front end then runs
    /// its timing script and reports with `bench_report`.
    bench: bool,
}

#[tauri::command]
fn ui_info() -> UiInfo {
    let config = match familiar_config::paths::resolve(None) {
        None => Ok(None),
        Some((path, _)) if !path.exists() => Ok(None),
        Some((path, _)) => familiar_config::load(&path)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
    };
    let (config, config_error) = match config {
        Ok(c) => (c, None),
        Err(e) => (None, Some(e)),
    };
    let (ai_tool, ai_accounts) = match &config {
        Some(c) if c.ai.enabled && c.ai.tool != "none" => (
            Some(c.ai.tool.clone()),
            c.ai.accounts
                .clone()
                .unwrap_or_else(|| c.accounts.keys().cloned().collect()),
        ),
        _ => (None, Vec::new()),
    };
    let (theme, keys, views, processed) = match config {
        Some(c) => (
            theme_name(c.app.theme),
            c.keys,
            c.views
                .into_iter()
                .map(|v| View {
                    name: v.name,
                    query: v.query,
                })
                .collect(),
            c.accounts
                .into_iter()
                .filter_map(|(k, a)| a.processed_folder.map(|f| (k, f)))
                .collect(),
        ),
        None => ("system", Default::default(), Vec::new(), Default::default()),
    };
    UiInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        theme,
        keys,
        views,
        processed,
        ai_tool,
        ai_accounts,
        config_error,
        bench: std::env::var_os(BENCH_ENV).is_some(),
    }
}

/// Names the file the bench results go to.
const BENCH_ENV: &str = "FAMILIAR_BENCH_OUT";

/// When set with `FAMILIAR_BENCH_OUT`, the app stays open after reporting,
/// so a harness can measure its idle memory.
const BENCH_HOLD_ENV: &str = "FAMILIAR_BENCH_HOLD";

/// When `main` started, for cold start timing.
static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Milliseconds since the process started.
#[tauri::command]
fn bench_elapsed_ms() -> f64 {
    STARTED
        .get()
        .map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0)
}

/// Writes the front end's timing results (JSON) to `FAMILIAR_BENCH_OUT`, then
/// closes the app.
#[tauri::command]
fn bench_report(report: Value, app: tauri::AppHandle) -> Result<(), CommandError> {
    let Some(path) = std::env::var_os(BENCH_ENV) else {
        return Ok(());
    };
    std::fs::write(&path, format!("{report:#}\n"))
        .map_err(|e| CommandError::internal(format!("write bench results: {e}")))?;
    if std::env::var_os(BENCH_HOLD_ENV).is_none() {
        app.exit(0);
    }
    Ok(())
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

/// Opens config.toml in the system's default app for `.toml` files. Windows
/// often has none, so it falls back to Notepad there. Returns the path.
#[tauri::command]
fn open_config() -> Result<String, CommandError> {
    let (path, _) = familiar_config::paths::resolve(None)
        .ok_or_else(|| CommandError::internal("no home folder to look for config.toml in"))?;
    if !path.exists() {
        return Err(CommandError::usage(format!(
            "there is no config file at {}",
            path.display()
        )));
    }
    let opened = open::that_detached(&path);
    #[cfg(windows)]
    let opened = opened.or_else(|_| open::with_detached(&path, "notepad"));
    opened.map_err(|e| CommandError::internal(format!("couldn't open {}: {e}", path.display())))?;
    Ok(path.display().to_string())
}

/// Opens a message list view; see [`list::Lists::open`].
#[tauri::command]
async fn list_open(
    spec: list::ViewSpec,
    lists: tauri::State<'_, std::sync::Arc<list::Lists>>,
) -> Result<list::Opened, CommandError> {
    let lists = std::sync::Arc::clone(&lists);
    tauri::async_runtime::spawn_blocking(move || lists.open(&spec))
        .await
        .map_err(|e| CommandError::internal(format!("list task failed: {e}")))?
}

/// Rows of the open view by position.
#[tauri::command]
fn list_rows(
    start: usize,
    count: usize,
    lists: tauri::State<'_, std::sync::Arc<list::Lists>>,
) -> Result<Vec<list::Row>, CommandError> {
    lists.rows(start, count)
}

/// Where a message is in the open view, if it is there.
#[tauri::command]
fn list_index_of(id: i64, lists: tauri::State<'_, std::sync::Arc<list::Lists>>) -> Option<usize> {
    lists.index_of(id)
}

fn main() {
    STARTED.get_or_init(std::time::Instant::now);
    tauri::Builder::default()
        .manage(std::sync::Arc::new(Registry::builtin()))
        .manage(daemon::Shared::default())
        .plugin(tauri_plugin_dialog::init())
        .manage(reading::Reading::load())
        .register_asynchronous_uri_scheme_protocol("famimg", |_ctx, request, responder| {
            tauri::async_runtime::spawn(async move {
                responder.respond(reading::remote::proxy(request).await);
            });
        })
        .manage(std::sync::Arc::new(list::Lists::default()))
        .invoke_handler(tauri::generate_handler![
            ui_info,
            open_config,
            run,
            daemon_state,
            reading::reading_open,
            reading::reading_trust_sender,
            reading::reading_attachments,
            reading::reading_preview,
            reading::reading_save,
            reading::reading_open_link,
            list_open,
            list_rows,
            list_index_of,
            bench_report,
            compose::compose_info,
            compose::open_compose,
            compose::file_info,
            compose::pick_files,
            bench_elapsed_ms
        ])
        .setup(|app| {
            let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Familiar")
                .inner_size(1280.0, 800.0)
                .min_inner_size(720.0, 420.0)
                // Off so messages can be dragged onto folders: on Windows the
                // native file-drop handler swallows the page's own drag and drop.
                // Only compose windows take dropped files.
                .disable_drag_drop_handler();
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
            let handle = app.handle().clone();
            std::thread::spawn(move || list::watch_store(&handle));
            let registry = std::sync::Arc::clone(&app.state::<std::sync::Arc<Registry>>());
            ui_socket::start(app.handle().clone(), registry);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Familiar app");
}
