// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam schema`: describes every command, for agents and code generators.

use crate::error::CommandError;
use crate::registry::{Access, Command, CommandSpec, Context};
use crate::surfaces::{self, McpToolList, SchemaDocument};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `fam schema`.
pub struct SchemaCmd;

/// Which description to produce.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SchemaFormat {
    /// Every command with its input and output schemas, errors and exit codes.
    #[default]
    Commands,
    /// The MCP `tools/list` result.
    Mcp,
    /// TypeScript bindings for the desktop app.
    Typescript,
}

/// Input for `fam schema`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaInput {
    /// What to describe.
    #[serde(default)]
    pub format: SchemaFormat,
}

/// Output of `fam schema`.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum SchemaOutput {
    /// `--format commands` (the default).
    Commands(SchemaDocument),
    /// `--format mcp`.
    Mcp(McpToolList),
    /// `--format typescript`.
    Typescript {
        /// The TypeScript source.
        typescript: String,
    },
}

impl Command for SchemaCmd {
    type Input = SchemaInput;
    type Output = SchemaOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["schema"],
        summary: "Describe every command, its input, output and errors",
        description: "Prints the command schema that the CLI, MCP tools and GUI bindings are generated from. Use --json for machine-readable output.",
        access: Access::Read,
        errors: &[],
    };

    fn run(&self, ctx: &Context<'_>, input: SchemaInput) -> Result<SchemaOutput, CommandError> {
        Ok(match input.format {
            SchemaFormat::Commands => {
                SchemaOutput::Commands(surfaces::schema_document(ctx.registry))
            }
            SchemaFormat::Mcp => SchemaOutput::Mcp(surfaces::mcp::tool_list(ctx.registry)),
            SchemaFormat::Typescript => SchemaOutput::Typescript {
                typescript: surfaces::typescript::bindings(ctx.registry),
            },
        })
    }

    fn render_text(&self, o: &SchemaOutput) -> String {
        match o {
            SchemaOutput::Commands(doc) => {
                let width = doc.commands.iter().map(|c| c.cli.len()).max().unwrap_or(0);
                let mut s = String::new();
                for c in &doc.commands {
                    s.push_str(&format!("{:width$}  {}\n", c.cli, c.summary));
                }
                s.push_str("\nUse --json for the full schema.");
                s
            }
            SchemaOutput::Mcp(list) => serde_json::to_string_pretty(list).unwrap_or_default(),
            SchemaOutput::Typescript { typescript } => typescript.trim_end().to_string(),
        }
    }
}
