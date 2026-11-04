// SPDX-License-Identifier: MIT OR Apache-2.0
//! The MCP tool list (`tools/list`), generated from the registry. The server
//! itself (`fam mcp serve`) is WP 2.7.

use crate::registry::{Access, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The result of MCP `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpToolList {
    /// One tool per command.
    pub tools: Vec<McpTool>,
}

/// One MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    /// Tool name, the command path joined with `_`.
    pub name: String,
    /// Short title.
    pub title: String,
    /// What the tool does.
    pub description: String,
    /// JSON schema of the arguments.
    pub input_schema: Value,
    /// JSON schema of the structured result, when it is an object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// Behaviour hints for the client.
    pub annotations: McpAnnotations,
}

/// MCP tool annotations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpAnnotations {
    /// The tool changes nothing.
    pub read_only_hint: bool,
    /// The tool may delete or send.
    pub destructive_hint: bool,
}

/// Builds the tool list for every command in the registry.
pub fn tool_list(registry: &Registry) -> McpToolList {
    let tools = registry
        .commands()
        .map(|c| {
            let spec = c.spec();
            let output = c.output_schema().to_value();
            let is_object = output.get("type").and_then(Value::as_str) == Some("object");
            let description = if spec.description.is_empty() {
                spec.summary.to_string()
            } else {
                spec.description.to_string()
            };
            McpTool {
                name: spec.mcp_name(),
                title: spec.summary.to_string(),
                description,
                input_schema: c.input_schema().to_value(),
                output_schema: is_object.then_some(output),
                annotations: McpAnnotations {
                    read_only_hint: spec.access.is_read_only(),
                    destructive_hint: matches!(
                        spec.access,
                        Access::Send | Access::Delete | Access::Rules
                    ),
                },
            }
        })
        .collect();
    McpToolList { tools }
}
