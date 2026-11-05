// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail read commands (WP 1.7, WP 1.10): `fam mail list`, `fam mail thread`,
//! `fam mail fetch` and `fam mail pin`.
//!
//! `list`, `thread` and `fetch` read the store directly, so they work offline
//! and without `famd`. `fetch` asks `famd` to download a body that isn't on
//! this device; `pin` always goes through `famd`, the store's only writer.

use std::collections::HashMap;

use crate::commands::auth::rfc3339;
use crate::commands::folder::folder_paths;
use crate::daemon::{
    Client, DaemonPaths, FetchBodyParams, FetchBodyResult, METHOD_FETCH_BODY, METHOD_PIN,
    PinParams, PinResult,
};
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use familiar_store::{
    Account, AccountId, Address, Cursor, FolderId, Message, MessageFilter, MessageId, Reader,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Shared helpers

pub(crate) fn store_err(e: familiar_store::Error) -> CommandError {
    match e {
        familiar_store::Error::NotFound(m) => CommandError::new(ErrorKind::NotFound, m),
        e => CommandError::internal(format!("store: {e}")),
    }
}

/// Opens the store read-only; "nothing synced yet" is `not_found`.
pub(crate) fn open_store(paths: &DaemonPaths) -> Result<Reader, CommandError> {
    match Reader::open(&paths.store) {
        Ok(r) => Ok(r),
        Err(familiar_store::Error::NoStore(_)) => Err(CommandError::new(
            ErrorKind::NotFound,
            "nothing has synced on this device yet; start famd first",
        )),
        Err(e) => Err(store_err(e)),
    }
}

/// Every account, or the one named (by config key or address).
pub(crate) fn pick_accounts(r: &Reader, only: Option<&str>) -> Result<Vec<Account>, CommandError> {
    let mut all = r.accounts().map_err(store_err)?;
    all.sort_by(|a, b| a.key.cmp(&b.key));
    let picked: Vec<Account> = all
        .into_iter()
        .filter(|a| {
            only.is_none_or(|k| a.key.eq_ignore_ascii_case(k) || a.email.eq_ignore_ascii_case(k))
        })
        .collect();
    if let (Some(k), true) = (only, picked.is_empty()) {
        return Err(CommandError::new(
            ErrorKind::NotFound,
            format!("account {k:?} has not synced on this device"),
        ));
    }
    Ok(picked)
}

pub(crate) fn parse_cursor(s: &str) -> Result<Cursor, CommandError> {
    let bad = || CommandError::usage(format!("--after {s:?} isn't a next-page token"));
    let (t, id) = s.split_once('.').ok_or_else(bad)?;
    Ok(Cursor {
        received_at: t.parse().map_err(|_| bad())?,
        id: id.parse().map_err(|_| bad())?,
    })
}

pub(crate) fn cursor_token(c: &Cursor) -> String {
    format!("{}.{}", c.received_at, c.id)
}

fn time(secs: i64) -> String {
    rfc3339(secs.max(0) as u64)
}

fn short_date(rfc: &str) -> &str {
    rfc.get(..10).unwrap_or(rfc)
}

fn fit(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        format!("{s:<n$}")
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// A mailbox address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MailAddress {
    /// Display name, possibly empty.
    pub name: String,
    /// Email address.
    pub addr: String,
}

impl From<&Address> for MailAddress {
    fn from(a: &Address) -> Self {
        Self {
            name: a.name.clone(),
            addr: a.addr.clone(),
        }
    }
}

impl MailAddress {
    fn label(&self) -> &str {
        if self.name.is_empty() {
            &self.addr
        } else {
            &self.name
        }
    }
}

/// Message importance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Importance {
    /// Low.
    Low,
    /// Normal.
    Normal,
    /// High.
    High,
}

