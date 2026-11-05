// SPDX-License-Identifier: MIT OR Apache-2.0
//! The typed command registry.
//!
//! Every user action is a [`Command`]: a Rust type with typed input, output
//! and a fixed set of error kinds. The CLI arguments, `fam schema --json`, the
//! MCP tool list and the TypeScript bindings are all generated from the
//! registry, and the parity test fails CI if any surface misses a command.

use crate::error::{CommandError, ErrorKind};
use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// What a command may do, used for agent permissions (`fam mcp serve --allow read,draft`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Reads local or server state only.
    Read,
    /// Creates or edits drafts.
    Draft,
    /// Changes mail or calendar state (mark read, move, create event).
    Write,
    /// Sends mail or forwards it.
    Send,
    /// Deletes mail, folders or events.
    Delete,
    /// Changes server-side or local rules.
    Rules,
}

impl Access {
    /// True for commands that change nothing.
    pub fn is_read_only(self) -> bool {
        self == Access::Read
    }
}

/// The static description of a command.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    /// The command path, for example `["mail", "list"]` for `fam mail list`.
    pub path: &'static [&'static str],
    /// One line for help and tool lists.
    pub summary: &'static str,
    /// Longer help; may be empty.
    pub description: &'static str,
    /// What the command may do.
    pub access: Access,
    /// The error kinds this command can return, besides `usage` and `internal`,
    /// which every command can return.
    pub errors: &'static [ErrorKind],
}

impl CommandSpec {
    /// The dotted name, for example `mail.list`. Used in the schema and TS bindings.
    pub fn name(&self) -> String {
        self.path.join(".")
    }

    /// The MCP tool name, for example `mail_list`.
    pub fn mcp_name(&self) -> String {
        self.path.join("_")
    }

    /// Every error kind the command can return, including `usage` and `internal`.
    pub fn all_errors(&self) -> Vec<ErrorKind> {
        let mut set: BTreeSet<ErrorKind> = self.errors.iter().copied().collect();
        set.insert(ErrorKind::Usage);
        set.insert(ErrorKind::Internal);
        set.into_iter().collect()
    }
}

/// What a command can see while it runs. Grows with the engine (store, daemon client, config).
pub struct Context<'a> {
    /// The registry the command was dispatched from.
    pub registry: &'a Registry,
    /// Shows a line to the person while the command runs, such as a sign-in code. The CLI
    /// prints it to stderr so `--json` output stays clean.
    pub notify: &'a dyn Fn(&str),
}

/// A command definition. Implement this, then add the type to [`Registry::builtin`].
pub trait Command: Send + Sync + 'static {
    /// The input. Fields become CLI flags, MCP input properties and TS fields.
    type Input: DeserializeOwned + JsonSchema;
    /// The output, as printed by `--json`.
    type Output: Serialize + JsonSchema;
    /// The static description.
    const SPEC: CommandSpec;

    /// Runs the command.
    fn run(&self, ctx: &Context<'_>, input: Self::Input) -> Result<Self::Output, CommandError>;

    /// Human-readable output for when `--json` is not given.
    fn render_text(&self, output: &Self::Output) -> String {
        serde_json::to_string_pretty(output).unwrap_or_default()
    }
}

/// A command's result: the JSON for `--json` and the text for people.
#[derive(Debug, Clone)]
pub struct Output {
    /// The output as JSON, matching the command's output schema.
    pub json: Value,
    /// The output rendered for people.
    pub text: String,
}

/// Object-safe view of a [`Command`], so the registry can hold all of them.
pub trait DynCommand: Send + Sync {
    /// The static description.
    fn spec(&self) -> &CommandSpec;
    /// The JSON schema of the input (draft 2020-12, self-contained).
    fn input_schema(&self) -> Schema;
    /// The JSON schema of the output (draft 2020-12, self-contained).
    fn output_schema(&self) -> Schema;
    /// Runs the command from JSON input and returns the JSON output and its text rendering.
    fn run_json(&self, ctx: &Context<'_>, input: Value) -> Result<Output, CommandError>;
}

/// Generates a self-contained schema for `T`.
pub fn schema_for<T: JsonSchema>() -> Schema {
    SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<T>()
}

impl<C: Command> DynCommand for C {
    fn spec(&self) -> &CommandSpec {
        &C::SPEC
    }

    fn input_schema(&self) -> Schema {
        schema_for::<C::Input>()
    }

    fn output_schema(&self) -> Schema {
        schema_for::<C::Output>()
    }

