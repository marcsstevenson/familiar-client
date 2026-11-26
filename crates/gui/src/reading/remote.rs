// SPDX-License-Identifier: MIT OR Apache-2.0
//! Remote images: the `famimg` proxy that loads the ones the person allowed,
//! and the list of senders whose images are always shown.
//!
//! The message frame's CSP lets images load only from `data:` and this
//! scheme, so the webview never contacts a remote server itself. The proxy
//! sends no cookies and no referrer, and only answers with images.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use tauri::http::{Request, Response, StatusCode, header};

use super::sanitize::{percent_decode, proxy_token};

/// Largest remote image the proxy loads.
const MAX_IMAGE_BYTES: usize = 15 * 1024 * 1024;

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent("Mozilla/5.0 (compatible; Familiar)")
            .build()
            .expect("build the image client")
    })
}

fn reply(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "max-age=3600")
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/// The remote URL a proxy request names: `famimg://localhost/<token>/<encoded URL>`.
/// Without this run's token (see [`proxy_token`]) there is none.
pub fn target(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix('/')?
        .strip_prefix(proxy_token())?
        .strip_prefix('/')?;
    let url = percent_decode(rest);
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://")).then_some(url)
}

/// Answers one `famimg` request.
pub async fn proxy(request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some(url) = target(request.uri().path()) else {
        return reply(
            StatusCode::BAD_REQUEST,
            "text/plain",
            b"not an image URL".to_vec(),
        );
    };
    let got = async {
        let resp = client().get(&url).send().await?.error_for_status()?;
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        // Stop reading once past the limit, whatever Content-Length said.
        let mut resp = resp;
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_IMAGE_BYTES {
                break;
            }
        }
        Ok::<_, reqwest::Error>((ct, body))
    }
    .await;
    match got {
        Ok((ct, body)) if body.len() > MAX_IMAGE_BYTES => {
            eprintln!("familiar: remote image over {MAX_IMAGE_BYTES} bytes skipped: {url} ({ct})");
            reply(StatusCode::PAYLOAD_TOO_LARGE, "text/plain", Vec::new())
        }
        // Only images: a page or script answer is refused.
        Ok((ct, _)) if ct.starts_with("text/") || ct.contains("javascript") => {
            reply(StatusCode::UNSUPPORTED_MEDIA_TYPE, "text/plain", Vec::new())
        }
        Ok((ct, body)) => {
            let ct = if ct.starts_with("image/") {
                ct
            } else {
                "application/octet-stream".into()
            };
            reply(StatusCode::OK, &ct, body)
        }
        Err(e) => reply(
            StatusCode::BAD_GATEWAY,
            "text/plain",
            e.to_string().into_bytes(),
        ),
    }
}

/// The active config for the reader's trusted remote-image senders.
pub struct TrustedSenders {
    file: Option<PathBuf>,
}

#[derive(serde::Deserialize)]
struct ReadingFile {
    image_senders: BTreeSet<String>,
}

impl TrustedSenders {
    /// Imports the old per-device sender list when the config has no explicit list.
    pub fn load(file: Option<PathBuf>, legacy: Option<PathBuf>) -> Self {
        if let (Some(config), Some(old)) = (&file, legacy)
            && let Err(e) =
                familiar_core::folder_prefs::import_legacy(config, &old, |doc, old: ReadingFile| {
                    let path = ["reading", "image_senders"];
                    if !doc.contains(&path) {
                        doc.set_serialized(&path, &old.image_senders)?;
                    }
                    Ok(())
                })
        {
            eprintln!("familiar: importing trusted image senders: {e}");
        }
        Self { file }
    }

    /// Whether the config allows this sender's images.
    pub fn contains(&self, addr: &str) -> bool {
        !addr.trim().is_empty()
            && self
                .file
                .as_ref()
                .and_then(|f| familiar_config::load(f).ok())
                .is_some_and(|c| {
                    c.reading
                        .image_senders
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case(addr.trim()))
                })
    }

    /// Adds or removes an address in the current config.
    pub fn set(&self, addr: &str, trusted: bool) -> Result<(), String> {
        let a = addr.trim().to_ascii_lowercase();
        if a.is_empty() {
            return Err("the sender has no address".into());
        }
        let path = self.file.as_ref().ok_or("no config path")?;
        familiar_config::ConfigDocument::update(path, |doc| {
            let mut set = doc
                .check()
                .config
                .ok_or_else(|| familiar_config::edit::EditError("invalid config".into()))?
                .reading
                .image_senders;
            set.retain(|s| !s.eq_ignore_ascii_case(&a));
            if trusted {
                set.insert(a);
            }
            doc.set_serialized(&["reading", "image_senders"], &set)
        })
        .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reading::sanitize::percent_encode;

    #[test]
    fn proxy_targets_only_web_urls() {
        let u = "https://cdn.example.com/a b.png?x=1&y=2";
        let t = proxy_token();
        assert_eq!(
            target(&format!("/{t}/{}", percent_encode(u))).as_deref(),
            Some(u)
        );
        assert_eq!(target(&format!("/{t}/file%3A%2F%2F%2Fetc%2Fpasswd")), None);
        // A URL a message wrote itself has no token.
        assert_eq!(target(&format!("/{}", percent_encode(u))), None);
        assert_eq!(target(&format!("/wrong/{}", percent_encode(u))), None);
        assert_eq!(target("/"), None);
    }

    #[test]
    fn trusted_senders_persist() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        let legacy = dir.path().join("reading.json");
        std::fs::write(&legacy, r#"{"image_senders":["imported@example.org"]}"#).unwrap();
        let t = TrustedSenders::load(Some(file.clone()), Some(legacy.clone()));
        assert!(t.contains("imported@example.org"));
        assert!(!t.contains("news@shop.example"));
        t.set("News@Shop.example", true).unwrap();
        assert!(t.contains("news@shop.example"));
        let again = TrustedSenders::load(Some(file.clone()), Some(legacy.clone()));
        assert!(again.contains("NEWS@shop.example"));
        again.set("news@shop.example", false).unwrap();
        assert!(!TrustedSenders::load(Some(file), None).contains("news@shop.example"));
    }
}
