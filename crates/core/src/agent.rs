// SPDX-License-Identifier: MIT OR Apache-2.0
//! Agent permissions (WP 2.6).
//!
//! A write from an agent session (`fam` run by an agent, or MCP) is checked
//! against `[agent]` in `config.toml` before anything changes: every op it
//! plans needs its access level granted on that op's account. Read and
//! draft are the baseline; `write` can be granted everywhere; `send`,
//! `delete` and `rules` only per account. A refused write changes nothing,
//! fails with `permission_denied` (exit 5) and is written to the audit log
//! as `denied`.
//!
//! Writes from people (`cli`, `gui`) aren't checked. The limits stop
//! mistakes and injected instructions, not a determined local user: an
//! agent with a shell can edit the config.

use std::collections::{BTreeMap, BTreeSet};

use familiar_store::{AccountId, EventId, FolderId, WriteTx};

use crate::error::{CommandError, ErrorKind};
use crate::ops::{OpPayload, Planned};
use crate::registry::Access;

/// The source of `fam` run by a person.
pub const SOURCE_CLI: &str = "cli";
/// The source of the MCP server.
pub const SOURCE_MCP: &str = "mcp";
/// Agent sources start with this, then the agent's name.
pub const AGENT_PREFIX: &str = "agent:";

/// The source this `fam` process writes as: `agent:<name>` when
/// `FAMILIAR_AGENT` names an agent, `agent:claude-code` inside Claude Code
/// (which sets `CLAUDECODE=1`), otherwise empty (a person, `cli`).
pub fn session_source() -> String {
    if let Some(s) = SOURCE.with(|s| s.borrow().clone()) {
        return s;
    }
    source_from(
        std::env::var("FAMILIAR_AGENT").ok().as_deref(),
        std::env::var("CLAUDECODE").ok().as_deref(),
    )
}

