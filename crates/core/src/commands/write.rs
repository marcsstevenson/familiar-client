// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail and folder writes (WP 2.1, 2.2) and the op queue commands.
//!
//! Every write changes the local store at once and queues an op for the
//! server; see [`crate::ops`]. `--wait` waits until the server has it (and
//! needs famd), `--dry-run` only says what would happen. Without famd a
//! write is still made and queued; famd sends it when it starts.

use std::time::Duration;

use crate::daemon::{
    Client, DaemonPaths, METHOD_OPS_CANCEL, METHOD_OPS_LIST, METHOD_OPS_RETRY, METHOD_OPS_SUBMIT,
    METHOD_OPS_WAIT, OpIdParams, OpsListParams,
};
use crate::error::{CommandError, ErrorKind};
use crate::ops::{self, OpView, SubmitParams, WaitParams, WriteOutput, WriteRequest};
use crate::registry::{Access, Command, CommandSpec, Context};
use familiar_store::{OpId, Reader, Writer};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Default `--wait` limit.
pub const DEFAULT_WAIT_SECS: u64 = 60;

const WRITE_ERRORS: &[ErrorKind] = &[
    ErrorKind::NotFound,
    ErrorKind::Conflict,
    ErrorKind::PermissionDenied,
    ErrorKind::ConfirmationRequired,
    ErrorKind::DaemonUnavailable,
    ErrorKind::Offline,
];

fn store_err(e: familiar_store::Error) -> CommandError {
    match e {
        familiar_store::Error::NotFound(m) => CommandError::new(ErrorKind::NotFound, m),
        e => CommandError::internal(format!("store: {e}")),
    }
}

/// Opens the store's writer when famd isn't running.
fn direct_writer(paths: &DaemonPaths) -> Result<Writer, CommandError> {
    match Reader::open(&paths.store) {
        Ok(_) => {}
        Err(familiar_store::Error::NoStore(_)) => {
            return Err(CommandError::new(
                ErrorKind::NotFound,
                "nothing has synced on this device yet; start famd first",
            ));
        }
        Err(familiar_store::Error::SchemaTooOld { .. }) => {
            return Err(CommandError::new(
                ErrorKind::DaemonUnavailable,
                "the store needs migrating; start famd",
            ));
        }
        Err(e) => return Err(store_err(e)),
    }
    match Writer::open(&paths.store) {
        Ok(w) => Ok(w),
        Err(familiar_store::Error::WriterLocked(_)) => Err(CommandError::new(
            ErrorKind::DaemonUnavailable,
            "famd holds the store but its socket isn't answering",
        )),
        Err(e) => Err(store_err(e)),
    }
}

/// How a write runs.
#[derive(Debug, Clone, Default)]
pub struct WriteFlags {
    /// Only say what would happen.
    pub dry_run: bool,
    /// Wait for the server.
    pub wait: bool,
    /// Limit for `wait`.
    pub wait_secs: Option<u64>,
    /// Who asks.
    pub source: String,
}

/// Plans, applies and queues `request` through famd, or directly when famd
/// isn't running (then `--wait` is refused before anything changes).
pub fn submit_at(
    paths: &DaemonPaths,
    request: WriteRequest,
    flags: &WriteFlags,
) -> Result<WriteOutput, CommandError> {
    let params = SubmitParams {
        request,
        source: if flags.source.is_empty() {
            "cli".into()
        } else {
            flags.source.clone()
        },
        dry_run: flags.dry_run,
    };
    let secs = flags.wait_secs.unwrap_or(DEFAULT_WAIT_SECS);
    match Client::connect_with_timeout(&paths.socket, Duration::from_secs(secs + 30)) {
        Ok(mut c) => {
            let mut out: WriteOutput = c.call(METHOD_OPS_SUBMIT, &params)?;
            if flags.wait && !out.dry_run {
                let ids: Vec<i64> = out.ops.iter().filter_map(|o| o.id).collect();
                if !ids.is_empty() {
                    let views: Vec<OpView> = c.call(
                        METHOD_OPS_WAIT,
                        &WaitParams {
                            ops: ids,
                            timeout_secs: secs,
                        },
                    )?;
                    for o in &mut out.ops {
                        match views.iter().find(|v| Some(v.id) == o.id) {
                            Some(v) => {
                                o.state = v.state.clone();
                                o.error = v.error.clone();
                            }
                            None => o.state = "cancelled".into(),
                        }
                    }
                }
                out.waited = true;
                if let Some(f) = out.ops.iter().find(|o| o.state == "failed") {
                    return Err(CommandError::new(
                        ErrorKind::Server,
                        format!(
                            "{} failed: {}",
                            f.summary,
                            f.error.as_deref().unwrap_or("refused")
                        ),
                    ));
                }
            }
            Ok(out)
        }
        Err(e) if e.kind == ErrorKind::DaemonUnavailable => {
            if flags.wait && !flags.dry_run {
                return Err(CommandError::new(
                    ErrorKind::DaemonUnavailable,
                    "--wait needs famd running to send the change; nothing was changed",
                ));
            }
            let mut w = direct_writer(paths)?;
            ops::with_tx(&mut w, |tx| ops::submit(tx, &params))
        }
        Err(e) => Err(e),
    }
}

