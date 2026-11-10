// SPDX-License-Identifier: MIT OR Apache-2.0
//! Follow-up reminders: "remind me if nobody replies in 3 days".
//!
//! `fam followup add <message>` (or `fam draft send --remind-days 3`) notes a
//! message in `followups.json` in the data directory, on this device only.
//! Nothing runs in the background: whenever the reminders are listed, each
//! one is checked against the store. A reminder whose conversation has a
//! message from someone else, newer than the one waiting, has been answered
//! and is dropped. The rest are `waiting` until their time, then `due`.
//!
//! A reminder is keyed by the waiting message's Internet Message-ID, which
//! the sent copy in Sent Items keeps. A draft sent with a reminder has no
//! sent copy yet, so its reminder holds the send op instead; the op learns
//! the Message-ID before it sends, and the next check takes it from there.
//! If the op is gone without one (the send was undone), so is the reminder.

use std::collections::HashMap;
use std::path::Path;

use crate::commands::auth::rfc3339;
use crate::commands::mail::{open_store, pick_accounts, store_err};
use crate::daemon::DaemonPaths;
use crate::error::{CommandError, ErrorKind};
use crate::ops::{OpPayload, WriteOutput};
use crate::registry::{Access, Command, CommandSpec, Context};
use familiar_store::{Message, MessageId, OpId, OpState, Reader};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The file name in the data directory.
pub const FILE: &str = "followups.json";

/// Days a reminder waits when none is given.
pub const DEFAULT_DAYS: u32 = 3;

/// Longest wait accepted, in days.
const MAX_DAYS: u32 = 365;

/// One reminder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUp {
    /// Account key.
    pub account: String,
    /// Internet Message-ID of the message waiting for a reply. Unknown for a
    /// draft until its send op has read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internet_message_id: Option<String>,
    /// The send op of a draft sent with a reminder, until the Message-ID is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_op: Option<i64>,
    /// Subject, for showing the reminder before the sent copy has synced.
    #[serde(default)]
    pub subject: String,
    /// When it was set (Unix seconds).
    pub created_at: i64,
    /// When it is due (Unix seconds).
    pub due_at: i64,
}

/// Every reminder on this device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUps(pub Vec<FollowUp>);

impl FollowUps {
    /// Reads `followups.json` from `data_dir`. A missing file is empty; an
    /// unreadable one is reported on stderr and treated as empty.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(FILE);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("familiar: ignoring an unreadable {}: {e}", path.display());
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                eprintln!("familiar: couldn't read {}: {e}", path.display());
                Self::default()
            }
        }
    }

    /// Writes `followups.json` to `data_dir` through a temporary file.
    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join(FILE);
        // Its own temporary file, so two saves at once (the folder pane and
        // the list both check on a store change) don't trip over each other.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let tmp = data_dir.join(format!("{FILE}.{}.{nanos}.tmp", std::process::id()));
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)
    }

    /// The reminder for a message (by account key and Message-ID), if any.
    pub fn find(&self, account: &str, internet_message_id: &str) -> Option<&FollowUp> {
        self.0.iter().find(|f| {
            f.account == account && f.internet_message_id.as_deref() == Some(internet_message_id)
        })
    }
}

fn save_err(e: std::io::Error) -> CommandError {
    CommandError::internal(format!("save {FILE}: {e}"))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Checks a `--days` / `--remind-days` value.
pub fn check_days(days: Option<u32>) -> Result<(), CommandError> {
    days_secs(days).map(|_| ())
}

fn days_secs(days: Option<u32>) -> Result<i64, CommandError> {
    let d = days.unwrap_or(DEFAULT_DAYS);
    if d == 0 || d > MAX_DAYS {
        return Err(CommandError::usage(format!(
            "--days must be 1 to {MAX_DAYS}"
        )));
    }
    Ok(i64::from(d) * 86_400)
}

/// Where a reminder stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// A draft waiting to go (undo-send delay, or offline).
    Sending,
    /// No reply yet, and not due.
    Waiting,
    /// No reply, and the time is up.
    Due,
    /// Someone has answered.
    Replied,
    /// The send was undone.
    Gone,
}

