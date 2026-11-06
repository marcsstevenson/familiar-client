// SPDX-License-Identifier: MIT OR Apache-2.0
//! Drafts and sending (WP 2.3): `fam draft create/reply/forward/update/send`
//! and `fam mail send`.
//!
//! Bodies are plain text unless `--html`; the account's signature from
//! `config.toml` (`[accounts.<name>.signature]`, `new` or `reply`) is added
//! unless `--no-signature`. A send waits `[app] send_delay_seconds` in the
//! queue first (cancel it with `fam ops cancel <id>`), unless `--now`.

use crate::commands::mail::open_store;
use crate::commands::write::{WriteFlags, flags, submit_at};
use crate::compose::{
    AttachmentData, ComposeRequest, Recipient, ResponseKind, check_outside, text_to_html,
};
use crate::daemon::DaemonPaths;
use crate::error::{CommandError, ErrorKind};
use crate::ops::{WriteOutput, WriteRequest};
use crate::registry::{Access, Command, CommandSpec, Context};
use familiar_store::{Account, MessageId};
use schemars::JsonSchema;
use serde::Deserialize;

const ERRORS: &[ErrorKind] = &[
    ErrorKind::NotFound,
    ErrorKind::Config,
    ErrorKind::Conflict,
    ErrorKind::PermissionDenied,
    ErrorKind::ConfirmationRequired,
    ErrorKind::DaemonUnavailable,
    ErrorKind::Offline,
    ErrorKind::Server,
];

/// `config.toml`, if there is one. An invalid file is an error.
fn config() -> Result<Option<familiar_config::Config>, CommandError> {
    let Some((path, _)) = familiar_config::paths::resolve(None) else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    familiar_config::load(&path)
        .map(Some)
        .map_err(|e| CommandError::new(ErrorKind::Config, format!("{}: {e}", path.display())))
}

/// The settings a compose command needs from config, resolved once.
#[derive(Debug, Clone, Default)]
pub struct ComposeSettings {
    /// Signature HTML for new messages, per account key.
    pub new_signatures: Vec<(String, String)>,
    /// Signature HTML for replies and forwards, per account key.
    pub reply_signatures: Vec<(String, String)>,
    /// Undo-send delay.
    pub send_delay_secs: u32,
    /// Aliases, per account key.
    pub aliases: Vec<(String, Vec<String>)>,
}

impl ComposeSettings {
    /// From `config.toml` (defaults without one).
    pub fn load() -> Result<Self, CommandError> {
        let Some(c) = config()? else {
            return Ok(Self {
                send_delay_secs: 10,
                ..Self::default()
            });
        };
        let mut s = Self {
            send_delay_secs: c.app.send_delay_seconds,
            ..Self::default()
        };
        for (key, a) in &c.accounts {
            if !a.aliases.is_empty() {
                s.aliases.push((key.clone(), a.aliases.clone()));
            }
            if let Some(sig) = &a.signature {
                if let Some(n) = &sig.new {
                    s.new_signatures.push((key.clone(), n.clone()));
                }
                if let Some(r) = &sig.reply {
                    s.reply_signatures.push((key.clone(), r.clone()));
                }
            }
        }
        Ok(s)
    }

    fn aliases(&self, key: &str) -> &[String] {
        self.aliases
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map_or(&[], |(_, v)| v.as_slice())
    }

