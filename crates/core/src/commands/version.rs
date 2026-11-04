// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam version`: the sample command that proves every surface is generated.

use crate::error::CommandError;
use crate::registry::{Access, Command, CommandSpec, Context};
use crate::surfaces::SCHEMA_VERSION;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `fam version`.
pub struct Version;

/// Input for `fam version`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VersionInput {
    /// Also report the operating system and CPU architecture.
    #[serde(default)]
    pub verbose: bool,
}

/// Output of `fam version`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct VersionOutput {
    /// The Familiar version.
    pub version: String,
    /// The version of the command schema (`fam schema --json`).
    pub schema_version: u32,
    /// The operating system, with `--verbose`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// The CPU architecture, with `--verbose`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

impl Command for Version {
    type Input = VersionInput;
    type Output = VersionOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["version"],
        summary: "Show the Familiar version",
        description: "Shows the Familiar version and the command schema version.",
        access: Access::Read,
        errors: &[],
    };

    fn run(&self, _ctx: &Context<'_>, input: VersionInput) -> Result<VersionOutput, CommandError> {
        Ok(VersionOutput {
            version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: SCHEMA_VERSION,
            os: input.verbose.then(|| std::env::consts::OS.to_string()),
            arch: input.verbose.then(|| std::env::consts::ARCH.to_string()),
        })
    }

    fn render_text(&self, o: &VersionOutput) -> String {
        let mut s = format!("familiar {} (schema {})", o.version, o.schema_version);
        if let (Some(os), Some(arch)) = (&o.os, &o.arch) {
            s.push_str(&format!(" {os}/{arch}"));
        }
        s
    }
}
