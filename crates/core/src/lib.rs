// SPDX-License-Identifier: MIT OR Apache-2.0
//! Command definitions and shared types. Every surface (CLI, MCP, GUI, local API) is generated from the registry here.

pub mod agent;
pub mod ai;
pub mod commands;
pub mod compose;
pub mod daemon;
pub mod error;
pub mod events;
pub mod folder_prefs;
pub mod ics;
pub mod markdown;
pub mod ops;
pub mod parity;
pub mod registry;
pub mod spam;
pub mod surfaces;
pub mod tz;
mod tz_table;

pub use error::{CommandError, ErrorKind};
pub use registry::{Access, Command, CommandSpec, Context, DynCommand, Output, Registry};

/// The crate name, used by the workspace smoke tests.
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn name_is_set() {
        assert_eq!(super::NAME, "familiar-core");
    }

    #[test]
    fn version_runs() {
        let r = Registry::builtin();
        let out = r.run("version", json!({})).unwrap();
        assert_eq!(out.json["version"], env!("CARGO_PKG_VERSION"));
        assert!(out.json.get("os").is_none());
        let out = r.run("version", json!({"verbose": true})).unwrap();
        assert!(out.json["os"].is_string());
    }

    #[test]
    fn unknown_input_is_a_usage_error() {
        let r = Registry::builtin();
        let err = r.run("version", json!({"nope": 1})).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    #[should_panic(expected = "clashes")]
    fn duplicate_names_are_rejected() {
        let mut r = Registry::builtin();
        r.register(commands::version::Version);
    }

    #[test]
    fn mcp_tools_cover_the_registry() {
        let r = Registry::builtin();
        let list = surfaces::mcp::tool_list(&r);
        assert_eq!(list.tools.len(), r.names().len());
        for t in &list.tools {
            assert_eq!(
                t.input_schema["type"], "object",
                "{} input must be an object",
                t.name
            );
        }
    }
}