    /// The alias to send from: `from` when given (checked against the
    /// account's aliases), otherwise for a response the alias the original
    /// was sent to. `None` means the account's own address.
    fn alias_to_send_as(
        &self,
        account: &Account,
        from: Option<&str>,
        original: Option<&familiar_store::Message>,
    ) -> Result<Option<String>, CommandError> {
        let aliases = self.aliases(&account.key);
        let alias = |addr: &str| {
            aliases
                .iter()
                .find(|a| a.eq_ignore_ascii_case(addr))
                .cloned()
        };
        if let Some(f) = from.map(str::trim).filter(|f| !f.is_empty()) {
            let addr = Recipient::parse_all(&[f.to_string()])?
                .pop()
                .map_or_else(|| f.to_string(), |r| r.address);
            if addr.eq_ignore_ascii_case(&account.email) {
                return Ok(None);
            }
            return alias(&addr).map(Some).ok_or_else(|| {
                CommandError::usage(format!(
                    "{addr} isn't an address of account {}; list it in aliases in config.toml",
                    account.key
                ))
            });
        }
        Ok(original.and_then(|m| m.to.iter().chain(&m.cc).find_map(|a| alias(&a.addr))))
    }

    fn signature(&self, key: &str, reply: bool) -> Option<&str> {
        let list = if reply {
            &self.reply_signatures
        } else {
            &self.new_signatures
        };
        list.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, s)| s.as_str())
    }
}

/// The text a compose command was given, as body HTML.
fn body_html(
    body: Option<&str>,
    body_file: Option<&str>,
    html: bool,
) -> Result<String, CommandError> {
    let text = match (body, body_file) {
        (Some(_), Some(_)) => {
            return Err(CommandError::usage("give --body or --body-file, not both"));
        }
        (Some(b), None) => b.to_string(),
        (None, Some(f)) => std::fs::read_to_string(f)
            .map_err(|e| CommandError::new(ErrorKind::NotFound, format!("{f}: {e}")))?,
        (None, None) => String::new(),
    };
    Ok(if html { text } else { text_to_html(&text) })
}

fn with_signature(body: String, sig: Option<&str>) -> String {
    match sig {
        Some(s) if !s.trim().is_empty() => {
            // A blank line between the sign-off and the signature.
            format!("{body}<br>\n<div><br></div>\n<div id=\"Signature\">{s}</div>")
        }
        _ => body,
    }
}

fn attachments(attach: &[String], inline: &[String]) -> Result<Vec<AttachmentData>, CommandError> {
    let mut out = Vec::new();
    for p in attach {
        out.push(AttachmentData::read(p, false)?);
    }
    for p in inline {
        out.push(AttachmentData::read(p, true)?);
    }
    Ok(out)
}

fn importance(i: Option<&str>) -> Result<Option<String>, CommandError> {
    match i {
        None => Ok(None),
        Some(v @ ("low" | "normal" | "high")) => Ok(Some(v.to_string())),
        Some(v) => Err(CommandError::usage(format!(
            "--importance is low, normal or high, not {v:?}"
        ))),
    }
}

/// The account a new message goes from: the one named, or the only one.
fn account_for(paths: &DaemonPaths, only: Option<&str>) -> Result<Account, CommandError> {
    let r = open_store(paths)?;
    let all = r.accounts().map_err(crate::commands::mail::store_err)?;
    match only {
        Some(k) => all
            .into_iter()
            .find(|a| a.key.eq_ignore_ascii_case(k) || a.email.eq_ignore_ascii_case(k))
            .ok_or_else(|| {
                CommandError::new(
                    ErrorKind::NotFound,
                    format!("no account {k:?} on this device"),
                )
            }),
        None if all.len() == 1 => Ok(all.into_iter().next().expect("one")),
        None if all.is_empty() => Err(CommandError::new(
            ErrorKind::NotFound,
            "no account has synced on this device",
        )),
        None => {
            let mut keys: Vec<String> = all.into_iter().map(|a| a.key).collect();
            keys.sort();
            Err(CommandError::usage(format!(
                "more than one account; pick one with --account ({})",
                keys.join(", ")
            )))
        }
    }
}