thread_local! {
    static SOURCE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` with [`session_source`] giving `source` on this thread (the MCP
/// server runs each tool call this way, as `mcp`).
pub fn with_source<T>(source: &str, f: impl FnOnce() -> T) -> T {
    let before = SOURCE.with(|s| s.replace(Some(source.to_string())));
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let v = self.0.take();
            SOURCE.with(|s| *s.borrow_mut() = v);
        }
    }
    let _restore = Restore(before);
    f()
}

fn source_from(familiar_agent: Option<&str>, claudecode: Option<&str>) -> String {
    match familiar_agent.map(str::trim) {
        Some("" | "0") | None => {}
        Some("1" | "true" | "yes") => return format!("{AGENT_PREFIX}agent"),
        Some(name) => return format!("{AGENT_PREFIX}{name}"),
    }
    if claudecode.map(str::trim) == Some("1") {
        return format!("{AGENT_PREFIX}claude-code");
    }
    String::new()
}

/// True when writes from `source` are checked against the grants.
pub fn is_agent(source: &str) -> bool {
    source == SOURCE_MCP || source.starts_with(AGENT_PREFIX)
}

/// What agent sessions may do, from `[agent]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grants {
    /// On every account.
    pub all: BTreeSet<Access>,
    /// Added on one account, by account key.
    pub accounts: BTreeMap<String, BTreeSet<Access>>,
}

impl Default for Grants {
    /// Read and draft on every account.
    fn default() -> Self {
        Self {
            all: [Access::Read, Access::Draft].into(),
            accounts: BTreeMap::new(),
        }
    }
}

/// Parses an access level as `config.toml` writes it.
pub fn parse_access(s: &str) -> Option<Access> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

/// The name `config.toml` uses for `a`.
pub fn access_name(a: Access) -> String {
    serde_json::to_value(a)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

impl Grants {
    /// From `[agent]`. Unknown names are left out (validation reports them),
    /// and so are per-account levels given for every account.
    pub fn from_config(c: &familiar_config::model::Agent) -> Self {
        let per_account = |a: &Access| {
            familiar_config::model::PER_ACCOUNT_ACCESS.contains(&access_name(*a).as_str())
        };
        Self {
            all: c
                .allow
                .iter()
                .filter_map(|s| parse_access(s))
                .filter(|a| !per_account(a))
                .collect(),
            accounts: c
                .accounts
                .iter()
                .map(|(k, g)| {
                    (
                        k.clone(),
                        g.allow.iter().filter_map(|s| parse_access(s)).collect(),
                    )
                })
                .collect(),
        }
    }

    /// From the config file in the usual place; the defaults when there is
    /// none. A config that doesn't load also gives the defaults.
    pub fn load() -> Self {
        let Some((path, _)) = familiar_config::paths::resolve(None) else {
            return Self::default();
        };
        Self::load_from(&path)
    }

    /// From the config file at `path`; the defaults when it doesn't load.
    pub fn load_from(path: &std::path::Path) -> Self {
        match familiar_config::load(path) {
            Ok(c) => Self::from_config(&c.agent),
            Err(_) => Self::default(),
        }
    }

    /// True when agents may do `access` on `account`.
    pub fn allows(&self, account: &str, access: Access) -> bool {
        access == Access::Read
            || self.all.contains(&access)
            || self
                .accounts
                .get(account)
                .is_some_and(|s| s.contains(&access))
    }
}

/// A refused write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    /// The account it was on.
    pub account: AccountId,
    /// Its key.
    pub account_key: String,
    /// What was missing.
    pub access: Access,
    /// The op, in words.
    pub summary: String,
}

impl Denial {
    /// The error the caller gets.
    pub fn error(&self) -> CommandError {
        let a = access_name(self.access);
        let fix = if familiar_config::model::PER_ACCOUNT_ACCESS.contains(&a.as_str()) {
            format!("[agent.accounts.{}] allow = [\"{a}\"]", self.account_key)
        } else {
            format!("[agent] allow = [..., \"{a}\"]")
        };
        CommandError::new(
            ErrorKind::PermissionDenied,
            format!(
                "agents aren't allowed to {a} on {} ({}); a person can run it, or grant it in config.toml with {fix}",
                self.account_key, self.summary
            ),
        )
    }
}

/// The access levels `p` needs, highest first.
pub fn required(tx: &WriteTx<'_>, p: &OpPayload) -> Vec<Access> {
    let folder_is = |id: i64, which: &str| {
        tx.folder(FolderId(id))
            .ok()
            .flatten()
            .and_then(|f| f.well_known)
            .is_some_and(|w| w == which)
    };
    // A change to a meeting you organised goes out to its attendees.
    let meeting = |id: i64| {
        tx.event(EventId(id))
            .ok()
            .flatten()
            .is_some_and(|e| e.data.is_organizer && !e.data.attendees.is_empty())
    };
    let mut need = match p {
        OpPayload::SetRead { .. }
        | OpPayload::SetFlag { .. }
        | OpPayload::CreateFolder { .. }
        | OpPayload::RenameFolder { .. }
        | OpPayload::MoveFolder { .. } => vec![Access::Write],
        // Into Deleted Items is still a delete to the person.
        OpPayload::MoveMessage { to, .. } if folder_is(*to, "deleteditems") => {
            vec![Access::Delete]
        }
        OpPayload::MoveMessage { .. } | OpPayload::ReportMessage { .. } => vec![Access::Write],
        OpPayload::DeleteMessage { .. } | OpPayload::DeleteFolder { .. } => vec![Access::Delete],
        OpPayload::CreateDraft(_) | OpPayload::UpdateDraft(_) => vec![Access::Draft],
        OpPayload::SendDraft { .. } => vec![Access::Send],
        OpPayload::CreateEvent(e) => {
            let mut v = vec![Access::Write];
            if !e.fields.attendees.is_empty() || !e.fields.optional.is_empty() {
                v.push(Access::Send);
            }
            v
        }
        OpPayload::UpdateEvent(e) => {
            let mut v = vec![Access::Write];
            if meeting(e.event) || e.changes.attendees.as_ref().is_some_and(|a| !a.is_empty()) {
                v.push(Access::Send);
            }
            v
        }
        OpPayload::RespondEvent(e) => {
            let mut v = vec![Access::Write];
            if e.send_response {
                v.push(Access::Send);
            }
            v
        }
        OpPayload::DeleteEvent(e) => {
            let mut v = vec![Access::Delete];
            if meeting(e.event) {
                v.push(Access::Send);
            }
            v
        }
    };
    need.sort();
    need.dedup();
    need.reverse();
    need
}

/// The first op of `planned` that `grants` doesn't allow.
pub fn check(tx: &WriteTx<'_>, grants: &Grants, planned: &[Planned]) -> Option<Denial> {
    for p in planned {
        for a in required(tx, &p.payload) {
            if !grants.allows(&p.account_key, a) {
                return Some(Denial {
                    account: p.account,
                    account_key: p.account_key.clone(),
                    access: a,
                    summary: p.summary.clone(),
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources() {
        assert_eq!(source_from(None, None), "");
        assert_eq!(source_from(Some(""), Some("0")), "");
        assert_eq!(source_from(None, Some("1")), "agent:claude-code");
        assert_eq!(source_from(Some("1"), None), "agent:agent");
        assert_eq!(source_from(Some("triage"), Some("1")), "agent:triage");
        assert!(is_agent("agent:x") && is_agent("mcp"));
        assert!(!is_agent("cli") && !is_agent("gui") && !is_agent(""));
        assert_eq!(with_source("mcp", session_source), "mcp");
    }

    #[test]
    fn grants_from_config() {
        let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let c = familiar_config::model::Agent {
            allow: strs(&["read", "draft", "write", "send"]),
            accounts: [(
                "hotmail".to_string(),
                familiar_config::model::AgentGrant {
                    allow: strs(&["send", "delete"]),
                },
            )]
            .into(),
        };
        let g = Grants::from_config(&c);
        assert!(g.allows("parklands", Access::Write));
        // `send` for every account is ignored: it is per account.
        assert!(!g.allows("parklands", Access::Send));
        assert!(g.allows("hotmail", Access::Send));
        assert!(g.allows("hotmail", Access::Delete));
        assert!(!g.allows("hotmail", Access::Rules));
        let d = Grants::default();
        assert!(d.allows("x", Access::Read) && d.allows("x", Access::Draft));
        assert!(!d.allows("x", Access::Write));
        assert_eq!(parse_access("rules"), Some(Access::Rules));
        assert_eq!(access_name(Access::Send), "send");
    }
}
