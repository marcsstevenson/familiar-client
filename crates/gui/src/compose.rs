// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the compose window needs beyond the command registry: the sending
//! identities and their signatures from config.toml, compose windows of their
//! own, and the files dropped on or picked for a message.
//!
//! Drafts themselves are saved with the registry's `draft.*` commands, the
//! same ones `fam draft` runs, so a draft written here opens in the CLI and
//! the other way round.

use familiar_core::compose::{AttachmentData, MAX_ATTACHMENT_BYTES, inline_images};
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicU32, Ordering};
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

/// A sending identity: an account in config.toml.
#[derive(Debug, Serialize)]
pub struct Identity {
    /// Its name in config.toml, as `--account` takes it.
    key: String,
    email: String,
    display_name: Option<String>,
    /// Signature HTML for new messages, with its images as `data:` URIs so the
    /// window can show them.
    signature_new: Option<String>,
    /// Signature HTML for replies and forwards.
    signature_reply: Option<String>,
}

/// Compose settings from config.toml.
#[derive(Debug, Serialize)]
pub struct ComposeInfo {
    identities: Vec<Identity>,
    /// The undo-send delay, in seconds.
    send_delay_seconds: u32,
    /// Why config.toml could not be read, if it couldn't.
    config_error: Option<String>,
}

#[tauri::command]
pub fn compose_info() -> ComposeInfo {
    let mut info = ComposeInfo {
        identities: Vec::new(),
        send_delay_seconds: 10,
        config_error: None,
    };
    let Some((path, _)) = familiar_config::paths::resolve(None) else {
        return info;
    };
    if !path.exists() {
        return info;
    }
    let config = match familiar_config::load(&path) {
        Ok(c) => c,
        Err(e) => {
            info.config_error = Some(format!("{}: {e}", path.display()));
            return info;
        }
    };
    info.send_delay_seconds = config.app.send_delay_seconds;
    for (key, a) in config.accounts {
        let sig = a.signature.unwrap_or_default();
        info.identities.push(Identity {
            key,
            email: a.email,
            display_name: a.display_name,
            signature_new: sig.new.as_deref().map(showable),
            signature_reply: sig.reply.as_deref().map(showable),
        });
    }
    info
}

/// Signature HTML with images from local files turned into `data:` URIs.
/// Saving turns them back into the same inline attachments `fam` makes.
fn showable(html: &str) -> String {
    match inline_images(html) {
        Ok((mut out, images)) => {
            for a in images {
                if let Some(cid) = &a.content_id {
                    out = out.replace(
                        &format!("cid:{cid}"),
                        &format!("data:{};base64,{}", a.content_type, a.content_b64),
                    );
                }
            }
            out
        }
        // A missing or oversized image: show the signature as written.
        Err(_) => html.to_string(),
    }
}

static NEXT_WINDOW: AtomicU32 = AtomicU32::new(1);

/// Opens a compose window. `spec` says what to write (see ui/src/compose/spec.ts)
/// and is handed to the page in its URL.
#[tauri::command]
pub fn open_compose(app: AppHandle, spec: Value) -> Result<(), String> {
    let n = NEXT_WINDOW.fetch_add(1, Ordering::Relaxed);
    let json = spec.to_string();
    let url = format!("index.html#compose={}", percent_encode(&json));
    let builder =
        WebviewWindowBuilder::new(&app, format!("compose-{n}"), WebviewUrl::App(url.into()))
            .title("New message")
            .inner_size(820.0, 720.0)
            .min_inner_size(520.0, 400.0);
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.decorations(false);
    builder.build().map(|_| ()).map_err(|e| e.to_string())
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A file dropped on or picked for a message.
#[derive(Debug, Serialize)]
pub struct FileInfo {
    path: String,
    name: String,
    size: u64,
    /// For an image of up to 3 MB: a `data:` URI, so it can go in the body.
    image: Option<String>,
    /// Why it can't be attached, if it can't.
    problem: Option<String>,
}

#[tauri::command]
pub fn file_info(paths: Vec<String>) -> Vec<FileInfo> {
    paths.into_iter().map(|p| describe(&p)).collect()
}

fn describe(path: &str) -> FileInfo {
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let mut info = FileInfo {
        path: path.to_string(),
        name,
        size: 0,
        image: None,
        problem: None,
    };
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => info.problem = Some("folders can't be attached".into()),
        Ok(m) => {
            info.size = m.len();
            if m.len() > MAX_ATTACHMENT_BYTES {
                info.problem = Some("attachments are limited to 3 MB for now".into());
            } else if is_image(&info.name) {
                if let Ok(a) = AttachmentData::read(path, true) {
                    info.image = Some(format!("data:{};base64,{}", a.content_type, a.content_b64));
                }
            }
        }
        Err(e) => info.problem = Some(e.to_string()),
    }
    info
}

fn is_image(name: &str) -> bool {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    )
}

/// Shows the system's file picker and describes the files chosen.
#[tauri::command]
pub async fn pick_files(app: AppHandle) -> Vec<FileInfo> {
    let picked =
        tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_files())
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
    picked
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .map(|p| describe(&p.to_string_lossy()))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn signature_images_from_files_become_data_uris() {
        let dir = std::env::temp_dir().join(format!("fam-gui-sig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let logo = dir.join("logo.png");
        std::fs::write(&logo, b"\x89PNG").unwrap();
        let html = format!("<p>Marc</p><img src=\"{}\">", logo.display());
        let out = super::showable(&html);
        assert!(
            out.contains("src=\"data:image/png;base64,iVBORw==\""),
            "{out}"
        );
        // Embedded images and web images are left as they are.
        let html = "<img src=\"data:image/gif;base64,R0lG\"><img src=\"https://x/a.png\">";
        assert_eq!(super::showable(html), html);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spec_is_url_safe() {
        assert_eq!(super::percent_encode("{\"a\":1}"), "%7B%22a%22%3A1%7D");
    }
}