/// The account a message belongs to.
fn account_of(
    paths: &DaemonPaths,
    message: i64,
) -> Result<(Account, familiar_store::Message), CommandError> {
    let r = open_store(paths)?;
    let m = r
        .message(MessageId(message))
        .map_err(crate::commands::mail::store_err)?
        .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("no message {message}")))?;
    let a = r
        .accounts()
        .map_err(crate::commands::mail::store_err)?
        .into_iter()
        .find(|a| a.id == m.account)
        .ok_or_else(|| CommandError::internal("message without an account"))?;
    Ok((a, m))
}

/// Everything a compose command can be given.
#[derive(Debug, Clone, Default)]
pub struct ComposeArgs {
    /// Account (new messages).
    pub account: Option<String>,
    /// Send from this alias of the account.
    pub from: Option<String>,
    /// Message responded to.
    pub respond_to: Option<i64>,
    /// How.
    pub response: Option<ResponseKind>,
    /// Recipients.
    pub to: Vec<String>,
    /// Cc.
    pub cc: Vec<String>,
    /// Bcc.
    pub bcc: Vec<String>,
    /// Subject.
    pub subject: Option<String>,
    /// Body text.
    pub body: Option<String>,
    /// Body from a file.
    pub body_file: Option<String>,
    /// Body is HTML.
    pub html: bool,
    /// Files.
    pub attach: Vec<String>,
    /// Inline images.
    pub inline: Vec<String>,
    /// Importance.
    pub importance: Option<String>,
    /// Leave the signature off.
    pub no_signature: bool,
}

/// Builds a compose request: recipients parsed, body and signature, files read.
pub fn compose_request(
    paths: &DaemonPaths,
    args: &ComposeArgs,
    settings: &ComposeSettings,
) -> Result<(ComposeRequest, Account), CommandError> {
    let (account, original) = match args.respond_to {
        Some(id) => {
            let (a, m) = account_of(paths, id)?;
            (a, Some(m))
        }
        None => (account_for(paths, args.account.as_deref())?, None),
    };
    let from = settings
        .alias_to_send_as(&account, args.from.as_deref(), original.as_ref())?
        .map(|address| Recipient {
            name: account.display_name.clone(),
            address,
        });
    let body = body_html(args.body.as_deref(), args.body_file.as_deref(), args.html)?;
    let sig = if args.no_signature {
        None
    } else {
        settings.signature(&account.key, args.respond_to.is_some())
    };
    // Signature (and body) images become inline attachments.
    let (html, images) = crate::compose::inline_images(&with_signature(body, sig))?;
    let mut files = attachments(&args.attach, &args.inline)?;
    files.extend(images);
    Ok((
        ComposeRequest {
            account: Some(account.key.clone()),
            from,
            respond_to: args.respond_to,
            response: args.response,
            subject: args.subject.clone(),
            html,
            to: Recipient::parse_all(&args.to)?,
            cc: Recipient::parse_all(&args.cc)?,
            bcc: Recipient::parse_all(&args.bcc)?,
            importance: importance(args.importance.as_deref())?,
            attachments: files,
        },
        account,
    ))
}

/// Saves a draft, and sends it when `send` (checking outside recipients).
pub fn compose_at(
    paths: &DaemonPaths,
    args: &ComposeArgs,
    settings: &ComposeSettings,
    send: Option<(bool, bool)>,
    f: &WriteFlags,
) -> Result<WriteOutput, CommandError> {
    let (req, account) = compose_request(paths, args, settings)?;
    let (send, delay) = match send {
        Some((now, yes)) => {
            check_outside(&account.email, &req.to, &req.cc, yes)?;
            (true, if now { 0 } else { settings.send_delay_secs })
        }
        None => (false, 0),
    };
    submit_at(
        paths,
        WriteRequest::Compose {
            draft: req,
            send,
            send_delay_secs: delay,
        },
        f,
    )
}

