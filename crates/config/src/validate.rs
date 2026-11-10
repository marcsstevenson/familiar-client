// SPDX-License-Identifier: MIT OR Apache-2.0
//! Checks that go beyond the shape: references between sections, formats and
//! duplicates. Every problem found is reported, not just the first.

use crate::model::{
    AGENT_ACCESS, CURRENT_VERSION, Config, MAX_SEND_DELAY_SECONDS, PER_ACCOUNT_ACCESS,
};
use std::collections::{BTreeMap, BTreeSet};

/// How serious a problem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The config can't be used until this is fixed.
    Error,
    /// Works, but probably not as intended.
    Warning,
}

/// One problem in a config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Error or warning.
    pub severity: Severity,
    /// Dotted key path, for example `accounts.hotmail.email`. Empty for the whole file.
    pub path: String,
    /// 1-based line, when known.
    pub line: Option<usize>,
    /// 1-based column, when known.
    pub column: Option<usize>,
    /// What is wrong and how to fix it.
    pub message: String,
}

impl Problem {
    pub(crate) fn error(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            path: path.into(),
            line: None,
            column: None,
            message: message.into(),
        }
    }

    pub(crate) fn warning(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(path, message)
        }
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, self.column) {
            (Some(l), Some(c)) => write!(f, "{l}:{c}: ")?,
            (Some(l), None) => write!(f, "{l}: ")?,
            _ => {}
        }
        if self.severity == Severity::Warning {
            f.write_str("warning: ")?;
        }
        if !self.path.is_empty() {
            write!(f, "{}: ", self.path)?;
        }
        f.write_str(&self.message)
    }
}