/// A message's header as every mail command shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageHeader {
    /// Local message ID, for `fam mail thread`, `fetch` and `pin`.
    pub id: i64,
    /// Account name.
    pub account: String,
    /// Folder path, such as `Inbox` or `Inbox/Club`.
    pub folder: String,
    /// Local folder ID.
    pub folder_id: i64,
    /// Local conversation ID.
    pub thread: i64,
    /// Subject.
    pub subject: String,
    /// Sender.
    pub from: MailAddress,
    /// To recipients.
    pub to: Vec<MailAddress>,
    /// Cc recipients.
    pub cc: Vec<MailAddress>,
    /// Received time (RFC 3339, UTC).
    pub received_at: String,
    /// Sent time (RFC 3339, UTC), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    /// Read state.
    pub is_read: bool,
    /// Follow-up flag.
    pub is_flagged: bool,
    /// Importance.
    pub importance: Importance,
    /// Has file attachments.
    pub has_attachments: bool,
    /// Short text preview.
    pub preview: String,
    /// Whether the body is on this device.
    pub body_local: bool,
    /// Whether the body is pinned to this device.
    pub body_pinned: bool,
    /// Graph immutable ID, for `fam api`.
    pub remote_id: String,
    /// RFC 5322 Message-ID, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internet_message_id: Option<String>,
}

/// Account and folder names for headers.
pub(crate) struct Names {
    accounts: HashMap<AccountId, String>,
    folders: HashMap<FolderId, String>,
}

impl Names {
    pub(crate) fn load(r: &Reader) -> Result<Self, CommandError> {
        let mut n = Names {
            accounts: HashMap::new(),
            folders: HashMap::new(),
        };
        for a in r.accounts().map_err(store_err)? {
            let folders = r.folders(a.id).map_err(store_err)?;
            n.folders.extend(folder_paths(&folders));
            n.accounts.insert(a.id, a.key);
        }
        Ok(n)
    }

    pub(crate) fn header(&self, m: &Message) -> MessageHeader {
        MessageHeader {
            id: m.id.0,
            account: self.accounts.get(&m.account).cloned().unwrap_or_default(),
            folder: self.folders.get(&m.folder).cloned().unwrap_or_default(),
            folder_id: m.folder.0,
            thread: m.thread.0,
            subject: m.subject.clone(),
            from: (&m.from).into(),
            to: m.to.iter().map(Into::into).collect(),
            cc: m.cc.iter().map(Into::into).collect(),
            received_at: time(m.received_at),
            sent_at: m.sent_at.map(time),
            is_read: m.is_read,
            is_flagged: m.is_flagged,
            importance: match m.importance {
                i if i <= 0 => Importance::Low,
                1 => Importance::Normal,
                _ => Importance::High,
            },
            has_attachments: m.has_attachments,
            preview: m.preview.clone(),
            body_local: m.body.is_some(),
            body_pinned: m.body_pinned,
            remote_id: m.remote_id.clone(),
            internet_message_id: m.internet_message_id.clone(),
        }
    }
}

fn header_line(h: &MessageHeader, place: bool) -> String {
    let mut s = format!(
        "{:>8}  {}  {}{} {}  {}",
        h.id,
        short_date(&h.received_at),
        if h.is_read { ' ' } else { '*' },
        if h.is_flagged { '!' } else { ' ' },
        fit(h.from.label(), 22),
        fit(&h.subject, 50),
    );
    if place {
        s.push_str(&format!("  [{}/{}]", h.account, h.folder));
    }
    s
}

fn message(r: &Reader, id: i64) -> Result<Message, CommandError> {
    r.message(MessageId(id)).map_err(store_err)?.ok_or_else(|| {
        CommandError::new(
            ErrorKind::NotFound,
            format!("no message {id} on this device"),
        )
    })
}

// ---------------------------------------------------------------------------
// fam mail list

/// `fam mail list`.
pub struct MailList;

/// Input for `fam mail list`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailListInput {
    /// Only this account (its name in config.toml, or its address). Default: every account.
    #[serde(default)]
    pub account: Option<String>,
    /// Folder name, path (Inbox/Club) or well-known name (inbox, sentitems,
    /// drafts, deleteditems, archive, junkemail). Default: inbox.
    #[serde(default)]
    pub folder: Option<String>,
    /// Only unread messages.
    #[serde(default)]
    pub unread: bool,
    /// Only flagged messages.
    #[serde(default)]
    pub flagged: bool,
    /// Most messages per page (default 50, at most 500).
    #[serde(default)]
    pub limit: Option<u32>,
    /// The next-page token from a previous list.
    #[serde(default)]
    pub after: Option<String>,
}