/// Sends a saved draft.
pub fn send_at(
    paths: &DaemonPaths,
    draft: i64,
    now: bool,
    yes: bool,
    settings: &ComposeSettings,
    f: &WriteFlags,
) -> Result<WriteOutput, CommandError> {
    let (account, m) = account_of(paths, draft)?;
    let rec = |v: &[familiar_store::Address]| -> Vec<Recipient> {
        v.iter()
            .map(|a| Recipient {
                name: a.name.clone(),
                address: a.addr.clone(),
            })
            .collect()
    };
    check_outside(&account.email, &rec(&m.to), &rec(&m.cc), yes)?;
    submit_at(
        paths,
        WriteRequest::Send {
            message: draft,
            delay_secs: if now { 0 } else { settings.send_delay_secs },
        },
        f,
    )
}

fn render(o: &WriteOutput) -> String {
    let mut s = o.render();
    if let Some(d) = o
        .ops
        .iter()
        .find_map(|q| (q.kind == "create_draft").then_some(q.item).flatten())
    {
        s.push_str(&format!("\nDraft: message {d}"));
    }
    if let Some(q) = o
        .ops
        .iter()
        .find(|q| q.kind == "send_draft" && q.state == "pending")
    {
        if let (Some(at), Some(id)) = (&q.not_before, q.id) {
            s.push_str(&format!("\nSending at {at}; to undo: fam ops cancel {id}"));
        }
    }
    s
}

// ---------------------------------------------------------------------------

/// `fam draft create`.
pub struct DraftCreate;

/// Input for `fam draft create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftCreateInput {
    /// The account to write from (config name or address); needed with more than one.
    #[serde(default)]
    pub account: Option<String>,
    /// Send from this alias of the account (listed in `aliases` in config.toml).
    #[serde(default)]
    pub from: Option<String>,
    /// To: an address or `Name <address>`; repeat, or separate with commas.
    #[serde(default)]
    pub to: Vec<String>,
    /// Cc.
    #[serde(default)]
    pub cc: Vec<String>,
    /// Bcc.
    #[serde(default)]
    pub bcc: Vec<String>,
    /// Subject.
    #[serde(default)]
    pub subject: Option<String>,
    /// Body text (plain text unless --html).
    #[serde(default)]
    pub body: Option<String>,
    /// Read the body from a file.
    #[serde(default)]
    pub body_file: Option<String>,
    /// The body is HTML.
    #[serde(default)]
    pub html: bool,
    /// Attach a file (up to 3 MB); repeat for more.
    #[serde(default)]
    pub attach: Vec<String>,
    /// Add an inline image, referenced in an HTML body as cid:<file name>.
    #[serde(default)]
    pub inline: Vec<String>,
    /// low, normal or high.
    #[serde(default)]
    pub importance: Option<String>,
    /// Leave the account's signature off.
    #[serde(default)]
    pub no_signature: bool,
    /// Only show what would happen.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the draft (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for DraftCreate {
    type Input = DraftCreateInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["draft", "create"],
        summary: "Save a new draft",
        description: "Saves a new message in Drafts, with the account's new-message signature, and queues it for the server. Send it later with fam draft send.",
        access: Access::Draft,
        errors: ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: DraftCreateInput) -> Result<WriteOutput, CommandError> {
        let args = ComposeArgs {
            account: i.account,
            from: i.from,
            to: i.to,
            cc: i.cc,
            bcc: i.bcc,
            subject: i.subject,
            body: i.body,
            body_file: i.body_file,
            html: i.html,
            attach: i.attach,
            inline: i.inline,
            importance: i.importance,
            no_signature: i.no_signature,
            ..ComposeArgs::default()
        };
        compose_at(
            &DaemonPaths::resolve()?,
            &args,
            &ComposeSettings::load()?,
            None,
            &flags(i.dry_run, i.wait, i.wait_secs),
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        render(o)
    }
}

/// `fam draft reply`.
pub struct DraftReply;

