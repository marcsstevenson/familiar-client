// SPDX-License-Identifier: MIT OR Apache-2.0
//! Microsoft Graph provider: folders, messages, delta sync, calendars and the throttling scheduler (WP 1.6).

/// The crate name, used by the workspace smoke tests.
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn name_is_set() {
        assert_eq!(super::NAME, "familiar-provider-graph");
    }
}
