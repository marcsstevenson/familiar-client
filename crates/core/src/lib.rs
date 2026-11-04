// SPDX-License-Identifier: MIT OR Apache-2.0
//! Command definitions and shared types. Every surface (CLI, MCP, GUI, local API) is generated from the registry here (WP 1.2).

/// The crate name, used by the workspace smoke tests.
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn name_is_set() {
        assert_eq!(super::NAME, "familiar-core");
    }
}
