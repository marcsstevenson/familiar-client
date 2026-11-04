// SPDX-License-Identifier: MIT OR Apache-2.0
//! Familiar desktop app. A stub until Phase 3 adds Tauri 2 + SolidJS; it is the only crate allowed to depend on a GUI or webview library.

fn main() {
    println!(
        "familiar {} ({})",
        env!("CARGO_PKG_VERSION"),
        familiar_core::NAME
    );
}