/// A reminder after checking it against the store.
#[derive(Debug, Clone)]
pub struct Checked {
    /// The reminder, with its Message-ID filled in once known.
    pub entry: FollowUp,
    /// Where it stands.
    pub status: Status,
    /// The waiting message on this device, once synced (the Sent Items copy
    /// when there is one).
    pub message: Option<Message>,
}

/// Each account's own addresses (its address and its aliases), lower case,
/// by account key. Mail from these is never a reply.
pub fn own_addresses(r: &Reader) -> Result<HashMap<String, Vec<String>>, CommandError> {
    // Aliases are only a refinement: an unreadable config.toml still leaves
    // the account's own address.
    let aliases = crate::commands::draft::ComposeSettings::load()
        .map(|s| s.aliases)
        .unwrap_or_default();
    let mut out = HashMap::new();
    for a in r.accounts().map_err(store_err)? {
        let mut mine = vec![a.email.to_lowercase()];
        if let Some((_, list)) = aliases.iter().find(|(k, _)| k.eq_ignore_ascii_case(&a.key)) {
            mine.extend(list.iter().map(|s| s.to_lowercase()));
        }
        out.insert(a.key, mine);
    }
    Ok(out)
}

/// Checks one reminder against the store at `now`.
pub fn check(
    r: &Reader,
    f: &FollowUp,
    own: &HashMap<String, Vec<String>>,
    now: i64,
) -> Result<Checked, CommandError> {
    let mut entry = f.clone();
    let open = |entry: FollowUp, message| Checked {
        status: if now >= entry.due_at {
            Status::Due
        } else {
            Status::Waiting
        },
        entry,
        message,
    };
    if entry.internet_message_id.is_none() {
        let op = match entry.send_op {
            Some(id) => r.op(OpId(id)).map_err(store_err)?,
            None => None,
        };
        let Some(op) = op else {
            // Undone (a cancelled op is deleted), or never had a send op.
            return Ok(Checked {
                entry,
                status: Status::Gone,
                message: None,
            });
        };
        if let Ok(OpPayload::SendDraft {
            internet_message_id: Some(id),
            ..
        }) = OpPayload::of(&op)
        {
            entry.internet_message_id = Some(id);
            entry.send_op = None;
        } else if op.state == OpState::Done || op.state == OpState::Failed {
            // Sent without a Message-ID (the draft was gone), or failed.
            return Ok(Checked {
                entry,
                status: Status::Gone,
                message: None,
            });
        } else {
            return Ok(Checked {
                entry,
                status: Status::Sending,
                message: None,
            });
        }
    }
    let imid = entry.internet_message_id.clone().unwrap_or_default();
    let Some(account) = r.account_by_key(&entry.account).map_err(store_err)? else {
        return Ok(open(entry, None));
    };
    let mut copies: Vec<Message> = r
        .messages_by_internet_id(&imid)
        .map_err(store_err)?
        .into_iter()
        .filter(|m| m.account == account.id)
        .collect();
    if copies.is_empty() {
        // Sent, but Sent Items hasn't synced yet.
        return Ok(open(entry, None));
    }
    let Some(conv) = r.conversation_of(copies[0].id).map_err(store_err)? else {
        return Ok(open(entry, None));
    };
    // Prefer the Sent Items copy for showing.
    let sent: Vec<MessageId> = conv
        .messages
        .iter()
        .filter(|m| m.is_sent())
        .map(|m| m.message.id)
        .chain(conv.messages.iter().flat_map(|m| {
            m.copies
                .iter()
                .filter(|c| c.well_known.as_deref() == Some(familiar_store::SENT_ITEMS))
                .map(|c| c.id)
        }))
        .collect();
    copies.sort_by_key(|m| (!sent.contains(&m.id), m.id));
    let waiting = copies.swap_remove(0);
    let mine = own.get(&entry.account).cloned().unwrap_or_default();
    let replied = conv.messages.iter().any(|m| {
        m.message.received_at > waiting.received_at
            && !m.is_sent()
            && !m.is_draft()
            && !mine.contains(&m.message.from.addr.to_lowercase())
    });
    if replied {
        return Ok(Checked {
            entry,
            status: Status::Replied,
            message: Some(waiting),
        });
    }
    Ok(open(entry, Some(waiting)))
}

