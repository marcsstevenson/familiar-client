// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam api` (WP 1.10): the escape hatch. Calls Microsoft Graph directly with
//! an account's token, for anything Familiar doesn't model yet.
//!
//! The request goes through `famd`, which holds the tokens and the account's
//! request scheduler, so throttling, `Retry-After` and token refresh behave
//! as they do for sync. Only GET exists in Phase 1: writes come with the
//! durable op queue in Phase 2, so the command can't change a mailbox.

use crate::daemon::{Client, DaemonPaths, GraphGetParams, METHOD_GRAPH_GET};
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

/// `fam api`.
pub struct Api;

/// Input for `fam api`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiInput {
    /// A Graph path under https://graph.microsoft.com/v1.0, such as
    /// '/me/mailFolders?$top=5', or a full link Graph returned, such as an
    /// @odata.nextLink.
    #[schemars(extend("x-positional" = 0))]
    pub path: String,
    /// The account whose token to use (its name in config.toml). May be left
    /// out when famd syncs only one account.
    #[serde(default)]
    pub account: Option<String>,
    /// HTTP method. Only GET is allowed until writes arrive in Phase 2.
    #[serde(default)]
    pub method: Option<String>,
}

const GRAPH_HOST: &str = "https://graph.microsoft.com/";

/// Checks the input before anything is sent.
pub fn validate(input: &ApiInput) -> Result<(), CommandError> {
    if let Some(m) = &input.method
        && !m.eq_ignore_ascii_case("get")
    {
        return Err(CommandError::new(
            ErrorKind::PermissionDenied,
            format!(
                "fam api allows only GET for now ({m} was asked for); writes come with the op queue in Phase 2"
            ),
        ));
    }
    let p = input.path.trim();
    let lower = p.to_ascii_lowercase();
    if p.starts_with('/') {
        if p.starts_with("//") {
            return Err(CommandError::usage(format!(
                "{p:?} isn't a Graph path; start it with a single /"
            )));
        }
        Ok(())
    } else if lower.starts_with("https://") || lower.starts_with("http://") {
        if lower.starts_with(GRAPH_HOST) {
            Ok(())
        } else {
            Err(CommandError::usage(format!(
                "fam api only calls Microsoft Graph ({GRAPH_HOST}...), not {p:?}"
            )))
        }
    } else {
        Err(CommandError::usage(format!(
            "{p:?} isn't a Graph path; give one such as /me/mailFolders"
        )))
    }
}

/// Sends the GET through the daemon at `paths`. Used by the command and by tests.
pub fn api_at(paths: &DaemonPaths, input: &ApiInput) -> Result<Value, CommandError> {
    validate(input)?;
    Client::connect(&paths.socket)?.call(
        METHOD_GRAPH_GET,
        &GraphGetParams {
            account: input.account.clone(),
            path: input.path.trim().to_string(),
        },
    )
}

impl Command for Api {
    type Input = ApiInput;
    type Output = Value;
    const SPEC: CommandSpec = CommandSpec {
        path: &["api"],
        summary: "Call Microsoft Graph directly (GET only)",
        description: "The escape hatch for anything Familiar doesn't model yet: GETs a Graph path with an account's token and prints Graph's JSON response unchanged. Goes through famd, which holds the tokens and paces requests, so famd must be running. Only GET is allowed until writes arrive in Phase 2; any other --method is refused with exit code 5 (permission_denied). Example: fam api '/me/mailFolders/inbox/messages?$top=3&$select=subject' --account hotmail",
        access: Access::Read,
        errors: &[
            ErrorKind::NotFound,
            ErrorKind::PermissionDenied,
            ErrorKind::DaemonUnavailable,
            ErrorKind::Offline,
            ErrorKind::AuthRequired,
            ErrorKind::Throttled,
            ErrorKind::Server,
        ],
    };

    fn run(&self, _ctx: &Context<'_>, input: ApiInput) -> Result<Value, CommandError> {
        api_at(&DaemonPaths::resolve()?, &input)
    }

    fn render_text(&self, o: &Value) -> String {
        serde_json::to_string_pretty(o).unwrap_or_default()
    }
}
