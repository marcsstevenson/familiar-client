// SPDX-License-Identifier: MIT OR Apache-2.0
//! SQLite store with WAL, migrations and body files. Only `famd` writes to it (WP 1.5).

/// The crate name, used by the workspace smoke tests.
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn name_is_set() {
        assert_eq!(super::NAME, "familiar-store");
    }
}
