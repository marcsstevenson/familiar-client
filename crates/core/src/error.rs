// SPDX-License-Identifier: MIT OR Apache-2.0
//! Command errors and the stable exit codes they map to.
//!
//! Exit codes are part of the public contract: scripts and agents branch on
//! them. A code, once published, never changes meaning. New kinds get new
//! numbers; retired kinds keep their number reserved.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// What went wrong, as a stable category. Each kind has a fixed exit code.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// An unexpected failure inside Familiar. A bug; please report it.
    Internal,
    /// The command line or input was invalid.
    Usage,
    /// `config.toml` is missing or invalid.
    Config,
    /// The account needs to sign in (again): `fam auth login`.
    AuthRequired,
    /// The action is not granted to this caller, for example an agent without a send grant.
    PermissionDenied,
    /// The message, folder, event or account does not exist.
    NotFound,
    /// The server could not be reached and the command needs it.
    Offline,
    /// The server asked us to slow down; retry later.
    Throttled,
    /// The item changed on the server or locally since it was read.
    Conflict,
    /// The command sends, deletes or changes rules and needs `--yes` when not interactive.
    ConfirmationRequired,
    /// The daemon `famd` is not running or not reachable.
    DaemonUnavailable,
    /// The server returned an error it did not explain as one of the kinds above.
    Server,
}

impl ErrorKind {
    /// Every kind, in exit code order.
    pub const ALL: &'static [ErrorKind] = &[
        ErrorKind::Internal,
        ErrorKind::Usage,
        ErrorKind::Config,
        ErrorKind::AuthRequired,
        ErrorKind::PermissionDenied,
        ErrorKind::NotFound,
        ErrorKind::Offline,
        ErrorKind::Throttled,
        ErrorKind::Conflict,
        ErrorKind::ConfirmationRequired,
        ErrorKind::DaemonUnavailable,
        ErrorKind::Server,
    ];

    /// The process exit code for this kind. Stable: never renumber.
    pub const fn exit_code(self) -> u8 {
        match self {
            ErrorKind::Internal => 1,
            ErrorKind::Usage => 2,
            ErrorKind::Config => 3,
            ErrorKind::AuthRequired => 4,
            ErrorKind::PermissionDenied => 5,
            ErrorKind::NotFound => 6,
            ErrorKind::Offline => 7,
            ErrorKind::Throttled => 8,
            ErrorKind::Conflict => 9,
            ErrorKind::ConfirmationRequired => 10,
            ErrorKind::DaemonUnavailable => 11,
            ErrorKind::Server => 12,
        }
    }

    /// The snake_case name used in JSON output and the schema.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Internal => "internal",
            ErrorKind::Usage => "usage",
            ErrorKind::Config => "config",
            ErrorKind::AuthRequired => "auth_required",
            ErrorKind::PermissionDenied => "permission_denied",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Offline => "offline",
            ErrorKind::Throttled => "throttled",
            ErrorKind::Conflict => "conflict",
            ErrorKind::ConfirmationRequired => "confirmation_required",
            ErrorKind::DaemonUnavailable => "daemon_unavailable",
            ErrorKind::Server => "server",
        }
    }

    /// A one-line description for documentation and `fam schema`.
    pub const fn description(self) -> &'static str {
        match self {
            ErrorKind::Internal => "Unexpected failure inside Familiar (a bug).",
            ErrorKind::Usage => "Invalid command line or input.",
            ErrorKind::Config => "config.toml is missing or invalid.",
            ErrorKind::AuthRequired => "The account needs to sign in: fam auth login.",
            ErrorKind::PermissionDenied => "Not granted to this caller (agent permissions).",
            ErrorKind::NotFound => "The item or account does not exist.",
            ErrorKind::Offline => "The server is unreachable and the command needs it.",
            ErrorKind::Throttled => "The server asked to slow down; retry later.",
            ErrorKind::Conflict => "The item changed since it was read.",
            ErrorKind::ConfirmationRequired => "Needs --yes when not run interactively.",
            ErrorKind::DaemonUnavailable => "famd is not running or not reachable.",
            ErrorKind::Server => "The server returned an unexplained error.",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error returned by a command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommandError {
    /// The stable category.
    pub kind: ErrorKind,
    /// A human-readable explanation.
    pub message: String,
}

impl CommandError {
    /// Builds an error of the given kind.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Shorthand for a usage error.
    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Usage, message)
    }

    /// Shorthand for an internal error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    /// The process exit code for this error.
    pub fn exit_code(&self) -> u8 {
        self.kind.exit_code()
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for CommandError {}

/// Renders the exit code table as Markdown. `docs/exit-codes.md` is checked against this.
pub fn exit_codes_markdown() -> String {
    let mut out = String::from(
        "# fam exit codes\n\n\
         Generated from `familiar_core::ErrorKind`; do not edit by hand. Regenerate with\n\
         `FAMILIAR_BLESS=1 cargo test -p familiar-cli --test parity`.\n\n\
         These codes are stable. A code never changes meaning once published.\n\n\
         | Code | Kind | Meaning |\n| --- | --- | --- |\n| 0 | | Success. |\n",
    );
    for kind in ErrorKind::ALL {
        out.push_str(&format!(
            "| {} | `{}` | {} |\n",
            kind.exit_code(),
            kind.as_str(),
            kind.description()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published numbering. Changing this test means breaking scripts and agents.
    #[test]
    fn exit_codes_are_stable() {
        let codes: Vec<(&str, u8)> = ErrorKind::ALL
            .iter()
            .map(|k| (k.as_str(), k.exit_code()))
            .collect();
        assert_eq!(
            codes,
            vec![
                ("internal", 1),
                ("usage", 2),
                ("config", 3),
                ("auth_required", 4),
                ("permission_denied", 5),
                ("not_found", 6),
                ("offline", 7),
                ("throttled", 8),
                ("conflict", 9),
                ("confirmation_required", 10),
                ("daemon_unavailable", 11),
                ("server", 12),
            ]
        );
    }

    #[test]
    fn names_match_serde() {
        for kind in ErrorKind::ALL {
            let json = serde_json::to_value(kind).unwrap();
            assert_eq!(json, kind.as_str());
        }
    }
}
