// SPDX-License-Identifier: MIT OR Apache-2.0
//! Drafts and sending (WP 2.3): the parts of a write request and op payload
//! that compose mail, and helpers the commands use to build them.
//!
//! A draft exists locally at once, in the account's Drafts folder, with a
//! placeholder server ID; the `create_draft` op makes it on the server
//! (through `createReply`/`createReplyAll`/`createForward` for responses, so
//! threading headers are right) and then sets the body and recipients
//! Familiar composed, and adds attachments one at a time. Its progress is
//! saved in the op after each step, so a retry carries on where it stopped.
//!
//! Sending queues a `send_draft` op that may not run before the undo-send
//! delay (`[app] send_delay_seconds`); cancelling it in that time
//! (`fam ops cancel`) keeps the draft. While offline it waits in the queue,
//! which is the outbox.

use familiar_store::{
    AccountId, Address, BodyRef, Folder, LOCAL_REMOTE_PREFIX, Message, MessageId, NewMessage,
    WriteTx,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{CommandError, ErrorKind};

/// Graph takes attachments up to this size in one request.
pub const MAX_ATTACHMENT_BYTES: u64 = 3 * 1024 * 1024;

/// A sender or recipient.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Recipient {
    /// Display name, possibly empty.
    #[serde(default)]
    pub name: String,
    /// Address.
    pub address: String,
}

impl Recipient {
    /// Parses `Name <addr@example.com>` or `addr@example.com`.
    pub fn parse(s: &str) -> Result<Self, CommandError> {
        let s = s.trim();
        let (name, address) = match (s.rfind('<'), s.ends_with('>')) {
            (Some(i), true) => (
                s[..i].trim().trim_matches('"').to_string(),
                s[i + 1..s.len() - 1].trim().to_string(),
            ),
            _ => (String::new(), s.to_string()),
        };
        let ok = address
            .split_once('@')
            .is_some_and(|(l, d)| !l.is_empty() && d.contains('.') && !d.ends_with('.'))
            && !address.contains(char::is_whitespace);
        if !ok {
            return Err(CommandError::usage(format!("{s:?} isn't an email address")));
        }
        Ok(Self { name, address })
    }

    /// Parses a list, splitting each entry on commas and semicolons too.
    pub fn parse_all(items: &[String]) -> Result<Vec<Self>, CommandError> {
        let mut out = Vec::new();
        for item in items {
            for part in split_addresses(item) {
                out.push(Self::parse(&part)?);
            }
        }
        Ok(out)
    }

    fn from_address(a: &Address) -> Self {
        Self {
            name: a.name.clone(),
            address: a.addr.clone(),
        }
    }

    fn to_address(&self) -> Address {
        Address {
            name: self.name.clone(),
            addr: self.address.clone(),
        }
    }

    /// The domain, lower case.
    pub fn domain(&self) -> String {
        self.address
            .rsplit_once('@')
            .map(|(_, d)| d.to_ascii_lowercase())
            .unwrap_or_default()
    }
}

/// Splits on `,` and `;` outside quotes and angle brackets.
fn split_addresses(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quote, mut angle) = (false, false);
    for c in s.chars() {
        match c {
            '"' => quote = !quote,
            '<' => angle = true,
            '>' => angle = false,
            ',' | ';' if !quote && !angle => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// A file to attach, read by `fam` and carried in the op.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentData {
    /// File name.
    pub name: String,
    /// MIME type.
    pub content_type: String,
    /// An inline image, referenced from the body as `cid:<content_id>`.
    pub inline: bool,
    /// Content ID for inline images.
    pub content_id: Option<String>,
    /// The bytes, base64.
    pub content_b64: String,
    /// Size in bytes.
    pub size: u64,
}

impl AttachmentData {
    /// Reads a file. `inline` makes it an inline image whose content ID is
    /// its file name.
    pub fn read(path: &str, inline: bool) -> Result<Self, CommandError> {
        let bytes = std::fs::read(path)
            .map_err(|e| CommandError::new(ErrorKind::NotFound, format!("{path}: {e}")))?;
        if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
            return Err(CommandError::usage(format!(
                "{path} is {} KB; attachments are limited to 3 MB for now",
                bytes.len() / 1024
            )));
        }
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".into());
        Ok(Self {
            content_type: mime_for(&name).into(),
            content_id: inline.then(|| name.clone()),
            inline,
            size: bytes.len() as u64,
            content_b64: base64(&bytes),
            name,
        })
    }
}

