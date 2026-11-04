// SPDX-License-Identifier: MIT OR Apache-2.0
//! The parity check: every registered command must appear on every surface,
//! and no surface may expose a command the registry doesn't have.

use crate::registry::Registry;
use std::collections::BTreeSet;
use std::fmt;

/// A place where commands are exposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Surface {
    /// `fam` subcommands.
    Cli,
    /// `fam schema --json`.
    JsonSchema,
    /// The MCP tool list.
    Mcp,
    /// The checked-in TypeScript bindings.
    TypeScript,
}

impl Surface {
    /// Every surface the parity test must check.
    pub const ALL: &'static [Surface] = &[
        Surface::Cli,
        Surface::JsonSchema,
        Surface::Mcp,
        Surface::TypeScript,
    ];
}

impl fmt::Display for Surface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Surface::Cli => "CLI",
            Surface::JsonSchema => "JSON schema",
            Surface::Mcp => "MCP tools",
            Surface::TypeScript => "TypeScript bindings",
        })
    }
}

/// One parity failure.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mismatch {
    /// A registered command is missing from a surface.
    Missing {
        /// Where it is missing.
        surface: Surface,
        /// Dotted command name.
        command: String,
    },
    /// A surface exposes a command that isn't registered (stale output).
    Unknown {
        /// Where it was found.
        surface: Surface,
        /// Dotted command name.
        command: String,
    },
    /// A surface was not checked at all.
    NotChecked {
        /// The surface.
        surface: Surface,
    },
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mismatch::Missing { surface, command } => {
                write!(f, "`{command}` is missing from the {surface}")
            }
            Mismatch::Unknown { surface, command } => {
                write!(
                    f,
                    "the {surface} expose `{command}`, which is not a registered command"
                )
            }
            Mismatch::NotChecked { surface } => write!(f, "the {surface} were not checked"),
        }
    }
}

/// Compares each surface's command names (dotted) with the registry.
pub fn check(
    registry: &Registry,
    surfaces: &[(Surface, BTreeSet<String>)],
) -> Result<(), Vec<Mismatch>> {
    let expected = registry.names();
    let mut problems = Vec::new();
    for surface in Surface::ALL {
        let Some((_, found)) = surfaces.iter().find(|(s, _)| s == surface) else {
            problems.push(Mismatch::NotChecked { surface: *surface });
            continue;
        };
        for command in expected.difference(found) {
            problems.push(Mismatch::Missing {
                surface: *surface,
                command: command.clone(),
            });
        }
        for command in found.difference(&expected) {
            problems.push(Mismatch::Unknown {
                surface: *surface,
                command: command.clone(),
            });
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}