/// A folder a list covered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListedFolder {
    /// Account name.
    pub account: String,
    /// Folder path.
    pub folder: String,
    /// Local folder ID.
    pub folder_id: i64,
    /// Messages in the folder.
    pub total: i64,
    /// Unread messages in the folder.
    pub unread: i64,
}

/// Output of `fam mail list`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailListOutput {
    /// The folders listed (one per account when no account is named).
    pub folders: Vec<ListedFolder>,
    /// Messages, newest first, across those folders.
    pub messages: Vec<MessageHeader>,
    /// Pass as `--after` for the next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;

/// Lists messages from the store at `paths`. Used by the command and by tests.
pub fn list_at(paths: &DaemonPaths, input: &MailListInput) -> Result<MailListOutput, CommandError> {
    let r = open_store(paths)?;
    let limit = input
        .limit
        .map_or(DEFAULT_LIMIT, |l| l as usize)
        .clamp(1, MAX_LIMIT);
    let after = input.after.as_deref().map(parse_cursor).transpose()?;
    let wanted = input.folder.as_deref().unwrap_or("inbox");
    let filter = MessageFilter {
        unread_only: input.unread,
        flagged_only: input.flagged,
    };

    let mut folders = Vec::new();
    for a in pick_accounts(&r, input.account.as_deref())? {
        let all = r.folders(a.id).map_err(store_err)?;
        let paths = folder_paths(&all);
        let by_path: Vec<_> = all
            .iter()
            .filter(|f| {
                paths
                    .get(&f.id)
                    .is_some_and(|p| p.eq_ignore_ascii_case(wanted))
            })
            .collect();
        let matches = if !by_path.is_empty() {
            by_path
        } else {
            let by_known: Vec<_> = all
                .iter()
                .filter(|f| {
                    f.well_known
                        .as_deref()
                        .is_some_and(|w| w.eq_ignore_ascii_case(wanted))
                })
                .collect();
            if !by_known.is_empty() {
                by_known
            } else {
                all.iter()
                    .filter(|f| f.name.eq_ignore_ascii_case(wanted))
                    .collect()
            }
        };
        if matches.len() > 1 {
            let mut options: Vec<&str> = matches
                .iter()
                .filter_map(|f| paths.get(&f.id).map(String::as_str))
                .collect();
            options.sort_unstable();
            return Err(CommandError::usage(format!(
                "{} has more than one folder called {wanted:?}; give its path: {}",
                a.key,
                options.join(", ")
            )));
        }
        if let Some(f) = matches.first() {
            folders.push(ListedFolder {
                account: a.key.clone(),
                folder: paths.get(&f.id).cloned().unwrap_or_default(),
                folder_id: f.id.0,
                total: f.total,
                unread: f.unread,
            });
        }
    }
    if folders.is_empty() {
        let whose = input
            .account
            .as_deref()
            .map_or_else(|| "any account".to_string(), |a| format!("account {a:?}"));
        return Err(CommandError::new(
            ErrorKind::NotFound,
            format!("{whose} has no folder {wanted:?} on this device (see fam folder list)"),
        ));
    }

    // One page from each folder, one row more than needed so a full page
    // knows there is another.
    let mut rows = Vec::new();
    for f in &folders {
        rows.extend(
            r.messages_filtered(FolderId(f.folder_id), filter, after, limit + 1)
                .map_err(store_err)?,
        );
    }
    rows.sort_by_key(|m| std::cmp::Reverse((m.received_at, m.id.0)));
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next = more.then(|| {
        let last = rows.last().expect("a full page");
        cursor_token(&Cursor {
            received_at: last.received_at,
            id: last.id.0,
        })
    });
    let names = Names::load(&r)?;
    Ok(MailListOutput {
        folders,
        messages: rows.iter().map(|m| names.header(m)).collect(),
        next,
    })
}

