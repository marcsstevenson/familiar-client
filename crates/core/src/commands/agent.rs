// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam agent status` and `fam audit list` (WP 2.6).
//!
//! `fam agent status` says whether this session counts as an agent and what
//! agents may do on each account. `fam audit list` reads the audit log:
//! every write from an agent or a script, what came of it, and every write
//! refused for want of a grant. Both read config and store directly.

use familiar_store::Reader;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::{self, Grants};
use crate::daemon::DaemonPaths;
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};

const LEVELS: [Access; 6] = [
    Access::Read,
    Access::Draft,
    Access::Write,
    Access::Send,
    Access::Delete,
    Access::Rules,
];

/// `fam agent status`.
pub struct AgentStatus;

/// Input for `fam agent status`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentStatusInput {}

/// What agents may do on one account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccountGrants {
    /// Account name in config.toml.
    pub account: String,
    /// Access levels granted: read, draft, write, send, delete, rules.
    pub allow: Vec<String>,
}

/// Output of `fam agent status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentStatusOutput {
    /// True when this session's writes are checked against the grants.
    pub agent: bool,
    /// The source its writes are logged under (`cli` for a person).
    pub source: String,
    /// Per account.
    pub accounts: Vec<AccountGrants>,
    /// Why config.toml couldn't be read, if it couldn't; agents then get
    /// the defaults (read and draft) until it's fixed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_error: Option<String>,
}

/// Status for `source` with `grants` over `accounts`.
pub fn status(source: &str, grants: &Grants, accounts: &[String]) -> AgentStatusOutput {
    AgentStatusOutput {
        agent: agent::is_agent(source),
        source: if source.is_empty() {
            agent::SOURCE_CLI.into()
        } else {
            source.into()
        },
        accounts: accounts
            .iter()
            .map(|a| AccountGrants {
                account: a.clone(),
                allow: LEVELS
                    .iter()
                    .filter(|l| grants.allows(a, **l))
                    .map(|l| agent::access_name(*l))
                    .collect(),
            })
            .collect(),
        config_error: None,
    }
}

impl Command for AgentStatus {
    type Input = AgentStatusInput;
    type Output = AgentStatusOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["agent", "status"],
        summary: "Show what agents may do",
        description: "Says whether this session counts as an agent (FAMILIAR_AGENT is set, or it runs inside Claude Code) and lists, per account, the access agents have from [agent] in config.toml. A write that needs more fails with permission_denied (exit 5).",
        access: Access::Read,
        errors: &[ErrorKind::Config],
    };

    fn run(
        &self,
        _ctx: &Context<'_>,
        _i: AgentStatusInput,
    ) -> Result<AgentStatusOutput, CommandError> {
        // A broken config shouldn't hide what agents may do: the defaults.
        let (config, error) = match crate::commands::auth::config() {
            Ok(c) => (c, None),
            Err(e) => (None, Some(e.message)),
        };
        let (grants, accounts) = match &config {
            Some(c) => (
                Grants::from_config(&c.agent),
                c.accounts.keys().cloned().collect(),
            ),
            None => (Grants::default(), Vec::new()),
        };
        let mut out = status(&agent::session_source(), &grants, &accounts);
        out.config_error = error;
        Ok(out)
    }

    fn render_text(&self, o: &AgentStatusOutput) -> String {
        let mut s = if o.agent {
            format!("This is an agent session ({}).", o.source)
        } else {
            "This is not an agent session; writes are not limited.".to_string()
        };
        if let Some(e) = &o.config_error {
            s.push_str(&format!(
                "\nconfig.toml has a problem, so agents get read and draft only: {e}"
            ));
        } else if o.accounts.is_empty() {
            s.push_str("\nNo accounts in config.toml.");
        }
        for a in &o.accounts {
            s.push_str(&format!("\n{:<12} {}", a.account, a.allow.join(", ")));
        }
        s
    }
}

/// `fam audit list`.
pub struct AuditList;

