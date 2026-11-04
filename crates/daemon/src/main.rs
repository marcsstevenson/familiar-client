// SPDX-License-Identifier: MIT OR Apache-2.0
//! `famd`, the sync daemon and the only writer to the store (WP 1.7).

fn main() {
    println!(
        "famd {} ({})",
        env!("CARGO_PKG_VERSION"),
        familiar_core::NAME
    );
}
