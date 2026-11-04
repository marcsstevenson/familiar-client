// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam`, the command line (WP 1.10).

fn main() {
    println!(
        "fam {} ({})",
        env!("CARGO_PKG_VERSION"),
        familiar_core::NAME
    );
}