/// Turns the images an HTML body embeds (`src="data:image/png;base64,..."`,
/// as signatures in config.toml do) or takes from a local file
/// (`src="C:\\sig\\logo.png"`, `file:///...`) into inline attachments
/// referenced by `cid:`. Web images (`http`, `https`) and `cid:` are left alone.
pub fn inline_images(html: &str) -> Result<(String, Vec<AttachmentData>), CommandError> {
    images_in(html, true)
}

/// Like [`inline_images`], but only for images embedded as `data:` URIs: no
/// files are read. For a whole body being replaced, which can hold a quoted
/// message whose HTML someone else wrote.
pub fn embedded_images(html: &str) -> Result<(String, Vec<AttachmentData>), CommandError> {
    images_in(html, false)
}

fn images_in(html: &str, files: bool) -> Result<(String, Vec<AttachmentData>), CommandError> {
    let mut out = String::with_capacity(html.len());
    let mut found = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut at = 0;
    while let Some(rel) = lower[at..].find("src=") {
        let start = at + rel + 4;
        let quote = html[start..].chars().next();
        let (vstart, vend) = match quote {
            Some(q @ ('"' | '\'')) => {
                let Some(e) = html[start + 1..].find(q) else {
                    break;
                };
                (start + 1, start + 1 + e)
            }
            _ => {
                let e = html[start..]
                    .find(|c: char| c.is_whitespace() || c == '>')
                    .unwrap_or(html.len() - start);
                (start, start + e)
            }
        };
        let src = &html[vstart..vend];
        let made = if let Some(data) = src.strip_prefix("data:") {
            data_uri(data)
        } else if let Some(path) = local_path(src).filter(|_| files) {
            let mut a = AttachmentData::read(&path, true)?;
            a.content_id = Some(format!(
                "{}.{}@familiar",
                content_hash(&a.content_b64),
                a.name
            ));
            Some(a)
        } else {
            None
        };
        out.push_str(&html[at..vstart]);
        match made {
            Some(a) => {
                let cid = a.content_id.clone().unwrap_or_default();
                out.push_str("cid:");
                out.push_str(&cid);
                // The same image twice is one attachment.
                if !found
                    .iter()
                    .any(|f: &AttachmentData| f.content_id.as_deref() == Some(&cid))
                {
                    found.push(a);
                }
            }
            None => out.push_str(src),
        }
        at = vend;
    }
    out.push_str(&html[at..]);
    Ok((out, found))
}

/// A short name for an image's bytes (FNV-1a over the base64), so the same
/// image gets the same content ID every time a body is saved. `draft update`
/// relies on that to leave images the draft already has alone.
fn content_hash(b64: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in b64.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:012x}", h >> 16)
}

fn data_uri(data: &str) -> Option<AttachmentData> {
    let (meta, payload) = data.split_once(',')?;
    let mime = meta.strip_suffix(";base64")?;
    if !mime.starts_with("image/") {
        return None;
    }
    let b64: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    let ext = match mime {
        "image/jpeg" => "jpg",
        "image/svg+xml" => "svg",
        m => m.trim_start_matches("image/"),
    };
    let name = format!("image-{}.{ext}", content_hash(&b64));
    let size = (b64.trim_end_matches('=').len() * 3 / 4) as u64;
    Some(AttachmentData {
        content_type: mime.to_string(),
        content_id: Some(format!("{name}@familiar")),
        inline: true,
        size,
        content_b64: b64,
        name,
    })
}

fn local_path(src: &str) -> Option<String> {
    let p = src
        .strip_prefix("file:///")
        .map(|p| {
            if cfg!(windows) {
                p.to_string()
            } else {
                format!("/{p}")
            }
        })
        .unwrap_or_else(|| src.to_string());
    let p = p.replace("%20", " ");
    let path = std::path::Path::new(&p);
    (path.is_absolute() && path.is_file()).then_some(p)
}