impl Command for MailList {
    type Input = MailListInput;
    type Output = MailListOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "list"],
        summary: "List messages in a folder",
        description: "Lists a folder's messages newest first, from this device, so it works offline and without famd. Without --account it lists that folder in every account, merged. --folder takes a name, a path such as Inbox/Club, or a well-known name such as sentitems; the default is the Inbox. --after takes the next-page token.",
        access: Access::Read,
        errors: &[ErrorKind::Usage, ErrorKind::NotFound],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: MailListInput,
    ) -> Result<MailListOutput, CommandError> {
        list_at(&DaemonPaths::resolve()?, &input)
    }

    fn render_text(&self, o: &MailListOutput) -> String {
        let mut s = String::new();
        let place = o.folders.len() > 1;
        let names: Vec<String> = o
            .folders
            .iter()
            .map(|f| format!("{}/{} ({} unread)", f.account, f.folder, f.unread))
            .collect();
        s.push_str(&names.join(", "));
        s.push('\n');
        if o.messages.is_empty() {
            s.push_str("No messages.\n");
        }
        for h in &o.messages {
            s.push_str(&header_line(h, place));
            s.push('\n');
        }
        if let Some(n) = &o.next {
            s.push_str(&format!("More: --after {n}\n"));
        }
        s.trim_end().to_string()
    }
}

// ---------------------------------------------------------------------------
// fam mail thread

/// `fam mail thread`.
pub struct MailThread;

/// Input for `fam mail thread`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailThreadInput {
    /// The local ID of any message in the conversation.
    #[schemars(extend("x-positional" = 0))]
    pub message: i64,
    /// Leave out messages in Deleted Items and Junk Email.
    #[serde(default)]
    pub no_trash: bool,
}

/// A conversation's message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadMessage {
    /// The header. Where there are copies this is the one shown; it is unread
    /// if any copy is.
    #[serde(flatten)]
    pub header: MessageHeader,
    /// Sent from the account's own address.
    pub from_me: bool,
    /// In Deleted Items or Junk Email.
    pub in_trash: bool,
    /// Local IDs of other copies of the same message (same Message-ID), such
    /// as mail sent to yourself, which is in both Inbox and Sent Items.
    pub copies: Vec<i64>,
}

/// Messages of the conversation in one folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadFolder {
    /// Folder path.
    pub folder: String,
    /// Local folder ID.
    pub folder_id: i64,
    /// Messages (copies included) in the folder.
    pub count: usize,
}

/// Output of `fam mail thread`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailThreadOutput {
    /// Local conversation ID.
    pub thread: i64,
    /// Account name.
    pub account: String,
    /// Graph conversation ID.
    pub conversation_id: String,
    /// Subject without RE:/FW: prefixes.
    pub subject: String,
    /// Everyone on the conversation, in order of first appearance.
    pub participants: Vec<MailAddress>,
    /// Messages after copies are merged (trash included).
    pub message_count: usize,
    /// Messages with an unread copy.
    pub unread_count: usize,
    /// Newest message time (RFC 3339, UTC).
    pub last_received_at: String,
    /// Where the conversation's messages are.
    pub folders: Vec<ThreadFolder>,
    /// The messages, oldest first, across every folder.
    pub messages: Vec<ThreadMessage>,
}

/// Assembles the conversation of a message from the store at `paths`.
pub fn thread_at(
    paths: &DaemonPaths,
    input: &MailThreadInput,
) -> Result<MailThreadOutput, CommandError> {
    let r = open_store(paths)?;
    let c = r
        .conversation_of(MessageId(input.message))
        .map_err(store_err)?
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::NotFound,
                format!("no message {} on this device", input.message),
            )
        })?;
    let names = Names::load(&r)?;
    let messages = c
        .messages
        .iter()
        .filter(|m| !(input.no_trash && m.in_trash))
        .map(|m| ThreadMessage {
            header: names.header(&m.message),
            from_me: m.from_me,
            in_trash: m.in_trash,
            copies: m.copies.iter().map(|k| k.id.0).collect(),
        })
        .collect();
    Ok(MailThreadOutput {
        thread: c.thread.0,
        account: names.accounts.get(&c.account).cloned().unwrap_or_default(),
        conversation_id: c.conversation_id.clone(),
        subject: c.subject.clone(),
        participants: c.participants.iter().map(Into::into).collect(),
        message_count: c.message_count,
        unread_count: c.unread_count,
        last_received_at: time(c.last_received_at),
        folders: c
            .folders
            .iter()
            .map(|f| ThreadFolder {
                folder: names
                    .folders
                    .get(&f.folder)
                    .cloned()
                    .unwrap_or(f.name.clone()),
                folder_id: f.folder.0,
                count: f.count,
            })
            .collect(),
        messages,
    })
}