/// Runs every semantic check on a config that already has the right shape.
pub fn check(c: &Config) -> Vec<Problem> {
    let mut p = Vec::new();

    if c.version != CURRENT_VERSION {
        p.push(Problem::error(
            "version",
            format!(
                "unsupported version {}; this build reads version {CURRENT_VERSION}",
                c.version
            ),
        ));
    }

    if let Some(id) = &c.app.client_id
        && !is_guid(id)
    {
        p.push(Problem::error(
            "app.client_id",
            "must be a GUID like 00000000-0000-0000-0000-000000000000",
        ));
    }
    if c.app.send_delay_seconds > MAX_SEND_DELAY_SECONDS {
        p.push(Problem::error(
            "app.send_delay_seconds",
            format!("must be at most {MAX_SEND_DELAY_SECONDS}"),
        ));
    }

    if c.accounts.is_empty() {
        p.push(Problem::warning(
            "accounts",
            "no accounts configured; add an [accounts.<name>] table",
        ));
    }
    let mut emails: BTreeMap<String, &str> = BTreeMap::new();
    for (name, a) in &c.accounts {
        let base = format!("accounts.{name}");
        if !is_account_name(name) {
            p.push(Problem::error(
                &base,
                "account names may use only lowercase letters, digits, `-` and `_`, and must start with a letter",
            ));
        }
        if !is_email(&a.email) {
            p.push(Problem::error(
                format!("{base}.email"),
                format!("`{}` is not an email address", a.email),
            ));
        } else if let Some(other) = emails.insert(a.email.to_ascii_lowercase(), name) {
            p.push(Problem::error(
                format!("{base}.email"),
                format!("the same address is also used by account `{other}`"),
            ));
        }
        match a.tenant.as_str() {
            "consumers" => {}
            "organizations" | "common" => p.push(Problem::warning(
                format!("{base}.tenant"),
                format!(
                    "`{}` works for sign-in, but token refresh must use the account's own tenant; set the tenant ID or domain",
                    a.tenant
                ),
            )),
            t if is_guid(t) || is_domain(t) => {}
            _ => p.push(Problem::error(
                format!("{base}.tenant"),
                "must be `consumers` (Outlook.com, Hotmail), a tenant ID, or a domain",
            )),
        }
        if let Some(d) = &a.display_name
            && d.trim().is_empty()
        {
            p.push(Problem::error(
                format!("{base}.display_name"),
                "must not be empty; remove it to use the account's own name",
            ));
        }
        for (n, alias) in a.aliases.iter().enumerate() {
            if !is_email(alias) {
                p.push(Problem::error(
                    format!("{base}.aliases[{n}]"),
                    "must be an email address",
                ));
            } else if alias.eq_ignore_ascii_case(&a.email) {
                p.push(Problem::warning(
                    format!("{base}.aliases[{n}]"),
                    "is the account's own address; it needn't be listed",
                ));
            }
        }
        if let Some(f) = &a.processed_folder
            && (f.trim().is_empty() || f.contains('/'))
        {
            p.push(Problem::error(
                format!("{base}.processed_folder"),
                "must be a folder name, not empty and without '/'",
            ));
        }
    }

    for (key, value) in [("past", &c.calendar.past), ("future", &c.calendar.future)] {
        if parse_period(value).is_none() {
            p.push(Problem::error(
                format!("calendar.{key}"),
                "must be a period like `1y`, `6m`, `4w` or `90d`",
            ));
        }
    }

    for (purpose, target) in &c.calendar.purposes {
        let path = format!("calendar.purposes.{purpose}");
        if !c.accounts.contains_key(&target.account) {
            p.push(unknown_account(format!("{path}.account"), &target.account));
        }
        if target.calendar.trim().is_empty() {
            p.push(Problem::error(
                format!("{path}.calendar"),
                "must name a calendar",
            ));
        }
    }

    if parse_period(&c.sync.window).is_none() {
        p.push(Problem::error(
            "sync.window",
            "must be a period like `1y`, `6m`, `4w` or `90d`",
        ));
    }
    if c.sync.headers != "all" && parse_period(&c.sync.headers).is_none() {
        p.push(Problem::error(
            "sync.headers",
            "must be `all` or a period like `5y`",
        ));
    } else if let (Some(h), Some(w)) = (parse_period(&c.sync.headers), parse_period(&c.sync.window))
        && h < w
    {
        p.push(Problem::error(
            "sync.headers",
            "must cover at least the body window (`sync.window`)",
        ));
    }
    if parse_size(&c.sync.cache_limit).is_none() {
        p.push(Problem::error(
            "sync.cache_limit",
            "must be a size like `500MB` or `2GB`",
        ));
    }

    let mut view_names = BTreeSet::new();
    for (i, v) in c.views.iter().enumerate() {
        let path = format!("views[{i}]");
        if v.name.trim().is_empty() {
            p.push(Problem::error(format!("{path}.name"), "must not be empty"));
        } else if !view_names.insert(v.name.as_str()) {
            p.push(Problem::error(
                format!("{path}.name"),
                format!("another view is already called `{}`", v.name),
            ));
        }
        if v.query.trim().is_empty() {
            p.push(Problem::error(format!("{path}.query"), "must not be empty"));
        }
    }

    for (i, h) in c.hooks.iter().enumerate() {
        let path = format!("hooks[{i}]");
        if !crate::model::HOOK_EVENTS.contains(&h.on.as_str()) {
            p.push(Problem::error(
                format!("{path}.on"),
                format!(
                    "unknown event `{}`; expected one of: {}",
                    h.on,
                    crate::model::HOOK_EVENTS.join(", ")
                ),
            ));
        }
        if h.run.trim().is_empty() {
            p.push(Problem::error(format!("{path}.run"), "must not be empty"));
        }
        if h.matches.as_deref().is_some_and(|m| m.trim().is_empty()) {
            p.push(Problem::error(
                format!("{path}.match"),
                "must not be empty; leave it out to match every message",
            ));
        }
        if h.timeout_seconds == 0 {
            p.push(Problem::error(
                format!("{path}.timeout_seconds"),
                "must be at least 1",
            ));
        }
    }

    let mut rule_names = BTreeSet::new();
    for (i, r) in c.rules.iter().enumerate() {
        let path = format!("rules[{i}]");
        if !c.accounts.contains_key(&r.account) {
            p.push(unknown_account(format!("{path}.account"), &r.account));
        }
        if r.name.trim().is_empty() {
            p.push(Problem::error(format!("{path}.name"), "must not be empty"));
        } else if !rule_names.insert((r.account.as_str(), r.name.as_str())) {
            p.push(Problem::error(
                format!("{path}.name"),
                format!(
                    "account `{}` already has a rule called `{}`",
                    r.account, r.name
                ),
            ));
        }
        if r.when.is_empty() {
            p.push(Problem::error(
                format!("{path}.when"),
                "needs at least one condition",
            ));
        }
        if r.then.is_empty() {
            p.push(Problem::error(
                format!("{path}.then"),
                "needs at least one action",
            ));
        }
    }

    let mut keys: BTreeMap<&str, &str> = BTreeMap::new();
    for (action, key) in &c.keys {
        let path = format!("keys.{action}");
        if key.is_empty() {
            p.push(Problem::error(path, "must not be empty"));
        } else if let Some(other) = keys.insert(key.as_str(), action.as_str()) {
            p.push(Problem::error(
                path,
                format!("`{key}` is already bound to `{other}`"),
            ));
        }
    }

    for (i, a) in c.agent.allow.iter().enumerate() {
        if !AGENT_ACCESS.contains(&a.as_str()) {
            p.push(Problem::error(
                format!("agent.allow[{i}]"),
                format!(
                    "unknown access `{a}`; expected one of {}",
                    AGENT_ACCESS.join(", ")
                ),
            ));
        }
    }
    for (i, a) in c.agent.allow.iter().enumerate() {
        if PER_ACCOUNT_ACCESS.contains(&a.as_str()) {
            p.push(Problem::error(
                format!("agent.allow[{i}]"),
                format!(
                    "`{a}` is granted per account: put it under [agent.accounts.<name>] instead"
                ),
            ));
        }
    }
    for (name, grant) in &c.agent.accounts {
        if !c.accounts.contains_key(name) {
            p.push(unknown_account(format!("agent.accounts.{name}"), name));
        }
        for (i, a) in grant.allow.iter().enumerate() {
            if !AGENT_ACCESS.contains(&a.as_str()) {
                p.push(Problem::error(
                    format!("agent.accounts.{name}.allow[{i}]"),
                    format!(
                        "unknown access `{a}`; expected one of {}",
                        AGENT_ACCESS.join(", ")
                    ),
                ));
            }
        }
    }
    let tool = c.ai.tool.as_str();
    let is_path = tool.contains('/') || tool.contains('\\');
    if !is_path && !crate::model::AI_TOOLS.contains(&tool) {
        p.push(Problem::error(
            "ai.tool",
            format!(
                "unknown tool `{tool}`; expected one of {}, or a path to the program",
                crate::model::AI_TOOLS.join(", ")
            ),
        ));
    }
    if c.ai.timeout_seconds == 0 {
        p.push(Problem::error("ai.timeout_seconds", "must be 1 or more"));
    }
    for (i, name) in c.ai.accounts.iter().flatten().enumerate() {
        if !c.accounts.contains_key(name) {
            p.push(unknown_account(format!("ai.accounts[{i}]"), name));
        }
    }
    for (key, a) in &c.accounts {
        if !a.spam_check && !a.spam_auto {
            continue;
        }
        let path = format!(
            "accounts.{key}.{}",
            if a.spam_auto {
                "spam_auto"
            } else {
                "spam_check"
            }
        );
        if !c.ai.enabled {
            p.push(Problem::warning(
                path,
                "needs `[ai] enabled = true`; until then only the facts are shown",
            ));
        } else if c
            .ai
            .accounts
            .as_ref()
            .is_some_and(|list| !list.iter().any(|x| x == key))
        {
            p.push(Problem::warning(
                path,
                "this account isn't in `[ai] accounts`, so its mail isn't sent; only the facts are shown",
            ));
        }
    }
    if !c.agent.allow.iter().any(|a| a == "read") {
        p.push(Problem::warning(
            "agent.allow",
            "without `read`, agents can't use Familiar at all",
        ));
    }

    p
}