fn mime_for(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "ics" => "text/calendar",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Plain text as HTML: escaped, with line breaks.
pub fn text_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("<br>\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Text for a preview: tags dropped, whitespace squeezed, 255 characters.
pub fn preview_of(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut space = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                space = true;
            }
            _ if in_tag => {}
            c if c.is_whitespace() => space = true,
            c => {
                if space && !out.is_empty() {
                    out.push(' ');
                }
                space = false;
                out.push(c);
            }
        }
        if out.chars().count() >= 255 {
            break;
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Which response a draft is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResponseKind {
    /// Reply to the sender.
    Reply,
    /// Reply to everyone.
    ReplyAll,
    /// Forward.
    Forward,
}

/// `create_draft` op payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DraftOp {
    /// Local ID of the draft (set when it is made locally).
    pub message: i64,
    /// From, when it's an alias rather than the account's own address.
    #[serde(default)]
    pub from: Option<Recipient>,
    /// The message it responds to, with how.
    pub respond_to: Option<(i64, ResponseKind)>,
    /// Subject.
    pub subject: String,
    /// The whole HTML body, quoted original and signature included.
    pub html: String,
    /// To.
    pub to: Vec<Recipient>,
    /// Cc.
    pub cc: Vec<Recipient>,
    /// Bcc.
    pub bcc: Vec<Recipient>,
    /// `low`, `normal` or `high`.
    pub importance: Option<String>,
    /// Files to attach.
    pub attachments: Vec<AttachmentData>,
    /// Server ID once created (progress, for retries).
    #[serde(default)]
    pub remote_id: Option<String>,
    /// Attachments already added (progress).
    #[serde(default)]
    pub attachments_done: usize,
}

/// What a draft looked like before an update, to undo it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DraftSnapshot {
    /// From.
    #[serde(default)]
    pub from: Option<Recipient>,
    /// Subject.
    pub subject: String,
    /// To.
    pub to: Vec<Recipient>,
    /// Cc.
    pub cc: Vec<Recipient>,
    /// Body HTML, if it was on this device.
    pub html: Option<String>,
}

/// `update_draft` op payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DraftUpdateOp {
    /// Local ID of the draft.
    pub message: i64,
    /// New From (an alias, or the account's own address).
    #[serde(default)]
    pub from: Option<Recipient>,
    /// New subject.
    pub subject: Option<String>,
    /// New whole body.
    pub html: Option<String>,
    /// New To (replaces).
    pub to: Option<Vec<Recipient>>,
    /// New Cc (replaces).
    pub cc: Option<Vec<Recipient>>,
    /// New Bcc (replaces).
    pub bcc: Option<Vec<Recipient>>,
    /// Files to add.
    pub attachments: Vec<AttachmentData>,
    /// Attachments already added (progress).
    #[serde(default)]
    pub attachments_done: usize,
    /// Attachments to take off, by name or Graph attachment ID.
    #[serde(default)]
    pub remove_attachments: Vec<String>,
    /// Whether the draft's existing attachments have been checked: removals
    /// done, inline images the new body no longer shows taken off, and images
    /// it already has dropped from `attachments` (progress).
    #[serde(default)]
    pub pruned: bool,
    /// Before the update.
    pub prev: DraftSnapshot,
}

/// A new draft, as `fam` asks for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComposeRequest {
    /// The account (config name or address). Taken from the original for
    /// responses; needed for a new message with more than one account.
    pub account: Option<String>,
    /// Send from this alias; `None` sends from the account's own address.
    #[serde(default)]
    pub from: Option<Recipient>,
    /// Local ID of the message this responds to.
    pub respond_to: Option<i64>,
    /// How it responds; `None` for a new message.
    pub response: Option<ResponseKind>,
    /// Subject; for responses, defaults to `RE:`/`FW:` and the original's.
    pub subject: Option<String>,
    /// The new part of the body, signature included. The original is quoted
    /// below it for responses.
    pub html: String,
    /// To. For replies, defaults to the original's sender (and everyone, for
    /// reply all); given, it replaces them.
    pub to: Vec<Recipient>,
    /// Cc.
    pub cc: Vec<Recipient>,
    /// Bcc.
    pub bcc: Vec<Recipient>,
    /// `low`, `normal` or `high`.
    pub importance: Option<String>,
    /// Files to attach.
    pub attachments: Vec<AttachmentData>,
}