/// Input for `fam draft reply`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftReplyInput {
    /// Local ID of the message to reply to.
    #[schemars(extend("x-positional" = 0))]
    pub message: i64,
    /// Reply to everyone on it, not just the sender.
    #[serde(default)]
    pub all: bool,
    /// Send from this alias of the account; defaults to the alias the original was sent to.
    #[serde(default)]
    pub from: Option<String>,
    /// Replace the recipients (default: the sender, or everyone with --all).
    #[serde(default)]
    pub to: Vec<String>,
    /// Cc (replaces the default with --all).
    #[serde(default)]
    pub cc: Vec<String>,
    /// Bcc.
    #[serde(default)]
    pub bcc: Vec<String>,
    /// Body text, above the quoted original (plain text unless --html).
    #[serde(default)]
    pub body: Option<String>,
    /// Read the body from a file.
    #[serde(default)]
    pub body_file: Option<String>,
    /// The body is HTML.
    #[serde(default)]
    pub html: bool,
    /// Attach a file; repeat for more.
    #[serde(default)]
    pub attach: Vec<String>,
    /// Add an inline image.
    #[serde(default)]
    pub inline: Vec<String>,
    /// Leave the account's reply signature off.
    #[serde(default)]
    pub no_signature: bool,
    /// Only show what would happen.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the draft (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for DraftReply {
    type Input = DraftReplyInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["draft", "reply"],
        summary: "Save a reply as a draft",
        description: "Saves a reply (or reply all with --all) in Drafts, threaded with the original, with your text and the reply signature above the quoted message.",
        access: Access::Draft,
        errors: ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: DraftReplyInput) -> Result<WriteOutput, CommandError> {
        let args = ComposeArgs {
            respond_to: Some(i.message),
            from: i.from,
            response: Some(if i.all {
                ResponseKind::ReplyAll
            } else {
                ResponseKind::Reply
            }),
            to: i.to,
            cc: i.cc,
            bcc: i.bcc,
            body: i.body,
            body_file: i.body_file,
            html: i.html,
            attach: i.attach,
            inline: i.inline,
            no_signature: i.no_signature,
            ..ComposeArgs::default()
        };
        compose_at(
            &DaemonPaths::resolve()?,
            &args,
            &ComposeSettings::load()?,
            None,
            &flags(i.dry_run, i.wait, i.wait_secs),
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        render(o)
    }
}

/// `fam draft forward`.
pub struct DraftForward;

/// Input for `fam draft forward`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftForwardInput {
    /// Local ID of the message to forward.
    #[schemars(extend("x-positional" = 0))]
    pub message: i64,
    /// Send from this alias of the account; defaults to the alias the original was sent to.
    #[serde(default)]
    pub from: Option<String>,
    /// To.
    #[serde(default)]
    pub to: Vec<String>,
    /// Cc.
    #[serde(default)]
    pub cc: Vec<String>,
    /// Bcc.
    #[serde(default)]
    pub bcc: Vec<String>,
    /// Body text, above the forwarded message.
    #[serde(default)]
    pub body: Option<String>,
    /// Read the body from a file.
    #[serde(default)]
    pub body_file: Option<String>,
    /// The body is HTML.
    #[serde(default)]
    pub html: bool,
    /// Attach another file; repeat for more.
    #[serde(default)]
    pub attach: Vec<String>,
    /// Leave the account's reply signature off.
    #[serde(default)]
    pub no_signature: bool,
    /// Only show what would happen.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the draft (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for DraftForward {
    type Input = DraftForwardInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["draft", "forward"],
        summary: "Save a forward as a draft",
        description: "Saves a forward of a message (with its attachments) in Drafts, with your text and the reply signature above it.",
        access: Access::Draft,
        errors: ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: DraftForwardInput) -> Result<WriteOutput, CommandError> {
        let args = ComposeArgs {
            respond_to: Some(i.message),
            from: i.from,
            response: Some(ResponseKind::Forward),
            to: i.to,
            cc: i.cc,
            bcc: i.bcc,
            body: i.body,
            body_file: i.body_file,
            html: i.html,
            attach: i.attach,
            no_signature: i.no_signature,
            ..ComposeArgs::default()
        };
        compose_at(
            &DaemonPaths::resolve()?,
            &args,
            &ComposeSettings::load()?,
            None,
            &flags(i.dry_run, i.wait, i.wait_secs),
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        render(o)
    }
}

