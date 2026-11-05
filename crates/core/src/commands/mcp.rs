// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam mcp serve` (WP 2.7): the commands as MCP tools over stdio.
//!
//! See [`crate::surfaces::mcp::McpServer`]. Register it with an MCP client
//! as the command `fam mcp serve --allow read,draft`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use crate::surfaces::mcp::{McpServer, parse_allow};

/// `fam mcp serve`.
pub struct McpServe;

/// Input for `fam mcp serve`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpServeInput {
    /// Access levels whose commands are offered, comma-separated or repeated:
    /// read, draft, write, send, delete, rules. Default read,draft. famd
    /// still checks each write against [agent] in config.toml.
    #[serde(default)]
    pub allow: Vec<String>,
}

/// Output of `fam mcp serve`, once the client has gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpServeOutput {
    /// Requests answered.
    pub requests: u64,
}

impl Command for McpServe {
    type Input = McpServeInput;
    type Output = McpServeOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["mcp", "serve"],
        summary: "Serve the commands to an MCP client over stdio",
        description: "Speaks MCP (JSON-RPC, one message per line) on stdin and stdout until stdin closes. Each command is a tool named like mail_list; the same server also answers each command by its dotted name (mail.list) as a plain JSON-RPC method. Only commands within --allow are offered (default read,draft); writes are made as source mcp and checked by famd against [agent] in config.toml, and logged in the audit log.",
        access: Access::Read,
        errors: &[ErrorKind::Internal],
    };

    fn run(&self, ctx: &Context<'_>, i: McpServeInput) -> Result<McpServeOutput, CommandError> {
        let mut allow = parse_allow(&i.allow)?;
        if allow.is_empty() {
            allow = vec![Access::Read, Access::Draft];
        }
        let server = McpServer::new(ctx.registry, allow);
        let stdin = std::io::stdin();
        let requests = server
            .serve(stdin.lock(), std::io::stdout())
            .map_err(|e| CommandError::internal(format!("stdio: {e}")))?;
        Ok(McpServeOutput { requests })
    }

    fn render_text(&self, _o: &McpServeOutput) -> String {
        String::new()
    }
}
