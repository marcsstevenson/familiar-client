// SPDX-License-Identifier: MIT OR Apache-2.0
//! Generated surfaces: the schema document, the MCP tool list and the TS bindings.
//! The CLI is generated in `familiar-cli` from the same registry.

pub mod mcp;
pub mod typescript;

pub use mcp::McpToolList;

use crate::error::ErrorKind;
use crate::registry::{Access, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The version of the schema document format. Bump on breaking changes to it.
pub const SCHEMA_VERSION: u32 = 1;

/// `fam schema --json`: everything an agent or generator needs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SchemaDocument {
    /// The format version of this document.
    pub schema_version: u32,
    /// The Familiar version that produced it.
    pub familiar_version: String,
    /// Every command, by name.
    pub commands: Vec<CommandDoc>,
    /// Every exit code `fam` can return.
    pub exit_codes: Vec<ExitCodeDoc>,
}

/// One command in the schema document.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CommandDoc {
    /// Dotted name, for example `mail.list`.
    pub name: String,
    /// The CLI form, for example `fam mail list`.
    pub cli: String,
    /// The MCP tool name, for example `mail_list`.
    pub mcp_tool: String,
    /// One line.
    pub summary: String,
    /// Longer help.
    pub description: String,
    /// What the command may do.
    pub access: Access,
    /// Error kinds the command can return.
    pub errors: Vec<ErrorKind>,
    /// JSON schema of the input.
    pub input: Value,
    /// JSON schema of the `--json` output.
    pub output: Value,
}

/// One exit code.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExitCodeDoc {
    /// The process exit code.
    pub code: u8,
    /// The error kind, or none for success.
    pub kind: Option<ErrorKind>,
    /// What it means.
    pub description: String,
}

/// Builds the schema document for a registry.
pub fn schema_document(registry: &Registry) -> SchemaDocument {
    let commands = registry
        .commands()
        .map(|c| {
            let spec = c.spec();
            CommandDoc {
                name: spec.name(),
                cli: format!("fam {}", spec.path.join(" ")),
                mcp_tool: spec.mcp_name(),
                summary: spec.summary.to_string(),
                description: spec.description.to_string(),
                access: spec.access,
                errors: spec.all_errors(),
                input: c.input_schema().to_value(),
                output: c.output_schema().to_value(),
            }
        })
        .collect();
    let mut exit_codes = vec![ExitCodeDoc {
        code: 0,
        kind: None,
        description: "Success.".into(),
    }];
    exit_codes.extend(ErrorKind::ALL.iter().map(|k| ExitCodeDoc {
        code: k.exit_code(),
        kind: Some(*k),
        description: k.description().into(),
    }));
    SchemaDocument {
        schema_version: SCHEMA_VERSION,
        familiar_version: env!("CARGO_PKG_VERSION").into(),
        commands,
        exit_codes,
    }
}
