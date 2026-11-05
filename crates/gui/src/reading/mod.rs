// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reading pane's IPC commands (WP 3.3).
//!
//! `reading_open` returns a message's header and its body as a sanitised
//! document (see [`sanitize`]). The front end calls it twice when it needs
//! to: first with `local` so the header shows at once, then without, which
//! downloads the body and the message's inline images through famd. Opening
//! a message also prefetches the next ones in its folder in the background.
//!
//! Attachments are read from the server when asked for, through
//! `familiar_core::commands::attachments`, the same code as
//! `fam mail attachments`.

pub mod remote;
pub mod sanitize;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use base64::Engine as _;
use familiar_core::CommandError;
use familiar_core::commands::attachments::{self, AttachmentInfo, AttachmentKind};
use familiar_core::commands::mail::{
    BodyFormat, BodyType, MailFetchInput, MailFetchOutput, fetch_at,
};
use familiar_core::daemon::DaemonPaths;
use familiar_store::{Cursor, MessageId, Reader};
use serde::Serialize;

use remote::TrustedSenders;

/// How many of the following messages are prefetched when one is opened.
const PREFETCH: usize = 2;
/// Inline image sets kept in memory before the cache starts over.
const CID_CACHE: usize = 64;
/// Largest attachment previewed inside the app.
const MAX_PREVIEW_BYTES: usize = 20 * 1024 * 1024;

type CidMap = Arc<HashMap<String, String>>;

/// The reading pane's state, managed by Tauri.
pub struct Reading {
    senders: TrustedSenders,
    /// Messages whose images were allowed this session.
    shown: Mutex<HashSet<i64>>,
    /// Inline images by message.
    cid: Mutex<HashMap<i64, CidMap>>,
    prefetching: Arc<AtomicBool>,
}

impl Reading {
    /// Loads the trusted senders from the data directory.
    pub fn load() -> Arc<Self> {
        let file = DaemonPaths::resolve()
            .ok()
            .map(|p| p.data_dir.join("reading.json"));
        Arc::new(Self {
            senders: TrustedSenders::load(file),
            shown: Mutex::default(),
            cid: Mutex::default(),
            prefetching: Arc::default(),
        })
    }

    fn cached_cid(&self, message: i64) -> Option<CidMap> {
        self.cid.lock().ok()?.get(&message).cloned()
    }

    fn keep_cid(&self, message: i64, map: CidMap) {
        if let Ok(mut c) = self.cid.lock() {
            if c.len() >= CID_CACHE {
                c.clear();
            }
            c.insert(message, map);
        }
    }
}

/// Downloads a message's inline images as `data:` URIs by Content-ID.
/// A failure leaves the images out and is logged; the body still shows.
/// The flag is false when something failed, so the set isn't cached.
fn load_cid(paths: &DaemonPaths, message: i64) -> (CidMap, bool) {
    let mut map = HashMap::new();
    let mut complete = true;
    let list = match attachments::list_at(paths, message) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "familiar: inline images of message {message}: {}",
                e.message
            );
            return (Arc::new(map), false);
        }
    };
    for a in list
        .iter()
        .filter(|a| a.is_inline && a.kind == AttachmentKind::File)
    {
        match attachments::content_at(paths, message, &a.id) {
            Ok(c) => {
                if let Some(cid) = &c.content_id {
                    map.insert(
                        sanitize::cid_key(cid),
                        data_uri(&c.info.content_type, &c.bytes),
                    );
                }
            }
            Err(e) => {
                complete = false;
                eprintln!(
                    "familiar: inline image {} of message {message}: {}",
                    a.name, e.message
                );
            }
        }
    }
    (Arc::new(map), complete)
}

fn data_uri(content_type: &str, bytes: &[u8]) -> String {
    let ct = if content_type.is_empty() {
        "application/octet-stream"
    } else {
        content_type
    };
    format!(
        "data:{ct};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// What the reading pane shows for one message.
#[derive(Debug, Serialize)]
pub struct ReadView {
    /// The header, as `fam mail fetch` returns it (without the raw body).
    pub message: MailFetchOutput,
    /// The sanitised document, or None when the body isn't on this device yet.
    pub html: Option<String>,
    /// Remote images that were blocked.
    pub blocked_images: usize,
    /// Remote images are shown (for this message or its sender).
    pub images_shown: bool,
    /// The sender's images are always shown.
    pub sender_trusted: bool,
    /// The body refers to inline images that haven't been downloaded yet.
    pub inline_pending: bool,
    /// Milliseconds spent in the app's backend.
    pub took_ms: f64,
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, CommandError> + Send + 'static,
) -> Result<T, CommandError> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| CommandError::internal(format!("reading task failed: {e}")))?
}

/// Opens a message. `local` reads only what is on this device; `show_images`
/// allows remote images for this message for the rest of the session.
#[tauri::command]
pub async fn reading_open(
    message: i64,
    local: bool,
    show_images: bool,
    state: tauri::State<'_, Arc<Reading>>,
) -> Result<ReadView, CommandError> {
    let state = Arc::clone(&state);
    blocking(move || {
        let start = Instant::now();
        let paths = DaemonPaths::resolve()?;
        let mut out = fetch_at(
            &paths,
            &MailFetchInput {
                message,
                format: BodyFormat::Html,
                local,
            },
        )?;
        if show_images && let Ok(mut s) = state.shown.lock() {
            s.insert(message);
        }
        let sender_trusted = state.senders.contains(&out.header.from.addr);
        let images_shown = sender_trusted || state.shown.lock().is_ok_and(|s| s.contains(&message));
        let body = out.body.take();
        let is_html = out.body_type == Some(BodyType::Html);
        let mut inline_pending = false;
        let (html, blocked_images) = match &body {
            None => (None, 0),
            Some(b) => {
                let cid = if is_html && sanitize::mentions_cid(b) {
                    match state.cached_cid(message) {
                        Some(c) => c,
                        None if local => {
                            inline_pending = true;
                            CidMap::default()
                        }
                        None => {
                            let (c, complete) = load_cid(&paths, message);
                            if complete {
                                state.keep_cid(message, Arc::clone(&c));
                            }
                            c
                        }
                    }
                } else {
                    CidMap::default()
                };
                let clean = sanitize::sanitize(&sanitize::Input {
                    body: b,
                    is_html,
                    remote_images: images_shown,
                    cid,
                });
                (Some(clean.html), clean.blocked_images)
            }
        };
        prefetch_after(&state, &paths, message);
        Ok(ReadView {
            message: out,
            html,
            blocked_images,
            images_shown,
            sender_trusted,
            inline_pending,
            took_ms: start.elapsed().as_secs_f64() * 1000.0,
        })
    })
    .await
}

