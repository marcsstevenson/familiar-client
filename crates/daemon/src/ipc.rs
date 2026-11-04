// SPDX-License-Identifier: MIT OR Apache-2.0
//! The local socket (Unix) or named pipe (Windows) that `fam` and the GUI
//! talk to. One JSON-RPC request per line, one response per line; see
//! [`familiar_core::daemon`] for the protocol.
//!
//! Only the user who started `famd` can connect: on Unix the socket file is
//! mode 0600; on Windows the pipe refuses remote clients and keeps the
//! default security, which gives write access only to its creator, SYSTEM
//! and administrators.

use std::path::Path;
use std::sync::Arc;

use familiar_core::CommandError;
use familiar_core::daemon::{Request, Response};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::watch;

use crate::service::Service;

/// Longest request line accepted.
const MAX_LINE: usize = 1 << 20;

async fn serve_conn<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    svc: Arc<Service>,
    mut shutdown: watch::Receiver<bool>,
) {
    let (r, mut w) = tokio::io::split(stream);
    let mut lines = BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        // An open connection must not keep the daemon (and its writer lock) alive.
        let read = tokio::select! {
            r = lines.read_line(&mut line) => r,
            _ = shutdown.changed() => return,
        };
        match read {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let resp = if line.len() > MAX_LINE {
            Response::err(0, &CommandError::usage("request too long"))
        } else {
            match serde_json::from_str::<Request>(&line) {
                Ok(req) => match svc.handle(&req.method, req.params).await {
                    Ok(v) => Response::ok(req.id, v),
                    Err(e) => Response::err(req.id, &e),
                },
                Err(e) => Response {
                    jsonrpc: "2.0".into(),
                    id: serde_json::from_str::<Value>(&line)
                        .ok()
                        .and_then(|v| v.get("id").and_then(Value::as_u64))
                        .unwrap_or(0),
                    result: None,
                    error: Some(familiar_core::daemon::RpcError {
                        code: -32700,
                        message: format!("bad request: {e}"),
                        data: None,
                    }),
                },
            }
        };
        let mut out = serde_json::to_string(&resp).unwrap_or_default();
        out.push('\n');
        if w.write_all(out.as_bytes()).await.is_err() || w.flush().await.is_err() {
            return;
        }
    }
}

/// Listens on `socket` until `shutdown` turns true.
#[cfg(unix)]
pub async fn listen(
    socket: &Path,
    svc: Arc<Service>,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // A socket file left by a daemon that crashed. We hold the store's
    // writer lock, so no other famd is using it.
    let _ = std::fs::remove_file(socket);
    let listener = tokio::net::UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    loop {
        tokio::select! {
            r = listener.accept() => {
                if let Ok((stream, _)) = r {
                    tokio::spawn(serve_conn(stream, svc.clone(), shutdown.clone()));
                }
            }
            _ = shutdown.changed() => break,
        }
        if *shutdown.borrow() {
            break;
        }
    }
    let _ = std::fs::remove_file(socket);
    Ok(())
}

/// Listens on the pipe `socket` until `shutdown` turns true.
#[cfg(windows)]
pub async fn listen(
    socket: &Path,
    svc: Arc<Service>,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let name = socket.as_os_str();
    // first_pipe_instance fails if another process already owns the name.
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(name)?;
    loop {
        tokio::select! {
            r = server.connect() => {
                r?;
                let next = ServerOptions::new().reject_remote_clients(true).create(name)?;
                let conn = std::mem::replace(&mut server, next);
                tokio::spawn(serve_conn(conn, svc.clone(), shutdown.clone()));
            }
            _ = shutdown.changed() => break,
        }
        if *shutdown.borrow() {
            break;
        }
    }
    Ok(())
}
