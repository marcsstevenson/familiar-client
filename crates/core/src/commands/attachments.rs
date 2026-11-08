// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam mail attachments` (WP 3.3): list a message's attachments and save them.
//!
//! Attachments aren't synced: they are read from Graph when asked for,
//! through famd's `graph.get`, which holds the tokens and paces requests. The
//! desktop app's reading pane uses the same functions for its attachment bar,
//! previews and the `cid:` images inside HTML bodies.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::mail::{open_store, store_err};
use crate::daemon::{Client, DaemonPaths, GraphGetParams, METHOD_GRAPH_GET};
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use familiar_store::MessageId;
use schemars::JsonSchema;

/// `fam mail attachments`.
pub struct MailAttachments;

/// Input for `fam mail attachments`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailAttachmentsInput {
    /// The message's local ID.
    #[schemars(extend("x-positional" = 0))]
    pub message: i64,
    /// Save the attachments into this folder. A file that already exists is
    /// kept and the new one gets a number, like `report (2).pdf`.
    #[serde(default)]
    pub save: Option<String>,
    /// Only the attachment with this name or ID (with --save).
    #[serde(default)]
    pub name: Option<String>,
    /// Include inline images (the ones shown inside the body).
    #[serde(default)]
    pub inline: bool,
}

/// What kind of attachment it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    /// A file: the only kind that can be saved or previewed.
    File,
    /// An attached Outlook item, such as a forwarded message or event.
    Item,
    /// A link to a file in OneDrive or SharePoint.
    Reference,
}

/// One attachment.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentInfo {
    /// Graph attachment ID.
    pub id: String,
    /// File name.
    pub name: String,
    /// MIME type, such as `application/pdf`.
    pub content_type: String,
    /// Size in bytes, as Graph reports it.
    pub size: u64,
    /// Shown inside the body (a `cid:` image) rather than in the list.
    pub is_inline: bool,
    /// File, item or reference.
    pub kind: AttachmentKind,
    /// Where it was saved, with --save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_to: Option<String>,
}

/// Output of `fam mail attachments`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailAttachmentsOutput {
    /// The message's local ID.
    pub message: i64,
    /// The attachments, in the order Graph lists them.
    pub attachments: Vec<AttachmentInfo>,
}

/// One attachment with its bytes.
#[derive(Debug, Clone)]
pub struct AttachmentContent {
    /// What it is.
    pub info: AttachmentInfo,
    /// The `Content-ID` an inline image is referenced by (`cid:...`), if any.
    pub content_id: Option<String>,
    /// The file's bytes.
    pub bytes: Vec<u8>,
}

/// Where a message lives on the server: its account key and Graph ID.
pub(crate) fn locate(paths: &DaemonPaths, message: i64) -> Result<(String, String), CommandError> {
    let r = open_store(paths)?;
    let m = r
        .message(MessageId(message))
        .map_err(store_err)?
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::NotFound,
                format!("no message {message} on this device"),
            )
        })?;
    let key = r
        .accounts()
        .map_err(store_err)?
        .into_iter()
        .find(|a| a.id == m.account)
        .map(|a| a.key)
        .ok_or_else(|| CommandError::internal(format!("message {message} has no account")))?;
    Ok((key, m.remote_id))
}

pub(crate) fn graph_get(
    paths: &DaemonPaths,
    account: &str,
    path: String,
) -> Result<Value, CommandError> {
    Client::connect(&paths.socket)
        .map_err(|e| {
            if e.kind == ErrorKind::DaemonUnavailable {
                CommandError::new(
                    ErrorKind::DaemonUnavailable,
                    "attachments are read from the server through famd, which isn't running",
                )
            } else {
                e
            }
        })?
        .call(
            METHOD_GRAPH_GET,
            &GraphGetParams {
                account: Some(account.to_string()),
                path,
            },
        )
}

fn info(v: &Value) -> AttachmentInfo {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let kind = match v.get("@odata.type").and_then(Value::as_str) {
        Some("#microsoft.graph.itemAttachment") => AttachmentKind::Item,
        Some("#microsoft.graph.referenceAttachment") => AttachmentKind::Reference,
        _ => AttachmentKind::File,
    };
    AttachmentInfo {
        id: s("id"),
        name: s("name"),
        content_type: s("contentType"),
        size: v.get("size").and_then(Value::as_u64).unwrap_or(0),
        is_inline: v.get("isInline").and_then(Value::as_bool).unwrap_or(false),
        kind,
        saved_to: None,
    }
}

/// Lists a message's attachments (without their bytes), inline ones included.
pub fn list_at(paths: &DaemonPaths, message: i64) -> Result<Vec<AttachmentInfo>, CommandError> {
    let (account, remote) = locate(paths, message)?;
    let mut path =
        format!("/me/messages/{remote}/attachments?$select=id,name,contentType,size,isInline");
    let mut out = Vec::new();
    loop {
        let page = graph_get(paths, &account, path)?;
        if let Some(items) = page.get("value").and_then(Value::as_array) {
            out.extend(items.iter().map(info));
        }
        match page.get("@odata.nextLink").and_then(Value::as_str) {
            Some(next) => path = next.to_string(),
            None => return Ok(out),
        }
    }
}

/// Downloads one attachment. Only file attachments have bytes.
pub fn content_at(
    paths: &DaemonPaths,
    message: i64,
    attachment: &str,
) -> Result<AttachmentContent, CommandError> {
    let (account, remote) = locate(paths, message)?;
    let v = graph_get(
        paths,
        &account,
        format!("/me/messages/{remote}/attachments/{attachment}"),
    )?;
    let info = info(&v);
    if info.kind != AttachmentKind::File {
        return Err(CommandError::usage(format!(
            "{} is an attached {} and has no file to save",
            info.name,
            if info.kind == AttachmentKind::Item {
                "Outlook item"
            } else {
                "cloud link"
            }
        )));
    }
    let b64 = v.get("contentBytes").and_then(Value::as_str).unwrap_or("");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| {
            CommandError::internal(format!("attachment {}: bad base64: {e}", info.name))
        })?;
    let content_id = v
        .get("contentId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(AttachmentContent {
        info,
        content_id,
        bytes,
    })
}

