// SPDX-License-Identifier: MIT OR Apache-2.0
//! The typed shape of `config.toml`. Unknown keys are errors, so a typo is
//! reported instead of silently ignored.

use serde::Deserialize;
use std::collections::BTreeMap;

/// The config file format version this build reads.
pub const CURRENT_VERSION: u32 = 1;

/// The whole of `config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// File format version. Must be 1.
    pub version: u32,
    /// App-wide settings.
    #[serde(default)]
    pub app: App,
    /// Mail accounts by short name (the name used with `--account`).
    #[serde(default)]
    pub accounts: BTreeMap<String, Account>,
    /// Calendar settings.
    #[serde(default)]
    pub calendar: Calendar,
    /// What is kept on this device.
    #[serde(default)]
    pub sync: Sync,
    /// Saved searches shown as smart folders.
    #[serde(default)]
    pub views: Vec<View>,
    /// Mail rules, in the order they run.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Single-key shortcuts: action name to key.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    /// What agent sessions (CLI or MCP) may do.
    #[serde(default)]
    pub agent: Agent,
}

/// `[app]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    /// The Microsoft app (client) ID. Public, not a secret. Defaults to Familiar's own.
    pub client_id: Option<String>,
    /// Colour theme.
    #[serde(default)]
    pub theme: Theme,
    /// How long a sent message waits in the outbox so it can be undone.
    #[serde(default = "default_send_delay")]
    pub send_delay_seconds: u32,
}

impl Default for App {
    fn default() -> Self {
        Self {
            client_id: None,
            theme: Theme::default(),
            send_delay_seconds: default_send_delay(),
        }
    }
}

fn default_send_delay() -> u32 {
    10
}

/// The longest undo-send delay accepted.
pub const MAX_SEND_DELAY_SECONDS: u32 = 120;

/// Colour theme.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follow the operating system.
    #[default]
    System,
    /// Light.
    Light,
    /// Dark.
    Dark,
}

/// `[accounts.<name>]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    /// The mailbox address.
    pub email: String,
    /// Where to sign in: `consumers` for Outlook.com and Hotmail, otherwise
    /// the organisation's tenant ID or domain.
    pub tenant: String,
    /// The name shown on sent mail. Defaults to the name on the account.
    pub display_name: Option<String>,
    /// Other addresses this mailbox can send from (Outlook.com aliases, or
    /// addresses the organisation lets it send as), offered as From choices.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// The folder "processed" messages go to (`fam mail process`, the tick in
    /// the desktop app). Off when unset; created at the top level when missing.
    pub processed_folder: Option<String>,
    /// Signatures for this account.
    pub signature: Option<Signature>,
}

impl Account {
    /// Whether `address` is this account's own address or one of its aliases.
    pub fn sends_as(&self, address: &str) -> bool {
        self.email.eq_ignore_ascii_case(address)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(address))
    }
}

/// `[accounts.<name>.signature]`, as HTML.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signature {
    /// Added to new messages.
    pub new: Option<String>,
    /// Added to replies and forwards.
    pub reply: Option<String>,
}

/// `[calendar]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calendar {
    /// How far back events are kept on this device, for example `3m`.
    #[serde(default = "default_calendar_past")]
    pub past: String,
    /// How far ahead events are kept on this device, for example `1y`.
    #[serde(default = "default_calendar_future")]
    pub future: String,
    /// Named purposes (`--purpose family`) mapped to a calendar on an account.
    #[serde(default)]
    pub purposes: BTreeMap<String, Purpose>,
}

impl Default for Calendar {
    fn default() -> Self {
        Self {
            past: default_calendar_past(),
            future: default_calendar_future(),
            purposes: BTreeMap::new(),
        }
    }
}

fn default_calendar_past() -> String {
    "3m".into()
}
fn default_calendar_future() -> String {
    "1y".into()
}

/// One entry in `[calendar.purposes]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Purpose {
    /// Account name from `[accounts]`.
    pub account: String,
    /// The calendar's display name on that account.
    pub calendar: String,
}

/// `[sync]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sync {
    /// How far back full message bodies are kept, for example `1y`, `6m`, `90d`.
    #[serde(default = "default_window")]
    pub window: String,
    /// How far back headers are kept: `all` or a period like `window`.
    #[serde(default = "default_headers")]
    pub headers: String,
    /// What happens to bodies older than the window.
    #[serde(default)]
    pub older_bodies: OlderBodies,
    /// The most disk the body cache may use, for example `2GB` or `500MB`.
    #[serde(default = "default_cache_limit")]
    pub cache_limit: String,
    /// When attachments are downloaded.
    #[serde(default)]
    pub attachments: Attachments,
}

impl Default for Sync {
    fn default() -> Self {
        Self {
            window: default_window(),
            headers: default_headers(),
            older_bodies: OlderBodies::default(),
            cache_limit: default_cache_limit(),
            attachments: Attachments::default(),
        }
    }
}

fn default_window() -> String {
    "1y".into()
}
fn default_headers() -> String {
    "all".into()
}
fn default_cache_limit() -> String {
    "2GB".into()
}

/// `sync.older_bodies`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OlderBodies {
    /// Fetched when opened, and searched on the server.
    #[default]
    OnDemand,
    /// Never fetched; headers only.
    None,
}

/// `sync.attachments`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Attachments {
    /// Downloaded when opened or saved.
    #[default]
    OnDemand,
    /// Downloaded for every message inside the sync window.
    Window,
}

/// One `[[views]]` entry.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    /// The name shown in the folder list.
    pub name: String,
    /// The search query, in `fam mail search` syntax.
    pub query: String,
}

/// One `[[rules]]` entry. Conditions and actions are checked by the rules engine (WP 4.x).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Unique per account.
    pub name: String,
    /// Account name from `[accounts]`.
    pub account: String,
    /// Where the rule runs.
    #[serde(default)]
    pub run: RuleRun,
    /// Off rules are kept but not applied.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Conditions.
    pub when: toml::Table,
    /// Actions.
    pub then: toml::Table,
}

fn default_true() -> bool {
    true
}

/// `rules.run`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleRun {
    /// A server rule on the Inbox; runs while Familiar is closed.
    #[default]
    Server,
    /// Runs in Familiar, only while the app or daemon is running.
    Local,
}

/// `[agent]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// Access levels agent sessions get on every account. `send`, `delete`
    /// and `rules` can't go here: they are granted per account.
    #[serde(default = "default_agent_allow")]
    pub allow: Vec<String>,
    /// More access on one account, keyed by account name
    /// (`[agent.accounts.hotmail] allow = ["send"]`).
    #[serde(default)]
    pub accounts: BTreeMap<String, AgentGrant>,
}

impl Default for Agent {
    fn default() -> Self {
        Self {
            allow: default_agent_allow(),
            accounts: BTreeMap::new(),
        }
    }
}

/// `[agent.accounts.<name>]`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentGrant {
    /// Access levels added on this account.
    #[serde(default)]
    pub allow: Vec<String>,
}

fn default_agent_allow() -> Vec<String> {
    vec!["read".into(), "draft".into()]
}

/// The values `agent.allow` accepts. These match `familiar_core::Access`
/// (a test in familiar-core keeps the two in step).
pub const AGENT_ACCESS: &[&str] = &["read", "draft", "write", "send", "delete", "rules"];

/// The access levels that need a grant per account: sending (and so
/// forwarding), deleting and rule changes.
pub const PER_ACCOUNT_ACCESS: &[&str] = &["send", "delete", "rules"];