/// Checks every reminder at `paths` and returns the open ones (sending,
/// waiting or due), soonest due first. Answered and undone reminders are
/// dropped from `followups.json`, and Message-IDs learnt from send ops are
/// kept, so the file only ever holds open reminders.
pub fn check_all_at(paths: &DaemonPaths, now: i64) -> Result<Vec<Checked>, CommandError> {
    let all = FollowUps::load(&paths.data_dir);
    if all.0.is_empty() {
        return Ok(Vec::new());
    }
    let r = open_store(paths)?;
    let own = own_addresses(&r)?;
    let mut checked = Vec::with_capacity(all.0.len());
    for f in &all.0 {
        checked.push(check(&r, f, &own, now)?);
    }
    checked.retain(|c| !matches!(c.status, Status::Replied | Status::Gone));
    let kept = FollowUps(checked.iter().map(|c| c.entry.clone()).collect());
    if kept != all {
        kept.save(&paths.data_dir).map_err(save_err)?;
    }
    checked.sort_by_key(|c| (c.entry.due_at, c.entry.created_at));
    Ok(checked)
}

/// Adds a reminder for the draft sent by `out` (`fam draft send
/// --remind-days`). Does nothing on a dry run.
pub fn remind_after_send(
    paths: &DaemonPaths,
    out: &WriteOutput,
    days: Option<u32>,
) -> Result<(), CommandError> {
    let secs = days_secs(days)?;
    if out.dry_run {
        return Ok(());
    }
    let Some(op) = out.ops.iter().find(|o| o.kind == "send_draft") else {
        return Ok(());
    };
    let Some(id) = op.id else { return Ok(()) };
    let summary = op.summary.clone();
    let mut all = FollowUps::load(&paths.data_dir);
    let created_at = now();
    all.0.push(FollowUp {
        account: op.account.clone(),
        internet_message_id: None,
        send_op: Some(id),
        subject: summary,
        created_at,
        due_at: created_at + secs,
    });
    all.save(&paths.data_dir).map_err(save_err)
}

// ---------------------------------------------------------------------------
// Commands

/// One reminder, as listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FollowUpRow {
    /// Account name.
    pub account: String,
    /// `sending`, `waiting` or `due`.
    pub status: Status,
    /// Local ID of the waiting message, once it is on this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<i64>,
    /// Subject.
    pub subject: String,
    /// Who it went to (To), when the message is on this device.
    pub to: Vec<String>,
    /// When the waiting message was sent (RFC 3339, UTC), once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    /// When the reminder is due (RFC 3339, UTC).
    pub due_at: String,
    /// The same, in Unix seconds.
    pub due_unix: i64,
}