impl Command for MailThread {
    type Input = MailThreadInput;
    type Output = MailThreadOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "thread"],
        summary: "Show a whole conversation",
        description: "Shows the conversation a message belongs to, oldest first, across every folder of its account: replies in Sent Items, drafts and moved messages included, with copies of one message shown once. Reads this device only. Use fam mail fetch for a message's text.",
        access: Access::Read,
        errors: &[ErrorKind::NotFound],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: MailThreadInput,
    ) -> Result<MailThreadOutput, CommandError> {
        thread_at(&DaemonPaths::resolve()?, &input)
    }

    fn render_text(&self, o: &MailThreadOutput) -> String {
        let mut s = format!(
            "{}  ({} message{}, {} unread)  [{}]\n",
            o.subject,
            o.message_count,
            if o.message_count == 1 { "" } else { "s" },
            o.unread_count,
            o.account
        );
        for m in &o.messages {
            s.push_str(&header_line(&m.header, false));
            s.push_str(&format!("  [{}]", m.header.folder));
            if m.in_trash {
                s.push_str(" (deleted)");
            }
            s.push('\n');
        }
        s.trim_end().to_string()
    }
}

// ---------------------------------------------------------------------------
// fam mail fetch

/// `fam mail fetch`.
pub struct MailFetch;

/// How to return a body.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BodyFormat {
    /// Plain text; HTML bodies are converted.
    #[default]
    Text,
    /// The body as stored: HTML when the message is HTML.
    Html,
}

/// Input for `fam mail fetch`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailFetchInput {
    /// The message's local ID.
    #[schemars(extend("x-positional" = 0))]
    pub message: i64,
    /// text (default) converts HTML to plain text; html returns the body as stored.
    #[serde(default)]
    pub format: BodyFormat,
    /// Don't download: show the body only if it is already on this device.
    #[serde(default)]
    pub local: bool,
}

/// The body's content type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BodyType {
    /// Plain text.
    Text,
    /// HTML.
    Html,
}

/// Output of `fam mail fetch`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MailFetchOutput {
    /// The message's header.
    #[serde(flatten)]
    pub header: MessageHeader,
    /// The body; absent with `--local` when it isn't on this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The type of `body` as returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_type: Option<BodyType>,
    /// Whether the body was downloaded for this request.
    pub downloaded: bool,
    /// Whether the body is kept in the on-demand cache (it is outside the
    /// sync window and not pinned), so it may be evicted later.
    pub cached: bool,
}

/// Shows a message and its body from the store at `paths`, asking the
/// daemon there to download the body if it isn't local.
pub fn fetch_at(
    paths: &DaemonPaths,
    input: &MailFetchInput,
) -> Result<MailFetchOutput, CommandError> {
    let r = open_store(paths)?;
    let mut m = message(&r, input.message)?;
    let mut downloaded = false;
    if m.body.is_none() && !input.local {
        let got: FetchBodyResult = Client::connect(&paths.socket)
            .map_err(|e| {
                if e.kind == ErrorKind::DaemonUnavailable {
                    CommandError::new(
                        ErrorKind::DaemonUnavailable,
                        format!(
                            "message {}'s body isn't on this device and famd isn't running to download it; start famd, or use --local for the header only",
                            input.message
                        ),
                    )
                } else {
                    e
                }
            })?
            .call(
                METHOD_FETCH_BODY,
                &FetchBodyParams {
                    message: input.message,
                },
            )?;
        downloaded = got.fetched;
        m = message(&r, input.message)?;
    }
    let names = Names::load(&r)?;
    let (body, body_type) = match m.body {
        Some(b) => {
            let raw = r.body(&b.hash).map_err(store_err)?;
            let text = String::from_utf8_lossy(&raw).into_owned();
            match (b.is_html, input.format) {
                (true, BodyFormat::Text) => (
                    Some(familiar_search::text::html_to_lines(&text)),
                    Some(BodyType::Text),
                ),
                (true, BodyFormat::Html) => (Some(text), Some(BodyType::Html)),
                (false, _) => (Some(text), Some(BodyType::Text)),
            }
        }
        None => (None, None),
    };
    Ok(MailFetchOutput {
        header: names.header(&m),
        body,
        body_type,
        downloaded,
        cached: m.body_cached_at.is_some(),
    })
}