/// A file name that is safe to create on every OS: no folders, no reserved
/// characters, never empty.
pub fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "attachment".into()
    } else {
        cleaned.to_string()
    }
}

/// Writes `bytes` as `name` in `dir` without replacing an existing file:
/// `report.pdf`, then `report (2).pdf`, and so on. Returns the path written.
pub fn save_unique(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, CommandError> {
    use std::io::Write as _;
    let name = safe_file_name(name);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    for n in 1..1000 {
        let file = if n == 1 {
            name.clone()
        } else {
            format!("{stem} ({n}){ext}")
        };
        let path = dir.join(file);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(bytes).map_err(|e| {
                    CommandError::internal(format!("write {}: {e}", path.display()))
                })?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(CommandError::usage(format!(
                    "can't save to {}: {e}",
                    path.display()
                )));
            }
        }
    }
    Err(CommandError::usage(format!(
        "{} already has too many files called {name}",
        dir.display()
    )))
}

/// Runs `fam mail attachments` against the daemon and store at `paths`.
pub fn attachments_at(
    paths: &DaemonPaths,
    input: &MailAttachmentsInput,
) -> Result<MailAttachmentsOutput, CommandError> {
    let mut list = list_at(paths, input.message)?;
    let wanted = |a: &AttachmentInfo| match &input.name {
        Some(n) => a.name == *n || a.id == *n,
        None => input.inline || !a.is_inline,
    };
    list.retain(|a| wanted(a));
    if input.name.is_some() && list.is_empty() {
        return Err(CommandError::new(
            ErrorKind::NotFound,
            format!(
                "message {} has no attachment called {}",
                input.message,
                input.name.as_deref().unwrap_or("")
            ),
        ));
    }
    if let Some(dir) = &input.save {
        let dir = Path::new(dir);
        if !dir.is_dir() {
            return Err(CommandError::usage(format!(
                "{} isn't a folder",
                dir.display()
            )));
        }
        for a in &mut list {
            if a.kind != AttachmentKind::File {
                continue;
            }
            let c = content_at(paths, input.message, &a.id)?;
            let path = save_unique(dir, &a.name, &c.bytes)?;
            a.saved_to = Some(path.display().to_string());
        }
    }
    Ok(MailAttachmentsOutput {
        message: input.message,
        attachments: list,
    })
}

impl Command for MailAttachments {
    type Input = MailAttachmentsInput;
    type Output = MailAttachmentsOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "attachments"],
        summary: "List or save a message's attachments",
        description: "Lists a message's attachments with their names, types and sizes, read from the server through famd (attachments aren't kept on this device). --save DIR downloads them into a folder, never replacing a file that is already there; --name picks one. Inline images are left out unless --inline. Attached Outlook items and cloud links are listed but can't be saved.",
        access: Access::Read,
        errors: &[
            ErrorKind::NotFound,
            ErrorKind::DaemonUnavailable,
            ErrorKind::Offline,
            ErrorKind::AuthRequired,
            ErrorKind::Throttled,
            ErrorKind::Server,
        ],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: MailAttachmentsInput,
    ) -> Result<MailAttachmentsOutput, CommandError> {
        attachments_at(&DaemonPaths::resolve()?, &input)
    }

    fn render_text(&self, o: &MailAttachmentsOutput) -> String {
        if o.attachments.is_empty() {
            return format!("Message {} has no attachments.", o.message);
        }
        o.attachments
            .iter()
            .map(|a| {
                let mut s = format!("{}  ({}, {})", a.name, a.content_type, human_size(a.size));
                match a.kind {
                    AttachmentKind::Item => s.push_str("  [Outlook item]"),
                    AttachmentKind::Reference => s.push_str("  [cloud link]"),
                    AttachmentKind::File => {}
                }
                if let Some(p) = &a.saved_to {
                    s.push_str(&format!("\n  saved to {p}"));
                }
                s
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// `2.4 MB`, `310 KB`, `12 bytes`.
pub fn human_size(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{} KB", n.div_ceil(1024))
    } else {
        format!("{n} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(safe_file_name("report.pdf"), "report.pdf");
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("C:\\Users\\x\\a.txt"), "a.txt");
        assert_eq!(safe_file_name("a:b?.txt"), "a_b_.txt");
        assert_eq!(safe_file_name(".."), "attachment");
        assert_eq!(safe_file_name("  "), "attachment");
        assert_eq!(safe_file_name("name. "), "name");
    }

    #[test]
    fn saving_never_replaces_a_file() {
        let dir = std::env::temp_dir().join(format!("fam-att-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = save_unique(&dir, "r.pdf", b"one").unwrap();
        let b = save_unique(&dir, "r.pdf", b"two").unwrap();
        let c = save_unique(&dir, "noext", b"three").unwrap();
        let d = save_unique(&dir, "noext", b"four").unwrap();
        assert_eq!(a.file_name().unwrap(), "r.pdf");
        assert_eq!(b.file_name().unwrap(), "r (2).pdf");
        assert_eq!(d.file_name().unwrap(), "noext (2)");
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        assert_eq!(std::fs::read(&c).unwrap(), b"three");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(human_size(12), "12 bytes");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(5 * 1024 * 1024 / 2), "2.5 MB");
    }
}