/// Warms the next messages in the folder: downloads bodies that aren't on
/// this device and their inline images. One prefetch runs at a time.
fn prefetch_after(state: &Arc<Reading>, paths: &DaemonPaths, message: i64) {
    if state.prefetching.swap(true, Ordering::AcqRel) {
        return;
    }
    let state = Arc::clone(state);
    let paths = paths.clone();
    std::thread::spawn(move || {
        let next = || -> Option<Vec<familiar_store::Message>> {
            let r = Reader::open(&paths.store).ok()?;
            let m = r.message(MessageId(message)).ok()??;
            r.messages(
                m.folder,
                Some(Cursor {
                    received_at: m.received_at,
                    id: m.id.0,
                }),
                PREFETCH,
            )
            .ok()
        };
        for m in next().unwrap_or_default() {
            let id = m.id.0;
            let body = match fetch_at(
                &paths,
                &MailFetchInput {
                    message: id,
                    format: BodyFormat::Html,
                    local: false,
                },
            ) {
                Ok(o) => o.body,
                Err(e) => {
                    eprintln!("familiar: prefetch of message {id}: {}", e.message);
                    break;
                }
            };
            if body.as_deref().is_some_and(sanitize::mentions_cid) && state.cached_cid(id).is_none()
            {
                if let (c, true) = load_cid(&paths, id) {
                    state.keep_cid(id, c);
                }
            }
        }
        state.prefetching.store(false, Ordering::Release);
    });
}

/// Always show (or stop showing) remote images from `address`.
#[tauri::command]
pub fn reading_trust_sender(
    address: String,
    trusted: bool,
    state: tauri::State<'_, Arc<Reading>>,
) -> Result<(), CommandError> {
    state
        .senders
        .set(&address, trusted)
        .map_err(|e| CommandError::internal(format!("save reading.json: {e}")))
}

/// A message's attachments, without inline images.
#[tauri::command]
pub async fn reading_attachments(message: i64) -> Result<Vec<AttachmentInfo>, CommandError> {
    blocking(move || {
        let paths = DaemonPaths::resolve()?;
        let mut list = attachments::list_at(&paths, message)?;
        list.retain(|a| !a.is_inline);
        Ok(list)
    })
    .await
}

/// An attachment shown inside the app.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Preview {
    /// An image, as a `data:` URI.
    Image {
        /// The `data:` URI.
        src: String,
    },
    /// Text, shown as is.
    Text {
        /// The text.
        text: String,
    },
    /// No preview for this type; it can still be saved.
    None,
}

fn previewable(a: &AttachmentInfo) -> bool {
    let ct = a.content_type.to_ascii_lowercase();
    let name = a.name.to_ascii_lowercase();
    // SVG can carry script; it is only ever shown through <img>, which never runs it.
    ct.starts_with("image/")
        || ct.starts_with("text/plain")
        || ct == "text/csv"
        || [".txt", ".csv", ".log", ".md", ".json"]
            .iter()
            .any(|e| name.ends_with(e))
}

/// Previews an attachment: images and plain text only.
#[tauri::command]
pub async fn reading_preview(message: i64, attachment: String) -> Result<Preview, CommandError> {
    blocking(move || {
        let paths = DaemonPaths::resolve()?;
        let c = attachments::content_at(&paths, message, &attachment)?;
        if !previewable(&c.info) || c.bytes.len() > MAX_PREVIEW_BYTES {
            return Ok(Preview::None);
        }
        if c.info
            .content_type
            .to_ascii_lowercase()
            .starts_with("image/")
        {
            Ok(Preview::Image {
                src: data_uri(&c.info.content_type, &c.bytes),
            })
        } else {
            Ok(Preview::Text {
                text: String::from_utf8_lossy(&c.bytes).into_owned(),
            })
        }
    })
    .await
}

/// Saves an attachment into the Downloads folder (never over an existing
/// file) and returns the path.
#[tauri::command]
pub async fn reading_save(message: i64, attachment: String) -> Result<String, CommandError> {
    blocking(move || {
        let paths = DaemonPaths::resolve()?;
        let dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .ok_or_else(|| CommandError::internal("no Downloads folder"))?;
        let c = attachments::content_at(&paths, message, &attachment)?;
        let path = attachments::save_unique(&dir, &c.info.name, &c.bytes)?;
        Ok(path.display().to_string())
    })
    .await
}

/// Opens a link from a message in the system browser or mail handler.
#[tauri::command]
pub fn reading_open_link(url: String) -> Result<(), CommandError> {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:"))
    {
        return Err(CommandError::usage(format!(
            "not a web or mail link: {url}"
        )));
    }
    webbrowser::open(url.trim())
        .map_err(|e| CommandError::internal(format!("couldn't open {url}: {e}")))
}