/// Input for `fam audit list`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditListInput {
    /// Only this far back: a period like 7d, 4w or 6m. Default all.
    #[serde(default)]
    pub since: Option<String>,
    /// Only this source, e.g. mcp or agent:claude-code; a trailing * matches
    /// the start (agent:*).
    #[serde(default)]
    pub source: Option<String>,
    /// Only this outcome: queued, denied, done or failed.
    #[serde(default)]
    pub outcome: Option<String>,
    /// Most rows (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// One audit row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuditView {
    /// Row ID.
    pub id: i64,
    /// When (RFC 3339, UTC).
    pub at: String,
    /// Account name, if any.
    pub account: Option<String>,
    /// Who asked: cli, gui, mcp or agent:<name>.
    pub source: String,
    /// The command, e.g. mail.move.
    pub command: String,
    /// queued, denied, done or failed.
    pub outcome: String,
    /// The op it queued, if any.
    pub op: Option<i64>,
    /// The summary, or why it was refused or failed.
    pub detail: String,
}

/// Output of `fam audit list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuditListOutput {
    /// Rows, newest first.
    pub entries: Vec<AuditView>,
}

const OUTCOMES: [&str; 4] = ["queued", "denied", "done", "failed"];

/// Reads the audit log in the store at `paths`.
pub fn audit_list_at(
    paths: &DaemonPaths,
    i: &AuditListInput,
) -> Result<AuditListOutput, CommandError> {
    let since = match &i.since {
        None => 0,
        Some(p) => {
            let days = familiar_config::validate::parse_period(p).ok_or_else(|| {
                CommandError::usage(format!(
                    "--since takes a period like 7d, 4w or 6m, not `{p}`"
                ))
            })?;
            now() - i64::from(days) * 86_400
        }
    };
    if let Some(o) = &i.outcome {
        if !OUTCOMES.contains(&o.as_str()) {
            return Err(CommandError::usage(format!(
                "--outcome is one of {}",
                OUTCOMES.join(", ")
            )));
        }
    }
    let limit = i.limit.unwrap_or(50).clamp(1, 10_000) as usize;
    let r: Reader = crate::commands::mail::open_store(paths)?;
    let accounts = r
        .accounts()
        .map_err(|e| CommandError::internal(format!("store: {e}")))?;
    let rows = r
        .audit_filtered(since, i.source.as_deref(), i.outcome.as_deref(), limit)
        .map_err(|e| CommandError::internal(format!("store: {e}")))?;
    Ok(AuditListOutput {
        entries: rows
            .into_iter()
            .map(|a| AuditView {
                id: a.id,
                at: crate::commands::auth::rfc3339(a.at.max(0) as u64),
                account: a
                    .account
                    .and_then(|id| accounts.iter().find(|x| x.id == id))
                    .map(|x| x.key.clone()),
                source: a.source,
                command: a.command,
                outcome: a.outcome,
                op: a.op.map(|o| o.0),
                detail: a.detail,
            })
            .collect(),
    })
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Command for AuditList {
    type Input = AuditListInput;
    type Output = AuditListOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["audit", "list"],
        summary: "List the audit log of writes",
        description: "Lists writes with their source (cli, gui, mcp or agent:<name>) and outcome: queued, done, failed, or denied when an agent lacked the grant. Newest first.",
        access: Access::Read,
        errors: &[ErrorKind::NotFound],
    };

    fn run(&self, _ctx: &Context<'_>, i: AuditListInput) -> Result<AuditListOutput, CommandError> {
        audit_list_at(&DaemonPaths::resolve()?, &i)
    }

    fn render_text(&self, o: &AuditListOutput) -> String {
        if o.entries.is_empty() {
            return "Nothing in the audit log.".into();
        }
        o.entries
            .iter()
            .map(|e| {
                format!(
                    "{}  {:<7} {:<18} {:<10} {:<13} {}",
                    e.at,
                    e.outcome,
                    e.source,
                    e.account.as_deref().unwrap_or("-"),
                    e.command,
                    e.detail
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lists_levels_per_account() {
        let mut g = Grants::default();
        g.accounts
            .insert("hotmail".into(), [Access::Send].into_iter().collect());
        let s = status("agent:x", &g, &["hotmail".into(), "parklands".into()]);
        assert!(s.agent);
        assert_eq!(s.accounts[0].allow, ["read", "draft", "send"]);
        assert_eq!(s.accounts[1].allow, ["read", "draft"]);
        let s = status("", &g, &[]);
        assert!(!s.agent);
        assert_eq!(s.source, "cli");
    }
}