impl Command for MailFetch {
    type Input = MailFetchInput;
    type Output = MailFetchOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "fetch"],
        summary: "Show a message with its text",
        description: "Shows a message's header and body. Bodies in the sync window are already on this device; an older one is downloaded through famd into the size-limited cache (sync.cache_limit). --local never downloads. --format html returns the body as stored instead of plain text.",
        access: Access::Read,
        errors: &[
            ErrorKind::NotFound,
            ErrorKind::DaemonUnavailable,
            ErrorKind::Offline,
            ErrorKind::AuthRequired,
            ErrorKind::PermissionDenied,
            ErrorKind::Throttled,
            ErrorKind::Server,
        ],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: MailFetchInput,
    ) -> Result<MailFetchOutput, CommandError> {
        fetch_at(&DaemonPaths::resolve()?, &input)
    }

    fn render_text(&self, o: &MailFetchOutput) -> String {
        let h = &o.header;
        let list = |v: &[MailAddress]| {
            v.iter()
                .map(|a| {
                    if a.name.is_empty() {
                        a.addr.clone()
                    } else {
                        format!("{} <{}>", a.name, a.addr)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut s = format!(
            "From:    {}\nTo:      {}\n",
            list(std::slice::from_ref(&h.from)),
            list(&h.to)
        );
        if !h.cc.is_empty() {
            s.push_str(&format!("Cc:      {}\n", list(&h.cc)));
        }
        s.push_str(&format!(
            "Date:    {}\nSubject: {}\nFolder:  {}/{}  (id {})\n",
            h.received_at, h.subject, h.account, h.folder, h.id
        ));
        if h.has_attachments {
            s.push_str("Has attachments.\n");
        }
        s.push('\n');
        match &o.body {
            Some(b) => s.push_str(b.trim_end()),
            None => {
                s.push_str("(The body isn't on this device; run without --local to download it.)")
            }
        }
        s
    }
}

// ---------------------------------------------------------------------------
// fam mail pin

/// `fam mail pin`: keep a message's body on this device whatever its age.
///
/// Pinning changes only what this device keeps, never the mailbox, so it is
/// a read-level command. It goes through `famd`, the store's only writer,
/// which downloads the body first if it isn't local.
pub struct MailPin;

/// Input for `fam mail pin`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailPinInput {
    /// The message's local ID.
    #[schemars(extend("x-positional" = 0))]
    pub message: i64,
    /// Unpin instead: the body becomes an ordinary cached body again.
    #[serde(default)]
    pub unpin: bool,
}

impl Command for MailPin {
    type Input = MailPinInput;
    type Output = PinResult;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "pin"],
        summary: "Keep a message's body on this device",
        description: "Pins a message so its body stays on this device even outside the sync window, downloading it first if needed. --unpin lets it age out again. Needs famd.",
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

    fn run(&self, _ctx: &Context<'_>, input: MailPinInput) -> Result<PinResult, CommandError> {
        Client::connect_default()?.call(
            METHOD_PIN,
            &PinParams {
                message: input.message,
                pinned: !input.unpin,
            },
        )
    }

    fn render_text(&self, o: &PinResult) -> String {
        match (o.pinned, o.fetched) {
            (true, true) => format!("Pinned message {} (body downloaded).", o.message),
            (true, false) => format!("Pinned message {}.", o.message),
            (false, _) => format!("Unpinned message {}.", o.message),
        }
    }
}