fn store_err(e: familiar_store::Error) -> CommandError {
    match e {
        familiar_store::Error::NotFound(m) => CommandError::new(ErrorKind::NotFound, m),
        e => CommandError::internal(format!("store: {e}")),
    }
}

fn html_escape(s: &str) -> String {
    text_to_html(s).replace("<br>\n", " ")
}

/// The body of a response: the new part, then the original's header and
/// body as Outlook quotes them.
fn quote(tx: &WriteTx<'_>, orig: &Message, new_part: &str) -> Result<String, CommandError> {
    let body = match orig.body {
        Some(b) => {
            let bytes = tx.body(&b.hash).map_err(store_err)?;
            let s = String::from_utf8_lossy(&bytes).into_owned();
            if b.is_html { s } else { text_to_html(&s) }
        }
        None => text_to_html(&orig.preview),
    };
    let to: Vec<String> = orig.to.iter().map(|a| html_escape(&a.addr)).collect();
    Ok(format!(
        "<html><body><div>{new_part}</div><hr><div id=\"divRplyFwdMsg\"><b>From:</b> {} &lt;{}&gt;<br><b>Sent:</b> {}<br><b>To:</b> {}<br><b>Subject:</b> {}</div><br>{body}</body></html>",
        html_escape(&orig.from.name),
        html_escape(&orig.from.addr),
        sent_text(
            orig.sent_at.unwrap_or(orig.received_at),
            crate::tz::local_zone()
        ),
        to.join("; "),
        html_escape(&orig.subject),
    ))
}

/// A quoted message's time as Outlook writes it in this computer's zone:
/// "Thursday, 5 November 2026 8:56 pm".
fn sent_text(secs: i64, zone: chrono_tz::Tz) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .unwrap_or_default()
        .with_timezone(&zone)
        .format("%A, %-d %B %Y %-I:%M %P")
        .to_string()
}

fn prefixed(prefix: &str, subject: &str) -> String {
    let p = prefix.trim_end_matches(' ').trim_end_matches(':');
    let lower = subject.to_ascii_lowercase();
    if lower.starts_with(&format!("{}:", p.to_ascii_lowercase())) {
        subject.to_string()
    } else {
        format!("{prefix}{subject}")
    }
}

/// Turns a compose request into a draft op payload for `account`.
pub fn plan_draft(
    tx: &WriteTx<'_>,
    req: &ComposeRequest,
    account: &familiar_store::Account,
) -> Result<DraftOp, CommandError> {
    let mut op = DraftOp {
        message: 0,
        from: req.from.clone(),
        respond_to: None,
        subject: req.subject.clone().unwrap_or_default(),
        html: format!("<html><body>{}</body></html>", req.html),
        to: req.to.clone(),
        cc: req.cc.clone(),
        bcc: req.bcc.clone(),
        importance: req.importance.clone(),
        attachments: req.attachments.clone(),
        remote_id: None,
        attachments_done: 0,
    };
    if let Some(id) = req.respond_to {
        let kind = req.response.unwrap_or(ResponseKind::Reply);
        let orig = tx
            .message(MessageId(id))
            .map_err(store_err)?
            .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("no message {id}")))?;
        if orig.remote_id.starts_with(LOCAL_REMOTE_PREFIX) {
            return Err(CommandError::usage(format!(
                "message {id} hasn't reached the server yet; respond once it has"
            )));
        }
        op.respond_to = Some((id, kind));
        op.html = quote(tx, &orig, &req.html)?;
        if req.subject.is_none() {
            op.subject = match kind {
                ResponseKind::Forward => prefixed("FW: ", &orig.subject),
                _ => prefixed("RE: ", &orig.subject),
            };
        }
        if req.to.is_empty() && kind != ResponseKind::Forward {
            // Not to ourselves: the account's address, nor the alias it sends from.
            let me = account.email.to_ascii_lowercase();
            let alias = req.from.as_ref().map(|f| f.address.to_ascii_lowercase());
            let not_me = |r: &Recipient| {
                let a = r.address.to_ascii_lowercase();
                a != me && Some(&a) != alias.as_ref()
            };
            let from = Recipient::from_address(&orig.from);
            op.to = vec![from.clone()];
            if kind == ResponseKind::ReplyAll {
                let mut to: Vec<Recipient> = std::iter::once(from)
                    .chain(orig.to.iter().map(Recipient::from_address))
                    .filter(not_me)
                    .collect();
                dedup(&mut to);
                let mut cc: Vec<Recipient> = orig
                    .cc
                    .iter()
                    .map(Recipient::from_address)
                    .filter(not_me)
                    .filter(|r| {
                        !to.iter()
                            .any(|t| t.address.eq_ignore_ascii_case(&r.address))
                    })
                    .collect();
                dedup(&mut cc);
                if req.cc.is_empty() {
                    op.cc = cc;
                }
                op.to = to;
            }
        }
    }
    Ok(op)
}