    fn run_json(&self, ctx: &Context<'_>, input: Value) -> Result<Output, CommandError> {
        let input: C::Input = serde_json::from_value(input)
            .map_err(|e| CommandError::usage(format!("invalid input: {e}")))?;
        let output = self.run(ctx, input)?;
        let text = self.render_text(&output);
        let json = serde_json::to_value(output)
            .map_err(|e| CommandError::internal(format!("could not encode output: {e}")))?;
        Ok(Output { json, text })
    }
}

/// All registered commands, sorted by name.
pub struct Registry {
    commands: Vec<Box<dyn DynCommand>>,
}

impl Registry {
    /// An empty registry, for tests.
    pub fn empty() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// The registry every surface is generated from. Add new commands here.
    pub fn builtin() -> Self {
        let mut r = Self::empty();
        r.register(crate::commands::version::Version);
        r.register(crate::commands::schema::SchemaCmd);
        r.register(crate::commands::config::ConfigValidate);
        r.register(crate::commands::config::ConfigPath);
        r.register(crate::commands::auth::AuthLogin);
        r.register(crate::commands::auth::AuthStatus);
        r.register(crate::commands::auth::AuthLogout);
        r.register(crate::commands::sync::SyncStatus);
        r.register(crate::commands::sync::SyncCheck);
        r.register(crate::commands::mail::MailPin);
        r.register(crate::commands::mail::MailList);
        r.register(crate::commands::mail::MailThread);
        r.register(crate::commands::mail::MailFetch);
        r.register(crate::commands::folder::FolderList);
        r.register(crate::commands::api::Api);
        r.register(crate::commands::search::MailSearch);
        r.register(crate::commands::cal::CalList);
        r.register(crate::commands::cal::CalCalendars);
        r.register(crate::commands::write::MailRead);
        r.register(crate::commands::write::MailFlag);
        r.register(crate::commands::write::MailMove);
        r.register(crate::commands::write::MailArchive);
        r.register(crate::commands::write::MailDelete);
        r.register(crate::commands::write::FolderCreate);
        r.register(crate::commands::write::FolderRename);
        r.register(crate::commands::write::FolderMove);
        r.register(crate::commands::write::FolderDelete);
        r.register(crate::commands::write::OpsList);
        r.register(crate::commands::write::OpsRetry);
        r.register(crate::commands::write::OpsCancel);
        r.register(crate::commands::draft::DraftCreate);
        r.register(crate::commands::draft::DraftReply);
        r.register(crate::commands::draft::DraftForward);
        r.register(crate::commands::draft::DraftUpdate);
        r.register(crate::commands::draft::DraftSend);
        r.register(crate::commands::draft::MailSend);
        r
    }

    /// Adds a command. Panics if the name is taken or one path is a prefix of
    /// another (a command can't also be a command group).
    pub fn register<C: Command>(&mut self, command: C) {
        let new = &C::SPEC;
        assert!(!new.path.is_empty(), "command path must not be empty");
        for existing in &self.commands {
            let old = existing.spec();
            let n = old.path.len().min(new.path.len());
            assert!(
                old.path[..n] != new.path[..n],
                "command `{}` clashes with `{}`",
                new.name(),
                old.name()
            );
            assert!(
                old.mcp_name() != new.mcp_name(),
                "command `{}` has the same MCP tool name as `{}`",
                new.name(),
                old.name()
            );
        }
        self.commands.push(Box::new(command));
        self.commands.sort_by_key(|c| c.spec().name());
    }

    /// Commands in name order.
    pub fn commands(&self) -> impl Iterator<Item = &dyn DynCommand> {
        self.commands.iter().map(|c| c.as_ref())
    }

    /// Dotted names in order.
    pub fn names(&self) -> BTreeSet<String> {
        self.commands().map(|c| c.spec().name()).collect()
    }

    /// Finds a command by dotted name.
    pub fn get(&self, name: &str) -> Option<&dyn DynCommand> {
        self.commands().find(|c| c.spec().name() == name)
    }

    /// Runs a command by dotted name with JSON input. Progress lines are dropped.
    pub fn run(&self, name: &str, input: Value) -> Result<Output, CommandError> {
        self.run_with(name, input, &|_| {})
    }

    /// Runs a command, passing progress lines (see [`Context::notify`]) to `notify`.
    pub fn run_with(
        &self,
        name: &str,
        input: Value,
        notify: &dyn Fn(&str),
    ) -> Result<Output, CommandError> {
        let cmd = self
            .get(name)
            .ok_or_else(|| CommandError::usage(format!("unknown command `{name}`")))?;
        cmd.run_json(
            &Context {
                registry: self,
                notify,
            },
            input,
        )
    }
}
