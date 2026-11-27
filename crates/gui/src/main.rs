// SPDX-License-Identifier: MIT OR Apache-2.0
//! Familiar desktop app: a Tauri 2 window running the SolidJS front end in `ui/`.
//!
//! The front end reaches the engine through one IPC command, `run`, which calls
//! the same command registry as `fam` and the MCP server, so every surface
//! shares one implementation. famd is started in the background if it isn't
//! running. This is the only crate allowed to depend on a GUI or webview library.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod compose;
mod config_watch;
mod daemon;
mod icon;
mod list;
mod onboarding;
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
const NOT_FROM_GUI: &[&str] = &["draft.edit", "mcp.serve"];

/// What the main window remembers between runs: where it was, how big, and
/// whether it was maximised. Saved when the window closes or the app exits.
/// If the monitor it was on is gone, the window opens where the OS puts it.
pub(crate) const WINDOW_STATE: tauri_plugin_window_state::StateFlags =
    tauri_plugin_window_state::StateFlags::POSITION
        .union(tauri_plugin_window_state::StateFlags::SIZE)
        .union(tauri_plugin_window_state::StateFlags::MAXIMIZED);

/// One saved search.
#[derive(Debug, Serialize)]
struct View {
    name: String,
    query: String,
}

/// Facts the front end needs before it draws anything.
#[derive(Debug, Serialize)]
struct UiInfo {
    /// Store whose local IDs the desktop session refers to.
    session_store: Option<std::path::PathBuf>,
    version: &'static str,
    /// `windows`, `macos` or `linux`: picks the title bar style.
    platform: &'static str,
    /// The `[app] theme` from config.toml: `system`, `light` or `dark`.
    theme: &'static str,
    desktop: familiar_config::model::Desktop,
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
    /// The accounts with `spam_check` or `spam_auto`: their messages' menu offers
    /// "Check for spam or scam".
    spam_accounts: Vec<String>,
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
    let spam_accounts = config
        .as_ref()
        .map(|c| {
            c.accounts
                .iter()
                .filter(|(_, a)| a.spam_check || a.spam_auto)
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    let (theme, desktop, keys, views, processed) = match config {
        Some(c) => (
            theme_name(c.app.theme),
            c.desktop,
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
        None => (
            "system",
            Default::default(),
            Default::default(),
            Vec::new(),
            Default::default(),
        ),
    };
    UiInfo {
        session_store: familiar_core::daemon::DaemonPaths::resolve()
            .ok()
            .map(|p| p.store),
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        theme,
        desktop,
        keys,
        views,
        processed,
        ai_tool,
        ai_accounts,
        spam_accounts,
        config_error,
        bench: std::env::var_os(BENCH_ENV).is_some(),
    }
}

/// Writes only the desktop keys changed by a gesture. During import an
/// explicit config value always wins over an older webview preference.
#[tauri::command]
fn save_preferences(changes: serde_json::Map<String, Value>, import: bool) -> Result<(), String> {
    let path = familiar_core::folder_prefs::config_path().map_err(|e| e.message)?;
    write_preferences(&path, changes, import)
}

fn write_preferences(
    path: &std::path::Path,
    changes: serde_json::Map<String, Value>,
    import: bool,
) -> Result<(), String> {
    familiar_config::ConfigDocument::update(path, |doc| {
        for (key, value) in changes {
            let section = if key == "theme" { "app" } else { "desktop" };
            let path = [section, key.as_str()];
            if import && doc.contains(&path) {
                continue;
            }
            if value.is_null() {
                doc.remove(&path);
            } else {
                doc.set_serialized(&path, &value)?;
            }
        }
        Ok(())
    })
    .map_err(|e| e.to_string())
}

/// Names the file the bench results go to.
const BENCH_ENV: &str = "FAMILIAR_BENCH_OUT";

/// When set with `FAMILIAR_BENCH_OUT`, the app stays open after reporting,
/// so a harness can measure its idle memory.
const BENCH_HOLD_ENV: &str = "FAMILIAR_BENCH_HOLD";

/// When `main` started, for cold start timing.
static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Shows a mail overlay on the main window's taskbar button while `unread`
/// is above zero, and clears it at zero. Windows only; elsewhere it does
/// nothing.
#[tauri::command]
fn set_unread_badge(app: tauri::AppHandle, unread: u32) {
    #[cfg(target_os = "windows")]
    {
        let Some(window) = app.get_webview_window("main") else {
            return;
        };
        // 32x32 raw RGBA rendered from icons/unread-overlay.svg (raw so the
        // app needs no PNG decoder): `magick unread-overlay.png -depth 8
        // rgba:unread-overlay.rgba` after rendering the SVG with a
        // transparent background.
        let icon = (unread > 0).then(|| {
            tauri::image::Image::new(include_bytes!("../icons/unread-overlay.rgba"), 32, 32)
        });
        if let Err(e) = window.set_overlay_icon(icon) {
            eprintln!("familiar: set taskbar overlay: {e}");
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (app, unread);
}

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

/// Opens config.toml in the system's default app for `.toml` files, creating it from
/// the starting template first if there is none. Windows
/// often has none, so it falls back to Notepad there. Returns the path.
#[tauri::command]
fn open_config() -> Result<String, CommandError> {
    let (path, _) = familiar_config::paths::resolve(None)
        .ok_or_else(|| CommandError::internal("no home folder to look for config.toml in"))?;
    // No config yet: start one from the commented template, so the cog always
    // opens something to edit.
    familiar_config::template::create(&path)
        .map_err(|e| CommandError::internal(format!("couldn't create {}: {e}", path.display())))?;
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

/// Whether every corner of the window is on some monitor. A saved size can be
/// too big for where the window lands when its old monitor is gone, so it would
/// hang off the edge; a window spread over two monitors side by side still counts.
fn on_screen(window: &tauri::WebviewWindow) -> tauri::Result<bool> {
    // Windows draws an invisible resize border of a few pixels outside the
    // visible frame, so allow a little overhang.
    const SLACK: i32 = 16;
    let pos = window.outer_position()?;
    let size = window.outer_size()?;
    let (w, h) = (
        i32::try_from(size.width).unwrap_or(i32::MAX),
        i32::try_from(size.height).unwrap_or(i32::MAX),
    );
    let monitors = window.available_monitors()?;
    let on_some_monitor = |x: i32, y: i32| {
        monitors.iter().any(|m| {
            let (mp, ms) = (m.position(), m.size());
            x >= mp.x
                && y >= mp.y
                && i64::from(x) < i64::from(mp.x) + i64::from(ms.width)
                && i64::from(y) < i64::from(mp.y) + i64::from(ms.height)
        })
    };
    let (left, top) = (pos.x.saturating_add(SLACK), pos.y.saturating_add(SLACK));
    let (right, bottom) = (
        pos.x.saturating_add(w).saturating_sub(SLACK),
        pos.y.saturating_add(h).saturating_sub(SLACK),
    );
    Ok([(left, top), (right, top), (left, bottom), (right, bottom)]
        .into_iter()
        .all(|(x, y)| on_some_monitor(x, y)))
}

fn main() {
    STARTED.get_or_init(std::time::Instant::now);
    tauri::Builder::default()
        .manage(std::sync::Arc::new(Registry::builtin()))
        .manage(daemon::Shared::default())
        .on_window_event(|window, event| {
            // Closing the main window ends the session with its readers and
            // editors still open. Their working state is saved as it changes.
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                window.app_handle().exit(0);
            }
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(WINDOW_STATE)
                // Compose windows are numbered and short-lived; only the main
                // window keeps its place.
                .with_filter(|label| label == "main")
                .build(),
        )
        .manage(reading::Reading::load())
        .register_asynchronous_uri_scheme_protocol("famimg", |_ctx, request, responder| {
            tauri::async_runtime::spawn(async move {
                responder.respond(reading::remote::proxy(request).await);
            });
        })
        .manage(std::sync::Arc::new(list::Lists::default()))
        .invoke_handler(tauri::generate_handler![
            ui_info,
            save_preferences,
            open_config,
            onboarding::onboarding_state,
            config_watch::restart_app,
            run,
            daemon_state,
            daemon::retry_sync,
            reading::reading_open,
            reading::reading_spam_status,
            reading::open_reader,
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
            bench_elapsed_ms,
            icon::set_accent_icon,
            set_unread_badge
        ])
        .setup(|app| {
            let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Familiar")
                .inner_size(1280.0, 800.0)
                .min_inner_size(720.0, 420.0)
                // Hidden until the window-state plugin has moved it back to
                // where it was last time, so it doesn't jump on screen.
                .visible(false)
                // Where it opens when there is no saved place, or the saved
                // monitor is gone.
                .center()
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
            let window = builder.build()?;
            if !window.is_maximized()? && !on_screen(&window)? {
                window.center()?;
            }
            window.show()?;
            let handle = app.handle().clone();
            let shared = app.state::<daemon::Shared>().inner().clone();
            std::thread::spawn(move || daemon::keep_running(&handle, &shared));
            let handle = app.handle().clone();
            std::thread::spawn(move || list::watch_store(&handle));
            let registry = std::sync::Arc::clone(&app.state::<std::sync::Arc<Registry>>());
            ui_socket::start(app.handle().clone(), registry);
            config_watch::watch(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Familiar app");
}

#[cfg(test)]
mod preference_tests {
    use super::write_preferences;
    use serde_json::json;

    #[test]
    fn import_respects_toml_and_changes_round_trip_without_touching_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "version = 1
[app]
theme = 'dark' # mine
",
        )
        .unwrap();
        let change = |v: serde_json::Value, import| {
            write_preferences(&path, v.as_object().unwrap().clone(), import)
        };
        change(
            json!({"theme":"light", "accent":"#99d9ea", "neon":true}),
            true,
        )
        .unwrap();
        change(
            json!({"panes":{"folders":299,"list":500}, "calendar_view":"Day"}),
            false,
        )
        .unwrap();
        let cfg = familiar_config::load(&path).unwrap();
        assert_eq!(cfg.desktop.accent.as_deref(), Some("#99d9ea"));
        assert_eq!(cfg.desktop.neon, Some(true));
        assert_eq!(cfg.desktop.panes.unwrap().folders, 299);
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(before.contains("theme = 'dark' # mine"));
        assert!(change(json!({"accent":"#oops"}), false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        change(json!({"accent":null}), false).unwrap();
        assert!(
            familiar_config::load(&path)
                .unwrap()
                .desktop
                .accent
                .is_none()
        );
    }
}