/// `fam draft update`.
pub struct DraftUpdate;

/// Input for `fam draft update`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftUpdateInput {
    /// Local ID of the draft.
    #[schemars(extend("x-positional" = 0))]
    pub draft: i64,
    /// Send from this alias, or the account's own address.
    #[serde(default)]
    pub from: Option<String>,
    /// Replace To (`""` empties it).
    #[serde(default)]
    pub to: Vec<String>,
    /// Replace Cc (`""` empties it).
    #[serde(default)]
    pub cc: Vec<String>,
    /// Replace Bcc (`""` empties it).
    #[serde(default)]
    pub bcc: Vec<String>,
    /// New subject.
    #[serde(default)]
    pub subject: Option<String>,
    /// Replace the whole body (plain text unless --html; no signature is added).
    #[serde(default)]
    pub body: Option<String>,
    /// Replace the whole body from a file.
    #[serde(default)]
    pub body_file: Option<String>,
    /// The body is HTML.
    #[serde(default)]
    pub html: bool,
    /// Attach a file; repeat for more.
    #[serde(default)]
    pub attach: Vec<String>,
    /// Add an inline image.
    #[serde(default)]
    pub inline: Vec<String>,
    /// Take an attachment off, by file name (or Graph attachment ID); repeat for more.
    #[serde(default)]
    pub remove_attachment: Vec<String>,
    /// Only show what would happen.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the change (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for DraftUpdate {
    type Input = DraftUpdateInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["draft", "update"],
        summary: "Change a draft",
        description: "Changes a draft's recipients, subject or body, or adds and removes attachments. Recipient flags replace that line; --body replaces the whole body, and inline images it no longer shows are taken off.",
        access: Access::Draft,
        errors: ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: DraftUpdateInput) -> Result<WriteOutput, CommandError> {
        let paths = DaemonPaths::resolve()?;
        let from = match i.from.as_deref() {
            Some(f) => {
                let (account, _) = account_of(&paths, i.draft)?;
                let alias = ComposeSettings::load()?.alias_to_send_as(&account, Some(f), None)?;
                Some(Recipient {
                    name: account.display_name.clone(),
                    address: alias.unwrap_or(account.email),
                })
            }
            None => None,
        };
        let mut files = attachments(&i.attach, &i.inline)?;
        // Images the body embeds become inline attachments, as for a new
        // draft. Their content IDs follow their bytes, so images the draft
        // already has are not added again.
        let html = if i.body.is_some() || i.body_file.is_some() {
            // Only embedded images: the body may quote someone else's HTML,
            // which must not be able to name local files.
            let (html, images) = crate::compose::embedded_images(&body_html(
                i.body.as_deref(),
                i.body_file.as_deref(),
                i.html,
            )?)?;
            files.extend(images);
            Some(html)
        } else {
            None
        };
        // No flag leaves a line alone; `--cc ""` empties it.
        let list = |v: &[String]| -> Result<Option<Vec<Recipient>>, CommandError> {
            if v.is_empty() {
                Ok(None)
            } else if v.iter().all(|s| s.trim().is_empty()) {
                Ok(Some(Vec::new()))
            } else {
                Recipient::parse_all(v).map(Some)
            }
        };
        submit_at(
            &paths,
            WriteRequest::DraftUpdate {
                message: i.draft,
                from,
                subject: i.subject,
                html,
                to: list(&i.to)?,
                cc: list(&i.cc)?,
                bcc: list(&i.bcc)?,
                attachments: files,
                remove_attachments: i.remove_attachment,
            },
            &flags(i.dry_run, i.wait, i.wait_secs),
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        render(o)
    }
}