fn unknown_account(path: String, name: &str) -> Problem {
    Problem::error(path, format!("no account called `{name}` in [accounts]"))
}

/// True for a GUID in the usual 8-4-4-4-12 hex form.
pub fn is_guid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn is_account_name(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn is_domain(s: &str) -> bool {
    let labels: Vec<&str> = s.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn is_email(s: &str) -> bool {
    match s.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && !local.chars().any(char::is_whitespace)
                && !domain.contains('@')
                && is_domain(domain)
        }
        None => false,
    }
}

/// Parses a period like `1y`, `6m`, `4w` or `90d` into days.
pub fn parse_period(s: &str) -> Option<u32> {
    let unit = s.chars().last()?;
    let n: u32 = s[..s.len() - unit.len_utf8()].parse().ok()?;
    if n == 0 {
        return None;
    }
    let days = match unit {
        'd' => 1,
        'w' => 7,
        'm' => 30,
        'y' => 365,
        _ => return None,
    };
    n.checked_mul(days)
}

/// Parses a size like `500MB` or `2GB` into bytes. Units are powers of 1024.
pub fn parse_size(s: &str) -> Option<u64> {
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    let n: u64 = s[..digits].parse().ok()?;
    let mult: u64 = match s[digits..].trim() {
        "KB" => 1 << 10,
        "MB" => 1 << 20,
        "GB" => 1 << 30,
        "TB" => 1 << 40,
        _ => return None,
    };
    if n == 0 { None } else { n.checked_mul(mult) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert!(is_guid("c6057b6a-a692-47dd-b34e-12629372e32a"));
        assert!(!is_guid("c6057b6a-a692-47dd-b34e-12629372e32"));
        assert!(is_email("you@example.com"));
        assert!(!is_email("you@example"));
        assert!(!is_email("you example.com"));
        assert!(is_domain("club.example.org"));
        assert_eq!(parse_period("1y"), Some(365));
        assert_eq!(parse_period("90d"), Some(90));
        assert_eq!(parse_period("0d"), None);
        assert_eq!(parse_period("1x"), None);
        assert_eq!(parse_size("2GB"), Some(2 << 30));
        assert_eq!(parse_size("500 MB"), Some(500 << 20));
        assert_eq!(parse_size("2gb"), None);
        assert!(is_account_name("parklands"));
        assert!(!is_account_name("Parklands"));
        assert!(!is_account_name("9lives"));
    }
}
