// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam mail pin` (WP 1.7): keep a message's body on this device whatever its age.
//!
//! Pinning changes only what this device keeps, never the mailbox, so it is
//! a read-level command. It goes through `famd`, the store's only writer,
//! which downloads the body first if it isn't local.

use crate::daemon::{Client, METHOD_PIN, PinParams, PinResult};
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use schemars::JsonSchema;
use serde::Deserialize;

/// `fam mail pin`.
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