fn dedup(v: &mut Vec<Recipient>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|r| seen.insert(r.address.to_ascii_lowercase()));
}

/// Placeholder server IDs for drafts made locally.
pub fn placeholder() -> String {
    crate::ops::placeholder_remote_id()
}

fn drafts_folder(tx: &WriteTx<'_>, account: AccountId) -> Result<Folder, CommandError> {
    tx.folders(account)
        .map_err(store_err)?
        .into_iter()
        .find(|f| f.well_known.as_deref() == Some("drafts"))
        .ok_or_else(|| CommandError::new(ErrorKind::NotFound, "the account has no Drafts folder"))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Makes the draft locally, in Drafts. Returns its local ID.
pub fn apply_draft(
    tx: &WriteTx<'_>,
    op: &mut DraftOp,
    account: AccountId,
) -> familiar_store::Result<i64> {
    let drafts =
        drafts_folder(tx, account).map_err(|e| familiar_store::Error::NotFound(e.message))?;
    let acc = tx
        .accounts()?
        .into_iter()
        .find(|a| a.id == account)
        .ok_or_else(|| familiar_store::Error::NotFound(format!("account {}", account.0)))?;
    let conversation = match op.respond_to {
        Some((id, _)) => tx
            .message(MessageId(id))?
            .and_then(|m| tx.conversation_of(m.id).ok().flatten()),
        None => None,
    };
    let hash = tx.put_body(op.html.as_bytes())?;
    let remote = placeholder();
    let t = now();
    let id = tx.upsert_message(&NewMessage {
        account,
        folder: drafts.id,
        remote_id: remote.clone(),
        internet_message_id: None,
        conversation_id: conversation.unwrap_or_else(|| remote.clone()),
        subject: op.subject.clone(),
        from: op.from.as_ref().map_or_else(
            || Address {
                name: acc.display_name.clone(),
                addr: acc.email.clone(),
            },
            Recipient::to_address,
        ),
        to: op.to.iter().map(Recipient::to_address).collect(),
        cc: op.cc.iter().map(Recipient::to_address).collect(),
        received_at: t,
        sent_at: None,
        is_read: true,
        is_flagged: false,
        importance: match op.importance.as_deref() {
            Some("low") => 0,
            Some("high") => 2,
            _ => 1,
        },
        has_attachments: op.attachments.iter().any(|a| !a.inline),
        preview: preview_of(&op.html),
        body: Some(BodyRef {
            hash,
            is_html: true,
        }),
    })?;
    op.message = id.0;
    Ok(id.0)
}

/// Builds the store row for a message with changed fields.
fn rewrite(
    tx: &WriteTx<'_>,
    m: &Message,
    from: Option<&Recipient>,
    subject: Option<&str>,
    to: Option<&[Recipient]>,
    cc: Option<&[Recipient]>,
    html: Option<&str>,
) -> familiar_store::Result<()> {
    let body = match html {
        Some(h) => Some(BodyRef {
            hash: tx.put_body(h.as_bytes())?,
            is_html: true,
        }),
        None => None,
    };
    let conversation = tx.conversation_of(m.id)?.unwrap_or_default();
    tx.upsert_message(&NewMessage {
        account: m.account,
        folder: m.folder,
        remote_id: m.remote_id.clone(),
        internet_message_id: m.internet_message_id.clone(),
        conversation_id: conversation,
        subject: subject.map_or_else(|| m.subject.clone(), str::to_string),
        from: from.map_or_else(|| m.from.clone(), Recipient::to_address),
        to: to.map_or_else(
            || m.to.clone(),
            |v| v.iter().map(Recipient::to_address).collect(),
        ),
        cc: cc.map_or_else(
            || m.cc.clone(),
            |v| v.iter().map(Recipient::to_address).collect(),
        ),
        received_at: m.received_at,
        sent_at: m.sent_at,
        is_read: m.is_read,
        is_flagged: m.is_flagged,
        importance: m.importance,
        has_attachments: m.has_attachments,
        preview: html.map_or_else(|| m.preview.clone(), preview_of),
        body,
    })?;
    Ok(())
}

/// What a draft is now, for undoing an update.
pub fn snapshot(tx: &WriteTx<'_>, m: &Message) -> familiar_store::Result<DraftSnapshot> {
    let html = match m.body {
        Some(b) => Some(String::from_utf8_lossy(&tx.body(&b.hash)?).into_owned()),
        None => None,
    };
    Ok(DraftSnapshot {
        from: Some(Recipient::from_address(&m.from)),
        subject: m.subject.clone(),
        to: m.to.iter().map(Recipient::from_address).collect(),
        cc: m.cc.iter().map(Recipient::from_address).collect(),
        html,
    })
}

/// Applies a draft update locally.
pub fn apply_update(tx: &WriteTx<'_>, op: &DraftUpdateOp) -> familiar_store::Result<()> {
    let Some(m) = tx.message(MessageId(op.message))? else {
        return Err(familiar_store::Error::NotFound(format!(
            "message {}",
            op.message
        )));
    };
    rewrite(
        tx,
        &m,
        op.from.as_ref(),
        op.subject.as_deref(),
        op.to.as_deref(),
        op.cc.as_deref(),
        op.html.as_deref(),
    )
}

/// Undoes a draft update locally.
pub fn undo_update(tx: &WriteTx<'_>, op: &DraftUpdateOp) -> familiar_store::Result<()> {
    let Some(m) = tx.message(MessageId(op.message))? else {
        return Ok(());
    };
    rewrite(
        tx,
        &m,
        op.prev.from.as_ref(),
        Some(&op.prev.subject),
        Some(&op.prev.to),
        Some(&op.prev.cc),
        op.prev.html.as_deref(),
    )
}

/// Undoes a local draft that never reached the server.
pub fn undo_draft(tx: &WriteTx<'_>, op: &DraftOp) -> familiar_store::Result<()> {
    if let Some(m) = tx.message(MessageId(op.message))? {
        if m.remote_id.starts_with(LOCAL_REMOTE_PREFIX) {
            tx.delete_message(m.id)?;
        }
    }
    Ok(())
}

/// The draft's recipients that others on it can see, outside the account's
/// own domain. Bcc doesn't count.
pub fn visible_outside(account_email: &str, to: &[Recipient], cc: &[Recipient]) -> usize {
    let own = Recipient {
        name: String::new(),
        address: account_email.to_string(),
    }
    .domain();
    to.iter().chain(cc).filter(|r| r.domain() != own).count()
}

/// More visible outside recipients than this needs `--yes`.
pub const OUTSIDE_RECIPIENT_LIMIT: usize = 10;

/// Refuses a send to many outside recipients who can see each other.
pub fn check_outside(
    account_email: &str,
    to: &[Recipient],
    cc: &[Recipient],
    yes: bool,
) -> Result<(), CommandError> {
    let n = visible_outside(account_email, to, cc);
    if n > OUTSIDE_RECIPIENT_LIMIT && !yes {
        return Err(CommandError::new(
            ErrorKind::ConfirmationRequired,
            format!(
                "{n} recipients outside your domain would see each other's addresses; put them in Bcc, or add --yes to send anyway"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    #[test]
    fn quoted_time_reads_like_outlook() {
        // 2026-11-05T07:56:25Z is 8:56 pm in Auckland.
        assert_eq!(
            super::sent_text(1_793_865_385, chrono_tz::Pacific::Auckland),
            "Thursday, 5 November 2026 8:56 pm"
        );
    }

    #[test]
    fn signature_images_become_inline_attachments() {
        let html = "<p>Cheers</p><img alt=logo src=\"data:image/png;base64,iVBO\nRw0K\"> <img src='https://example.com/a.png'><img src=\"cid:x\">";
        let (out, files) = super::inline_images(html).unwrap();
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert!(f.inline);
        assert_eq!(f.content_type, "image/png");
        assert_eq!(f.content_b64, "iVBORw0K");
        let cid = f.content_id.clone().unwrap();
        assert!(
            cid.starts_with("image-") && cid.ends_with(".png@familiar"),
            "{cid}"
        );
        assert!(out.contains(&format!("src=\"cid:{cid}\"")), "{out}");
        // The same bytes give the same content ID, and repeats are one file.
        let (again, files2) = super::inline_images(&format!("{html}{html}")).unwrap();
        assert_eq!(files2.len(), 1);
        assert_eq!(files2[0].content_id.as_deref(), Some(cid.as_str()));
        assert_eq!(again.matches(&cid).count(), 2);
        assert!(out.contains("src='https://example.com/a.png'"));
        assert!(out.contains("src=\"cid:x\""));
        // A local file is read; one that doesn't exist is left as it was.
        let dir = std::env::temp_dir().join(format!("fam-inline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let logo = dir.join("logo.gif");
        std::fs::write(&logo, b"GIF89a").unwrap();
        let (out, files) = super::inline_images(&format!(
            "<img src=\"{}\"><img src=\"/no/such.png\">",
            logo.display()
        ))
        .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content_type, "image/gif");
        // A replaced body reads no files.
        let (kept, none) =
            super::embedded_images(&format!("<img src=\"{}\">", logo.display())).unwrap();
        assert!(none.is_empty() && kept.contains("logo.gif"), "{kept}");
        assert!(
            out.contains(".logo.gif@familiar") && out.contains("/no/such.png"),
            "{out}"
        );
    }

    use super::*;

    #[test]
    fn recipients_parse() {
        let r = Recipient::parse("Pat Doe <pat@example.com>").unwrap();
        assert_eq!(r.name, "Pat Doe");
        assert_eq!(r.address, "pat@example.com");
        assert_eq!(Recipient::parse("a@b.co").unwrap().name, "");
        assert!(Recipient::parse("not an address").is_err());
        assert!(Recipient::parse("a@b").is_err());
        let all = Recipient::parse_all(&["\"Doe, Pat\" <pat@example.com>; b@c.nz".into()]).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "Doe, Pat");
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn text_becomes_html() {
        assert_eq!(text_to_html("a < b\nc"), "a &lt; b<br>\nc");
        assert_eq!(preview_of("<p>Hi&nbsp;there</p><p>Pat</p>"), "Hi there Pat");
    }

    #[test]
    fn outside_recipients_count_visible_ones_only() {
        let r = |a: &str| Recipient::parse(a).unwrap();
        let to: Vec<Recipient> = (0..11).map(|i| r(&format!("p{i}@other.org"))).collect();
        assert_eq!(visible_outside("me@club.nz", &to, &[r("x@club.nz")]), 11);
        assert!(check_outside("me@club.nz", &to, &[], false).is_err());
        assert!(check_outside("me@club.nz", &to, &[], true).is_ok());
        assert!(check_outside("me@club.nz", &to[..10], &[], false).is_ok());
    }
}