impl From<&Checked> for FollowUpRow {
    fn from(c: &Checked) -> Self {
        let m = c.message.as_ref();
        FollowUpRow {
            account: c.entry.account.clone(),
            status: c.status,
            message: m.map(|m| m.id.0),
            subject: m.map_or_else(|| c.entry.subject.clone(), |m| m.subject.clone()),
            to: m
                .map(|m| {
                    m.to.iter()
                        .map(|a| {
                            if a.name.is_empty() {
                                a.addr.clone()
                            } else {
                                a.name.clone()
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
            sent_at: m.map(|m| rfc3339(m.sent_at.unwrap_or(m.received_at).max(0) as u64)),
            due_at: rfc3339(c.entry.due_at.max(0) as u64),
            due_unix: c.entry.due_at,
        }
    }
}

/// Output of the follow-up commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FollowUpListOutput {
    /// Open reminders, soonest due first.
    pub followups: Vec<FollowUpRow>,
}

fn render_list(o: &FollowUpListOutput) -> String {
    if o.followups.is_empty() {
        return "No follow-up reminders.".into();
    }
    o.followups
        .iter()
        .map(|f| {
            let id = f.message.map_or_else(|| "-".to_string(), |m| m.to_string());
            let status = match f.status {
                Status::Due => "DUE",
                Status::Waiting => "waiting",
                _ => "sending",
            };
            let to = if f.to.is_empty() {
                String::new()
            } else {
                format!(" to {}", f.to.join(", "))
            };
            format!(
                "{id:>8}  {status:<7}  due {}  {}{to} ({})",
                f.due_at.get(..10).unwrap_or(&f.due_at),
                if f.subject.is_empty() {
                    "(no subject)"
                } else {
                    &f.subject
                },
                f.account
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `fam followup list`.
pub struct FollowUpList;

/// Input for `fam followup list`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowUpListInput {
    /// Only this account (its name in config.toml).
    #[serde(default)]
    pub account: Option<String>,
    /// Only reminders that are due (no reply, and the time is up).
    #[serde(default)]
    pub due: bool,
}

/// Lists reminders at `paths`. Used by the command and by tests.
pub fn followup_list_at(
    paths: &DaemonPaths,
    input: &FollowUpListInput,
    now: i64,
) -> Result<FollowUpListOutput, CommandError> {
    let checked = check_all_at(paths, now)?;
    Ok(FollowUpListOutput {
        followups: checked
            .iter()
            .filter(|c| {
                input
                    .account
                    .as_deref()
                    .is_none_or(|a| c.entry.account.eq_ignore_ascii_case(a))
            })
            .filter(|c| !input.due || c.status == Status::Due)
            .map(FollowUpRow::from)
            .collect(),
    })
}

impl Command for FollowUpList {
    type Input = FollowUpListInput;
    type Output = FollowUpListOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["followup", "list"],
        summary: "List follow-up reminders for mail nobody has replied to",
        description: "Lists the reminders set with fam followup add or fam draft send --remind-days, soonest first: waiting (no reply yet), due (no reply and the time is up) or sending (the draft hasn't gone yet). A reminder whose conversation gets a message from someone else is answered and drops off. Kept on this device only (followups.json in the data directory).",
        access: Access::Read,
        errors: &[ErrorKind::NotFound],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: FollowUpListInput,
    ) -> Result<FollowUpListOutput, CommandError> {
        followup_list_at(&DaemonPaths::resolve()?, &input, now())
    }

    fn render_text(&self, o: &FollowUpListOutput) -> String {
        render_list(o)
    }
}

/// `fam followup add`.
pub struct FollowUpAdd;

/// Input for `fam followup add`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowUpAddInput {
    /// Local message IDs, usually from Sent Items.
    #[schemars(extend("x-positional" = 0))]
    pub messages: Vec<i64>,
    /// Days from now until the reminder is due (default 3).
    #[serde(default)]
    pub days: Option<u32>,
}

/// Adds reminders at `paths`. Used by the command and by tests.
pub fn followup_add_at(
    paths: &DaemonPaths,
    input: &FollowUpAddInput,
    now: i64,
) -> Result<FollowUpListOutput, CommandError> {
    let secs = days_secs(input.days)?;
    if input.messages.is_empty() {
        return Err(CommandError::usage("give the messages to follow up"));
    }
    let r = open_store(paths)?;
    let keys: HashMap<_, _> = pick_accounts(&r, None)?
        .into_iter()
        .map(|a| (a.id, a.key))
        .collect();
    let mut all = FollowUps::load(&paths.data_dir);
    for id in &input.messages {
        let m = r
            .message(MessageId(*id))
            .map_err(store_err)?
            .ok_or_else(|| CommandError::new(ErrorKind::NotFound, format!("no message {id}")))?;
        let account = keys.get(&m.account).cloned().unwrap_or_default();
        let Some(imid) = m.internet_message_id.clone() else {
            return Err(CommandError::usage(format!(
                "message {id} has no Internet Message-ID yet; try again once it has synced"
            )));
        };
        let entry = FollowUp {
            account: account.clone(),
            internet_message_id: Some(imid.clone()),
            send_op: None,
            subject: m.subject.clone(),
            created_at: now,
            due_at: now + secs,
        };
        match all.0.iter_mut().find(|f| {
            f.account == account && f.internet_message_id.as_deref() == Some(imid.as_str())
        }) {
            // Setting it again moves the time.
            Some(f) => f.due_at = entry.due_at,
            None => all.0.push(entry),
        }
    }
    all.save(&paths.data_dir).map_err(save_err)?;
    let wanted: Vec<i64> = input.messages.clone();
    let mut out = followup_list_at(paths, &FollowUpListInput::default(), now)?;
    out.followups
        .retain(|f| f.message.is_some_and(|m| wanted.contains(&m)) || f.message.is_none());
    Ok(out)
}

impl Command for FollowUpAdd {
    type Input = FollowUpAddInput;
    type Output = FollowUpListOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["followup", "add"],
        summary: "Remind me if nobody replies to a message",
        description: "Sets a reminder on messages (usually ones you sent): if nobody else has written in the conversation by then, it shows as due in fam followup list and in the desktop app's Follow up view. Due in 3 days unless --days says otherwise; setting it again moves the time. Kept on this device only (followups.json in the data directory), not on the server.",
        access: Access::Write,
        errors: &[ErrorKind::NotFound],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: FollowUpAddInput,
    ) -> Result<FollowUpListOutput, CommandError> {
        followup_add_at(&DaemonPaths::resolve()?, &input, now())
    }

    fn render_text(&self, o: &FollowUpListOutput) -> String {
        render_list(o)
    }
}

/// `fam followup cancel`.
pub struct FollowUpCancel;

/// Input for `fam followup cancel`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowUpCancelInput {
    /// Local message IDs.
    #[schemars(extend("x-positional" = 0))]
    pub messages: Vec<i64>,
}

/// Removes reminders at `paths`. Used by the command and by tests.
pub fn followup_cancel_at(
    paths: &DaemonPaths,
    input: &FollowUpCancelInput,
    now: i64,
) -> Result<FollowUpListOutput, CommandError> {
    let r = open_store(paths)?;
    let keys: HashMap<_, _> = pick_accounts(&r, None)?
        .into_iter()
        .map(|a| (a.id, a.key))
        .collect();
    let mut all = FollowUps::load(&paths.data_dir);
    let before = all.0.len();
    for id in &input.messages {
        let Some(m) = r.message(MessageId(*id)).map_err(store_err)? else {
            return Err(CommandError::new(
                ErrorKind::NotFound,
                format!("no message {id}"),
            ));
        };
        let account = keys.get(&m.account).cloned().unwrap_or_default();
        all.0.retain(|f| {
            f.account != account
                || m.internet_message_id.is_none()
                || f.internet_message_id != m.internet_message_id
        });
    }
    if all.0.len() == before {
        return Err(CommandError::new(
            ErrorKind::NotFound,
            "no follow-up reminder on those messages (see fam followup list)",
        ));
    }
    all.save(&paths.data_dir).map_err(save_err)?;
    followup_list_at(paths, &FollowUpListInput::default(), now)
}

impl Command for FollowUpCancel {
    type Input = FollowUpCancelInput;
    type Output = FollowUpListOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["followup", "cancel"],
        summary: "Cancel follow-up reminders",
        description: "Removes the follow-up reminders on messages. Lists the reminders left.",
        access: Access::Write,
        errors: &[ErrorKind::NotFound],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        input: FollowUpCancelInput,
    ) -> Result<FollowUpListOutput, CommandError> {
        followup_cancel_at(&DaemonPaths::resolve()?, &input, now())
    }

    fn render_text(&self, o: &FollowUpListOutput) -> String {
        render_list(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use familiar_store::{
        AccountId, Address, FolderId, NewAccount, NewFolder, NewMessage, NewOp, Writer,
    };

    const DAY: i64 = 86_400;

    struct Env {
        _dir: tempfile::TempDir,
        paths: DaemonPaths,
        w: Writer,
        acc: AccountId,
        inbox: FolderId,
        sent: FolderId,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let paths = DaemonPaths::at(dir.path());
        let mut w = Writer::open(&paths.store).unwrap();
        let (acc, inbox, sent) = w
            .write(|tx| {
                let acc = tx.upsert_account(&NewAccount {
                    key: "hotmail".into(),
                    email: "pat@example.com".into(),
                    ..Default::default()
                })?;
                let folder = |remote: &str, wk: &str| {
                    tx.upsert_folder(&NewFolder {
                        account: acc,
                        remote_id: remote.into(),
                        parent: None,
                        name: remote.into(),
                        well_known: Some(wk.into()),
                    })
                };
                Ok((acc, folder("Inbox", "inbox")?, folder("Sent", "sentitems")?))
            })
            .unwrap();
        Env {
            _dir: dir,
            paths,
            w,
            acc,
            inbox,
            sent,
        }
    }

    impl Env {
        fn message(&mut self, folder: FolderId, imid: &str, from: &str, at: i64) -> i64 {
            let m = NewMessage {
                account: self.acc,
                folder,
                remote_id: format!("r-{imid}"),
                internet_message_id: Some(imid.into()),
                conversation_id: "conv-1".into(),
                subject: "Quote for the deck".into(),
                from: Address {
                    name: String::new(),
                    addr: from.into(),
                },
                to: vec![Address {
                    name: "Bob".into(),
                    addr: "bob@example.com".into(),
                }],
                received_at: at,
                sent_at: Some(at),
                ..Default::default()
            };
            self.w.write(|tx| tx.upsert_message(&m)).unwrap().0
        }

        fn list(&self, now: i64) -> Vec<FollowUpRow> {
            followup_list_at(&self.paths, &FollowUpListInput::default(), now)
                .unwrap()
                .followups
        }
    }

    #[test]
    fn waits_then_falls_due_and_drops_off_when_someone_replies() {
        let mut e = env();
        let sent = e.message(e.sent, "<s1>", "pat@example.com", 1000);
        let out = followup_add_at(
            &e.paths,
            &FollowUpAddInput {
                messages: vec![sent],
                days: None,
            },
            2000,
        )
        .unwrap();
        assert_eq!(out.followups.len(), 1);
        assert_eq!(out.followups[0].status, Status::Waiting);
        assert_eq!(out.followups[0].message, Some(sent));
        assert_eq!(out.followups[0].to, vec!["Bob".to_string()]);
        assert_eq!(out.followups[0].due_unix, 2000 + 3 * DAY);

        // My own follow-up message isn't a reply.
        e.message(e.sent, "<s2>", "pat@example.com", 5000);
        let rows = e.list(2000 + 3 * DAY);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, Status::Due);
        let due = followup_list_at(
            &e.paths,
            &FollowUpListInput {
                account: None,
                due: true,
            },
            2000,
        )
        .unwrap();
        assert!(due.followups.is_empty(), "not due yet at 2000");

        // Bob answers: the reminder is done, and gone from the file.
        e.message(e.inbox, "<b1>", "bob@example.com", 6000);
        assert!(e.list(2000 + 3 * DAY).is_empty());
        assert!(FollowUps::load(&e.paths.data_dir).0.is_empty());
    }

    #[test]
    fn a_reply_from_before_the_message_does_not_count() {
        let mut e = env();
        e.message(e.inbox, "<b0>", "bob@example.com", 500);
        let sent = e.message(e.sent, "<s1>", "pat@example.com", 1000);
        followup_add_at(
            &e.paths,
            &FollowUpAddInput {
                messages: vec![sent],
                days: Some(1),
            },
            1000,
        )
        .unwrap();
        let rows = e.list(1000 + DAY);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, Status::Due);
    }

    #[test]
    fn cancel_removes_and_adding_again_moves_the_time() {
        let mut e = env();
        let sent = e.message(e.sent, "<s1>", "pat@example.com", 1000);
        let add = |days, now| {
            followup_add_at(
                &e.paths,
                &FollowUpAddInput {
                    messages: vec![sent],
                    days: Some(days),
                },
                now,
            )
            .unwrap()
        };
        add(1, 1000);
        let out = add(7, 2000);
        assert_eq!(out.followups.len(), 1);
        assert_eq!(out.followups[0].due_unix, 2000 + 7 * DAY);
        let left = followup_cancel_at(
            &e.paths,
            &FollowUpCancelInput {
                messages: vec![sent],
            },
            2000,
        )
        .unwrap();
        assert!(left.followups.is_empty());
        let err = followup_cancel_at(
            &e.paths,
            &FollowUpCancelInput {
                messages: vec![sent],
            },
            2000,
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert!(check_days(Some(0)).is_err());
    }

    #[test]
    fn a_sent_draft_is_followed_through_its_send_op() {
        let mut e = env();
        let op = |e: &mut Env, payload: &str| {
            let acc = e.acc;
            e.w.write(|tx| {
                tx.enqueue_op(&NewOp {
                    account: acc,
                    kind: "send_draft".into(),
                    target: "m:9".into(),
                    payload: payload.into(),
                    idempotency_key: None,
                    source: String::new(),
                    summary: "send Quote for the deck".into(),
                    not_before: 0,
                })
            })
            .unwrap()
            .0
        };
        let kept = op(&mut e, r#"{"kind":"send_draft","message":9}"#);
        let undone = op(&mut e, r#"{"kind":"send_draft","message":10}"#);
        let mut all = FollowUps::default();
        for id in [kept, undone] {
            all.0.push(FollowUp {
                account: "hotmail".into(),
                internet_message_id: None,
                send_op: Some(id),
                subject: "Quote for the deck".into(),
                created_at: 1000,
                due_at: 1000 + DAY,
            });
        }
        all.save(&e.paths.data_dir).unwrap();
        let rows = e.list(1000);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.status == Status::Sending));

        // The op reads the Message-ID before sending; the other is undone.
        e.w.write(|tx| {
            tx.set_op_payload(
                OpId(kept),
                r#"{"kind":"send_draft","message":9,"internet_message_id":"<s1>"}"#,
            )?;
            tx.cancel_op(OpId(undone))?;
            Ok(())
        })
        .unwrap();
        let rows = e.list(1000);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, Status::Waiting);
        assert_eq!(rows[0].message, None, "Sent Items hasn't synced");
        let file = FollowUps::load(&e.paths.data_dir);
        assert_eq!(file.0[0].internet_message_id.as_deref(), Some("<s1>"));
        assert_eq!(file.0[0].send_op, None);

        // The sent copy syncs.
        let sent = e.message(e.sent, "<s1>", "pat@example.com", 1010);
        let rows = e.list(1000 + DAY);
        assert_eq!(rows[0].message, Some(sent));
        assert_eq!(rows[0].status, Status::Due);
    }
}
