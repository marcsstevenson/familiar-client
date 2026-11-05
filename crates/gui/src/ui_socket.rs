// SPDX-License-Identifier: MIT OR Apache-2.0
//! The app's own local socket, for `fam ui` commands. Same line protocol as
//! famd (one JSON-RPC request per line). Each request is checked against the
//! store here, then handed to the front end as the `ui_command` event.

use std::path::Path;
use std::sync::Arc;

use familiar_core::commands::ui::{
    METHOD_UI_FOLDER, METHOD_UI_OPEN, UiDone, UiFolderInput, UiOpenInput,
};
use familiar_core::daemon::{DaemonPaths, Request, Response, ui_socket_for};
use familiar_core::registry::Registry;
use familiar_core::{CommandError, ErrorKind};
use familiar_store::{MessageId, Reader};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// What the front end is asked to show.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UiCommand {
    /// Show the message's `folder` and select `message`.
    Open { folder: i64, message: i64 },
    /// Show `folder` (None: all inboxes).
    Folder { folder: Option<i64> },
}

/// Starts listening. Errors are logged; the app works without the socket.
pub fn start(app: AppHandle, registry: Arc<Registry>) {
    tauri::async_runtime::spawn(async move {
        let socket = match DaemonPaths::resolve() {
            Ok(p) => ui_socket_for(&p.data_dir),
            Err(e) => return eprintln!("familiar: no ui socket: {}", e.message),
        };
        if let Err(e) = listen(&socket, app, registry).await {
            eprintln!("familiar: ui socket {}: {e}", socket.display());
        }
    });
}

async fn serve_conn<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    app: AppHandle,
    registry: Arc<Registry>,
) {
    let (r, mut w) = tokio::io::split(stream);
    let mut lines = BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        match lines.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let (app, registry) = (app.clone(), Arc::clone(&registry));
                let out = tauri::async_runtime::spawn_blocking(move || {
                    handle(&app, &registry, &req.method, req.params)
                })
                .await
                .unwrap_or_else(|e| Err(CommandError::internal(e.to_string())));
                match out {
                    Ok(done) => Response::ok(req.id, json!(done)),
                    Err(e) => Response::err(req.id, &e),
                }
            }
            Err(e) => Response::err(0, &CommandError::usage(format!("bad request: {e}"))),
        };
        let mut out = serde_json::to_string(&resp).unwrap_or_default();
        out.push('\n');
        if w.write_all(out.as_bytes()).await.is_err() || w.flush().await.is_err() {
            return;
        }
    }
}

fn handle(
    app: &AppHandle,
    registry: &Registry,
    method: &str,
    params: Value,
) -> Result<UiDone, CommandError> {
    let bad = |e: serde_json::Error| CommandError::usage(format!("bad input: {e}"));
    let (cmd, shown) = match method {
        METHOD_UI_OPEN => {
            let input: UiOpenInput = serde_json::from_value(params).map_err(bad)?;
            let store = DaemonPaths::resolve()?.store;
            let reader = Reader::open(&store)
                .map_err(|e| CommandError::new(ErrorKind::NotFound, e.to_string()))?;
            let m = reader
                .message(MessageId(input.message))
                .map_err(|e| CommandError::internal(e.to_string()))?
                .ok_or_else(|| {
                    CommandError::new(
                        ErrorKind::NotFound,
                        format!("message {} not found", input.message),
                    )
                })?;
            (
                UiCommand::Open {
                    folder: m.folder.0,
                    message: m.id.0,
                },
                format!("Opened message {}: {}", m.id.0, m.subject),
            )
        }
        METHOD_UI_FOLDER => {
            let input: UiFolderInput = serde_json::from_value(params).map_err(bad)?;
            match input.folder {
                None => (
                    UiCommand::Folder { folder: None },
                    "Showing all inboxes".into(),
                ),
                Some(name) => {
                    let (id, label) = find_folder(registry, &name, input.account.as_deref())?;
                    (
                        UiCommand::Folder { folder: Some(id) },
                        format!("Showing {label}"),
                    )
                }
            }
        }
        other => return Err(CommandError::usage(format!("unknown method `{other}`"))),
    };
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    app.emit("ui_command", cmd)
        .map_err(|e| CommandError::internal(e.to_string()))?;
    Ok(UiDone { shown })
}

/// Finds a folder by path, well-known name or name, like `fam mail list --folder`.
fn find_folder(
    registry: &Registry,
    name: &str,
    account: Option<&str>,
) -> Result<(i64, String), CommandError> {
    let out = registry
        .run("folder.list", json!({ "account": account }))?
        .json;
    let rows = out["folders"].as_array().cloned().unwrap_or_default();
    let field = |r: &Value, k: &str| r[k].as_str().unwrap_or_default().to_lowercase();
    let want = name.to_lowercase();
    let matches: Vec<&Value> = ["path", "well_known", "name"]
        .iter()
        .map(|k| {
            rows.iter()
                .filter(|r| field(r, k) == want)
                .collect::<Vec<_>>()
        })
        .find(|m| !m.is_empty())
        .unwrap_or_default();
    match matches.as_slice() {
        [] => Err(CommandError::new(
            ErrorKind::NotFound,
            format!("no folder `{name}`"),
        )),
        [r] => Ok((
            r["id"].as_i64().unwrap_or_default(),
            format!(
                "{}/{}",
                field(r, "account"),
                r["path"].as_str().unwrap_or_default()
            ),
        )),
        _ => Err(CommandError::usage(format!(
            "`{name}` is in more than one account; add --account"
        ))),
    }
}

#[cfg(unix)]
async fn listen(socket: &Path, app: AppHandle, registry: Arc<Registry>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // A socket left by an app that crashed; a second running app takes over.
    let _ = std::fs::remove_file(socket);
    let listener = tokio::net::UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    loop {
        if let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_conn(stream, app.clone(), Arc::clone(&registry)));
        }
    }
}

#[cfg(windows)]
async fn listen(socket: &Path, app: AppHandle, registry: Arc<Registry>) -> std::io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let name = socket.as_os_str();
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(name)?;
    loop {
        server.connect().await?;
        let next = ServerOptions::new()
            .reject_remote_clients(true)
            .create(name)?;
        let conn = std::mem::replace(&mut server, next);
        tokio::spawn(serve_conn(conn, app.clone(), Arc::clone(&registry)));
    }
}
