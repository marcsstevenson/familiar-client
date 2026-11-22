// SPDX-License-Identifier: MIT OR Apache-2.0
//! First run (onboarding WP OB.3): which screen the main window opens on.
//!
//! The Welcome screen adds the first account with `account.add`; Welcome back signs in
//! the accounts a restored config already names with `auth.login`. Both then restart the
//! app with [`crate::config_watch::restart_app`], which also restarts famd.

use familiar_core::{CommandError, Registry};
use serde::Serialize;

/// Where this install stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// No config.toml yet.
    NoConfig,
    /// config.toml has errors; see `errors`.
    Invalid,
    /// config.toml names no accounts.
    NoAccounts,
    /// Accounts are configured, but none is signed in on this computer.
    SignedOut,
    /// At least one account is signed in.
    Ready,
}

/// One configured account, for Welcome back.
#[derive(Debug, Clone, Serialize)]
pub struct OnboardingAccount {
    /// Its name in config.toml.
    pub name: String,
    /// Its address.
    pub email: String,
    /// `microsoft` or `gmail`.
    pub provider: &'static str,
    /// True when tokens for it are stored here.
    pub signed_in: bool,
}

/// The answer to `onboarding_state`.
#[derive(Debug, Clone, Serialize)]
pub struct OnboardingState {
    /// Where this install stands.
    pub stage: Stage,
    /// The config file Familiar uses, existing or not.
    pub config_path: String,
    /// Each problem in an invalid config, with its line and column.
    pub errors: Vec<String>,
    /// The configured accounts.
    pub accounts: Vec<OnboardingAccount>,
}

/// Works out which first-run screen, if any, the main window shows.
#[tauri::command]
pub async fn onboarding_state(
    registry: tauri::State<'_, std::sync::Arc<Registry>>,
) -> Result<OnboardingState, CommandError> {
    let registry = std::sync::Arc::clone(&registry);
    tauri::async_runtime::spawn_blocking(move || state(&registry))
        .await
        .map_err(|e| CommandError::internal(format!("onboarding task failed: {e}")))?
}

fn state(registry: &Registry) -> Result<OnboardingState, CommandError> {
    let (path, _) = familiar_config::paths::resolve(None)
        .ok_or_else(|| CommandError::internal("no home folder to look for config.toml in"))?;
    let mut out = OnboardingState {
        stage: Stage::NoConfig,
        config_path: path.display().to_string(),
        errors: Vec::new(),
        accounts: Vec::new(),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => {
            out.stage = Stage::Invalid;
            out.errors.push(format!("couldn't read it: {e}"));
            return Ok(out);
        }
    };
    let report = familiar_config::check(&text);
    out.errors = report.errors().map(|p| p.to_string()).collect();
    let config = match (report.config, out.errors.is_empty()) {
        (Some(c), true) => c,
        _ => {
            out.stage = Stage::Invalid;
            return Ok(out);
        }
    };
    if config.accounts.is_empty() {
        out.stage = Stage::NoAccounts;
        return Ok(out);
    }
    // A token store that can't be opened counts as nobody signed in; the sign-in
    // itself then shows the reason.
    let signed: Vec<String> = registry
        .run("auth.status", serde_json::json!({}))
        .ok()
        .and_then(|o| o.json["accounts"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter(|a| a["state"] != "signed_out")
        .filter_map(|a| a["account"].as_str().map(str::to_string))
        .collect();
    for (name, a) in &config.accounts {
        out.accounts.push(OnboardingAccount {
            name: name.clone(),
            email: a.email.clone(),
            provider: match a.provider {
                familiar_config::Provider::Graph => "microsoft",
                familiar_config::Provider::Gmail => "gmail",
            },
            signed_in: signed.contains(name),
        });
    }
    out.stage = if out.accounts.iter().any(|a| a.signed_in) {
        Stage::Ready
    } else {
        Stage::SignedOut
    };
    Ok(out)
}