fn flags(dry_run: bool, wait: bool, wait_secs: Option<u32>) -> WriteFlags {
    WriteFlags {
        dry_run,
        wait,
        wait_secs: wait_secs.map(u64::from),
        source: String::new(),
    }
}

fn run_write(
    request: WriteRequest,
    dry_run: bool,
    wait: bool,
    wait_secs: Option<u32>,
) -> Result<WriteOutput, CommandError> {
    submit_at(
        &DaemonPaths::resolve()?,
        request,
        &flags(dry_run, wait, wait_secs),
    )
}

fn need_yes(yes: bool, dry_run: bool, what: &str) -> Result<(), CommandError> {
    if yes || dry_run {
        return Ok(());
    }
    Err(CommandError::new(
        ErrorKind::ConfirmationRequired,
        format!(
            "{what} can't be undone; add --yes to go ahead (or --dry-run to see what it would do)"
        ),
    ))
}

// ---------------------------------------------------------------------------
// Mail

/// `fam mail read`.
pub struct MailRead;

/// Input for `fam mail read`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailReadInput {
    /// Local message IDs (from fam mail list).
    #[schemars(extend("x-positional" = 0))]
    pub messages: Vec<i64>,
    /// Mark unread instead.
    #[serde(default)]
    pub unread: bool,
    /// Only show what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the change (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for MailRead {
    type Input = MailReadInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "read"],
        summary: "Mark messages read or unread",
        description: "Marks messages read (or unread with --unread) on this device at once and queues the change for the server. Works offline; famd sends queued changes when it can.",
        access: Access::Write,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: MailReadInput) -> Result<WriteOutput, CommandError> {
        run_write(
            WriteRequest::MarkRead {
                messages: i.messages,
                read: !i.unread,
            },
            i.dry_run,
            i.wait,
            i.wait_secs,
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

/// `fam mail flag`.
pub struct MailFlag;

/// Input for `fam mail flag`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailFlagInput {
    /// Local message IDs.
    #[schemars(extend("x-positional" = 0))]
    pub messages: Vec<i64>,
    /// Clear the flag instead.
    #[serde(default)]
    pub clear: bool,
    /// Only show what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the change (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for MailFlag {
    type Input = MailFlagInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "flag"],
        summary: "Flag messages for follow-up, or clear the flag",
        description: "Flags messages (or clears their flags with --clear) on this device at once and queues the change for the server.",
        access: Access::Write,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: MailFlagInput) -> Result<WriteOutput, CommandError> {
        run_write(
            WriteRequest::Flag {
                messages: i.messages,
                flagged: !i.clear,
            },
            i.dry_run,
            i.wait,
            i.wait_secs,
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

/// `fam mail move`.
pub struct MailMove;

/// Input for `fam mail move`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailMoveInput {
    /// Local message IDs.
    #[schemars(extend("x-positional" = 0))]
    pub messages: Vec<i64>,
    /// Destination folder: path (Inbox/Club), well-known name or name, in
    /// each message's own account.
    pub to: String,
    /// Only show what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the change (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for MailMove {
    type Input = MailMoveInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "move"],
        summary: "Move messages to another folder",
        description: "Moves messages to a folder of the same account on this device at once and queues the move for the server.",
        access: Access::Write,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: MailMoveInput) -> Result<WriteOutput, CommandError> {
        run_write(
            WriteRequest::Move {
                messages: i.messages,
                to: i.to,
            },
            i.dry_run,
            i.wait,
            i.wait_secs,
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

/// `fam mail archive`.
pub struct MailArchive;

/// Input for `fam mail archive`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailArchiveInput {
    /// Local message IDs.
    #[schemars(extend("x-positional" = 0))]
    pub messages: Vec<i64>,
    /// Only show what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the change (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for MailArchive {
    type Input = MailArchiveInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "archive"],
        summary: "Move messages to Archive",
        description: "Moves messages to their account's Archive folder, on this device at once and queued for the server.",
        access: Access::Write,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: MailArchiveInput) -> Result<WriteOutput, CommandError> {
        run_write(
            WriteRequest::Archive {
                messages: i.messages,
            },
            i.dry_run,
            i.wait,
            i.wait_secs,
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

/// `fam mail delete`.
pub struct MailDelete;

/// Input for `fam mail delete`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailDeleteInput {
    /// Local message IDs.
    #[schemars(extend("x-positional" = 0))]
    pub messages: Vec<i64>,
    /// Delete for good. Only for messages already in Deleted Items; needs --yes.
    #[serde(default)]
    pub permanent: bool,
    /// Confirm a permanent delete when not interactive.
    #[serde(default)]
    pub yes: bool,
    /// Only show what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the change (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for MailDelete {
    type Input = MailDeleteInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mail", "delete"],
        summary: "Move messages to Deleted Items",
        description: "Moves messages to their account's Deleted Items folder, on this device at once and queued for the server. With --permanent (and --yes), deletes messages already in Deleted Items for good.",
        access: Access::Delete,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: MailDeleteInput) -> Result<WriteOutput, CommandError> {
        if i.permanent {
            need_yes(i.yes, i.dry_run, "a permanent delete")?;
        }
        run_write(
            WriteRequest::Delete {
                messages: i.messages,
                permanent: i.permanent,
            },
            i.dry_run,
            i.wait,
            i.wait_secs,
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

// ---------------------------------------------------------------------------
// Folders

/// `fam folder create`.
pub struct FolderCreate;

/// Input for `fam folder create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FolderCreateInput {
    /// The new folder's name.
    #[schemars(extend("x-positional" = 0))]
    pub name: String,
    /// Parent folder (path, well-known name or name). Default: the top level.
    #[serde(default)]
    pub parent: Option<String>,
    /// The account (config name or address); needed with more than one.
    #[serde(default)]
    pub account: Option<String>,
    /// Only show what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the change (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for FolderCreate {
    type Input = FolderCreateInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["folder", "create"],
        summary: "Create a mail folder",
        description: "Creates a folder on this device at once (messages can be moved into it straight away) and queues it for the server.",
        access: Access::Write,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: FolderCreateInput) -> Result<WriteOutput, CommandError> {
        run_write(
            WriteRequest::FolderCreate {
                account: i.account,
                name: i.name,
                parent: i.parent,
            },
            i.dry_run,
            i.wait,
            i.wait_secs,
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

/// `fam folder rename`.
pub struct FolderRename;

/// Input for `fam folder rename`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FolderRenameInput {
    /// The folder (path, or name).
    #[schemars(extend("x-positional" = 0))]
    pub folder: String,
    /// Its new name.
    #[schemars(extend("x-positional" = 1))]
    pub name: String,
    /// The account; needed with more than one.
    #[serde(default)]
    pub account: Option<String>,
    /// Only show what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the change (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for FolderRename {
    type Input = FolderRenameInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["folder", "rename"],
        summary: "Rename a mail folder",
        description: "Renames a folder on this device at once and queues it for the server. Built-in folders can't be renamed.",
        access: Access::Write,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: FolderRenameInput) -> Result<WriteOutput, CommandError> {
        run_write(
            WriteRequest::FolderRename {
                account: i.account,
                folder: i.folder,
                name: i.name,
            },
            i.dry_run,
            i.wait,
            i.wait_secs,
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

/// `fam folder move`.
pub struct FolderMove;

/// Input for `fam folder move`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FolderMoveInput {
    /// The folder (path, or name).
    #[schemars(extend("x-positional" = 0))]
    pub folder: String,
    /// The new parent folder.
    #[serde(default)]
    pub to: Option<String>,
    /// Move it to the top level instead.
    #[serde(default)]
    pub top: bool,
    /// The account; needed with more than one.
    #[serde(default)]
    pub account: Option<String>,
    /// Only show what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the change (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for FolderMove {
    type Input = FolderMoveInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["folder", "move"],
        summary: "Move a mail folder under another, or to the top level",
        description: "Moves a folder (with everything in it) under --to, or to the top level with --top, on this device at once and queued for the server.",
        access: Access::Write,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: FolderMoveInput) -> Result<WriteOutput, CommandError> {
        if i.to.is_some() == i.top {
            return Err(CommandError::usage("give one of --to <folder> or --top"));
        }
        run_write(
            WriteRequest::FolderMove {
                account: i.account,
                folder: i.folder,
                to: i.to,
            },
            i.dry_run,
            i.wait,
            i.wait_secs,
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

/// `fam folder delete`.
pub struct FolderDelete;

/// Input for `fam folder delete`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FolderDeleteInput {
    /// The folder (path, or name).
    #[schemars(extend("x-positional" = 0))]
    pub folder: String,
    /// Delete for good. Only for a folder already in Deleted Items; needs --yes.
    #[serde(default)]
    pub permanent: bool,
    /// Confirm a permanent delete when not interactive.
    #[serde(default)]
    pub yes: bool,
    /// The account; needed with more than one.
    #[serde(default)]
    pub account: Option<String>,
    /// Only show what would change.
    #[serde(default)]
    pub dry_run: bool,
    /// Wait until the server has the change (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for FolderDelete {
    type Input = FolderDeleteInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["folder", "delete"],
        summary: "Delete a mail folder (to Deleted Items)",
        description: "Moves a folder and everything in it into Deleted Items, as Outlook does. With --permanent (and --yes), deletes a folder already in Deleted Items for good.",
        access: Access::Delete,
        errors: WRITE_ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, i: FolderDeleteInput) -> Result<WriteOutput, CommandError> {
        if i.permanent {
            need_yes(i.yes, i.dry_run, "a permanent delete")?;
        }
        run_write(
            WriteRequest::FolderDelete {
                account: i.account,
                folder: i.folder,
                permanent: i.permanent,
            },
            i.dry_run,
            i.wait,
            i.wait_secs,
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        o.render()
    }
}

// ---------------------------------------------------------------------------
// The queue

/// `fam ops list`.
pub struct OpsList;

/// Input for `fam ops list`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpsListInput {
    /// Also show changes the server got in the last hour.
    #[serde(default)]
    pub all: bool,
    /// Most rows (default 100).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Output of `fam ops list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpsListOutput {
    /// Ops, newest first.
    pub ops: Vec<OpView>,
}

/// Lists ops from famd, or from the store when famd isn't running.
pub fn ops_list_at(paths: &DaemonPaths, i: &OpsListInput) -> Result<OpsListOutput, CommandError> {
    let p = OpsListParams {
        done_within_secs: if i.all { 3600 } else { 0 },
        limit: i.limit.map_or(100, |l| l as usize),
    };
    match Client::connect(&paths.socket) {
        Ok(mut c) => Ok(OpsListOutput {
            ops: c.call(METHOD_OPS_LIST, &p)?,
        }),
        Err(e) if e.kind == ErrorKind::DaemonUnavailable => {
            let r = crate::commands::mail::open_store(paths)?;
            let accounts = r.accounts().map_err(store_err)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            let since = if i.all { now - 3600 } else { i64::MAX };
            let ops = r
                .recent_ops(since, p.limit)
                .map_err(store_err)?
                .iter()
                .map(|o| {
                    let key = accounts
                        .iter()
                        .find(|a| a.id == o.account)
                        .map(|a| a.key.clone())
                        .unwrap_or_default();
                    ops::view(o, &key)
                })
                .collect();
            Ok(OpsListOutput { ops })
        }
        Err(e) => Err(e),
    }
}

impl Command for OpsList {
    type Input = OpsListInput;
    type Output = OpsListOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["ops", "list"],
        summary: "List changes waiting for the server",
        description: "Lists queued changes that haven't reached the server yet, and ones that failed. --all adds those done in the last hour.",
        access: Access::Read,
        errors: &[ErrorKind::NotFound],
    };

    fn run(&self, _ctx: &Context<'_>, i: OpsListInput) -> Result<OpsListOutput, CommandError> {
        ops_list_at(&DaemonPaths::resolve()?, &i)
    }

    fn render_text(&self, o: &OpsListOutput) -> String {
        if o.ops.is_empty() {
            return "Nothing waiting for the server.".into();
        }
        let mut s = String::new();
        for v in &o.ops {
            s.push_str(&format!(
                "{:>6}  {:<9}  {:<10}  {}",
                v.id, v.state, v.account, v.summary
            ));
            if let Some(e) = &v.error {
                s.push_str(&format!("  ({e})"));
            }
            s.push('\n');
        }
        s.pop();
        s
    }
}

/// Input for `fam ops retry` and `fam ops cancel`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpIdInput {
    /// The op's ID (from fam ops list).
    #[schemars(extend("x-positional" = 0))]
    pub id: i64,
}

/// Output of `fam ops retry` and `fam ops cancel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpIdOutput {
    /// The op.
    pub id: i64,
    /// Whether anything changed (false: it was not in a state that allows it).
    pub changed: bool,
}

fn op_call(
    paths: &DaemonPaths,
    method: &str,
    id: i64,
    local: fn(&familiar_store::WriteTx<'_>, OpId) -> Result<bool, CommandError>,
) -> Result<OpIdOutput, CommandError> {
    let changed = match Client::connect(&paths.socket) {
        Ok(mut c) => c.call(method, &OpIdParams { id })?,
        Err(e) if e.kind == ErrorKind::DaemonUnavailable => {
            let mut w = direct_writer(paths)?;
            ops::with_tx(&mut w, |tx| local(tx, OpId(id)))?
        }
        Err(e) => return Err(e),
    };
    Ok(OpIdOutput { id, changed })
}

/// `fam ops retry`.
pub struct OpsRetry;

impl Command for OpsRetry {
    type Input = OpIdInput;
    type Output = OpIdOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["ops", "retry"],
        summary: "Try a failed change again",
        description: "Queues a failed change again, making its local change again first.",
        access: Access::Write,
        errors: &[ErrorKind::NotFound, ErrorKind::DaemonUnavailable],
    };

    fn run(&self, _ctx: &Context<'_>, i: OpIdInput) -> Result<OpIdOutput, CommandError> {
        op_call(&DaemonPaths::resolve()?, METHOD_OPS_RETRY, i.id, ops::retry)
    }

    fn render_text(&self, o: &OpIdOutput) -> String {
        if o.changed {
            format!("Op {} is queued again.", o.id)
        } else {
            format!("Op {} hasn't failed; nothing to retry.", o.id)
        }
    }
}

/// `fam ops cancel`.
pub struct OpsCancel;

impl Command for OpsCancel {
    type Input = OpIdInput;
    type Output = OpIdOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["ops", "cancel"],
        summary: "Cancel a change that hasn't reached the server",
        description: "Removes a queued or failed change and undoes it on this device. A change already on its way or done can't be cancelled.",
        access: Access::Write,
        errors: &[ErrorKind::NotFound, ErrorKind::DaemonUnavailable],
    };

    fn run(&self, _ctx: &Context<'_>, i: OpIdInput) -> Result<OpIdOutput, CommandError> {
        op_call(
            &DaemonPaths::resolve()?,
            METHOD_OPS_CANCEL,
            i.id,
            ops::cancel,
        )
    }

    fn render_text(&self, o: &OpIdOutput) -> String {
        if o.changed {
            format!("Op {} is cancelled and undone here.", o.id)
        } else {
            format!(
                "Op {} is already on its way or done; it can't be cancelled.",
                o.id
            )
        }
    }
}