/// `fam draft send`.
pub struct DraftSend;

/// Input for `fam draft send`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftSendInput {
    /// Local ID of the draft.
    #[schemars(extend("x-positional" = 0))]
    pub draft: i64,
    /// Skip the undo-send delay.
    #[serde(default)]
    pub now: bool,
    /// Send even to many outside recipients who would see each other.
    #[serde(default)]
    pub yes: bool,
    /// Only show what would happen.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until it has gone (needs famd; includes the undo-send delay).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for DraftSend {
    type Input = DraftSendInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["draft", "send"],
        summary: "Send a draft",
        description: "Queues a draft to send after the undo-send delay ([app] send_delay_seconds; --now skips it). Undo with fam ops cancel <op> before then. Offline, it waits in the queue until it can go.",
        access: Access::Send,
        errors: ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: DraftSendInput) -> Result<WriteOutput, CommandError> {
        send_at(
            &DaemonPaths::resolve()?,
            i.draft,
            i.now,
            i.yes,
            &ComposeSettings::load()?,
            &flags(i.dry_run, i.wait, i.wait_secs),
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        render(o)
    }
}

/// `fam mail send`.
pub struct MailSend;

/// Input for `fam mail send`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailSendInput {
    /// The account to send from (config name or address); needed with more than one.
    #[serde(default)]
    pub account: Option<String>,
    /// Send from this alias of the account (listed in `aliases` in config.toml).
    #[serde(default)]
    pub from: Option<String>,
    /// To: an address or `Name <address>`; repeat, or separate with commas.
    #[serde(default)]
    pub to: Vec<String>,
    /// Cc.
    #[serde(default)]
    pub cc: Vec<String>,
    /// Bcc.
    #[serde(default)]
    pub bcc: Vec<String>,
    /// Subject.
    #[serde(default)]
    pub subject: Option<String>,
    /// Body text (plain text unless --html).
    #[serde(default)]
    pub body: Option<String>,
    /// Read the body from a file.
    #[serde(default)]
    pub body_file: Option<String>,
    /// The body is HTML.
    #[serde(default)]
    pub html: bool,
    /// Attach a file (up to 3 MB); repeat for more.
    #[serde(default)]
    pub attach: Vec<String>,
    /// Add an inline image.
    #[serde(default)]
    pub inline: Vec<String>,
    /// low, normal or high.
    #[serde(default)]
    pub importance: Option<String>,
    /// Leave the account's signature off.
    #[serde(default)]
    pub no_signature: bool,
    /// Skip the undo-send delay.
    #[serde(default)]
    pub now: bool,
    /// Send even to many outside recipients who would see each other.
    #[serde(default)]
    pub yes: bool,
    /// Only show what would happen.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until it has gone (needs famd; includes the undo-send delay).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for MailSend {
    type Input = MailSendInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "send"],
        summary: "Write and send a message",
        description: "Saves a new message as a draft with the account's signature and queues it to send after the undo-send delay (--now skips it). Undo with fam ops cancel <op> before then.",
        access: Access::Send,
        errors: ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: MailSendInput) -> Result<WriteOutput, CommandError> {
        let args = ComposeArgs {
            account: i.account,
            from: i.from,
            to: i.to,
            cc: i.cc,
            bcc: i.bcc,
            subject: i.subject,
            body: i.body,
            body_file: i.body_file,
            html: i.html,
            attach: i.attach,
            inline: i.inline,
            importance: i.importance,
            no_signature: i.no_signature,
            ..ComposeArgs::default()
        };
        compose_at(
            &DaemonPaths::resolve()?,
            &args,
            &ComposeSettings::load()?,
            Some((i.now, i.yes)),
            &flags(i.dry_run, i.wait, i.wait_secs),
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        render(o)
    }
}
