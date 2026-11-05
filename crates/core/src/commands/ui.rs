// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam ui`: drive the running desktop app, so its actions are commands too.
//!
//! The app listens on its own local socket ([`crate::daemon::ui_socket_for`])
//! with the same line protocol as famd. Each command here forwards its input
//! to the app as the method of the same name.

use crate::daemon::{Client, DaemonPaths, ui_socket_for};
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Method names on the app's socket.
pub const METHOD_UI_OPEN: &str = "ui.open";
/// See [`METHOD_UI_OPEN`].
pub const METHOD_UI_FOLDER: &str = "ui.folder";

/// What the app did.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UiDone {
    /// What the app now shows, in words.
    pub shown: String,
}

const ERRORS: &[ErrorKind] = &[ErrorKind::DaemonUnavailable, ErrorKind::NotFound];

/// Sends `method` with `params` to the running app.
fn send<P: Serialize>(method: &str, params: &P) -> Result<UiDone, CommandError> {
    let paths = DaemonPaths::resolve()?;
    let socket = ui_socket_for(&paths.data_dir);
    let mut client = Client::connect(&socket).map_err(|e| {
        if e.kind == ErrorKind::DaemonUnavailable {
            CommandError::new(
                ErrorKind::DaemonUnavailable,
                "the Familiar app isn't running; start it with `familiar`",
            )
        } else {
            e
        }
    })?;
    client.call(method, params)
}

/// `fam ui open`.
pub struct UiOpen;

/// Input for `fam ui open`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiOpenInput {
    /// Local ID of the message to open.
    #[schemars(extend("x-positional" = 0))]
    pub message: i64,
}

impl Command for UiOpen {
    type Input = UiOpenInput;
    type Output = UiDone;
    const SPEC: CommandSpec = CommandSpec {
        path: &["ui", "open"],
        summary: "Open a message in the app",
        description: "Shows the message in the running desktop app: its folder in the list and the message in the reading pane, and brings the window to the front.",
        access: Access::Read,
        errors: ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, input: UiOpenInput) -> Result<UiDone, CommandError> {
        send(METHOD_UI_OPEN, &input)
    }

    fn render_text(&self, o: &UiDone) -> String {
        o.shown.clone()
    }
}

/// `fam ui folder`.
pub struct UiFolder;

/// Input for `fam ui folder`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiFolderInput {
    /// Folder path (Inbox/Club) or well-known name (inbox, sentitems, drafts, archive). Default: every account's Inbox together.
    #[schemars(extend("x-positional" = 0))]
    #[serde(default)]
    pub folder: Option<String>,
    /// The account the folder is in (its name in config.toml). Needed when a folder is given and there is more than one account.
    #[serde(default)]
    pub account: Option<String>,
}

impl Command for UiFolder {
    type Input = UiFolderInput;
    type Output = UiDone;
    const SPEC: CommandSpec = CommandSpec {
        path: &["ui", "folder"],
        summary: "Show a folder in the app",
        description: "Shows a folder's messages in the running desktop app and brings the window to the front. With no folder, shows every account's Inbox together.",
        access: Access::Read,
        errors: ERRORS,
    };

    fn run(&self, _ctx: &Context<'_>, input: UiFolderInput) -> Result<UiDone, CommandError> {
        send(METHOD_UI_FOLDER, &input)
    }

    fn render_text(&self, o: &UiDone) -> String {
        o.shown.clone()
    }
}
