// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tauri build step. The web front end is built by npm into `ui/dist`; when it
//! has not been built (a plain `cargo build` or CI's Rust jobs), a placeholder
//! page is written so the crate still compiles.

fn main() {
    let dist = std::path::Path::new("ui/dist");
    if !dist.join("index.html").exists() {
        std::fs::create_dir_all(dist).expect("create ui/dist");
        std::fs::write(
            dist.join("index.html"),
            "<!doctype html><meta charset=utf-8><title>Familiar</title>\
             <p style=\"font:14px system-ui;padding:2em\">The front end is not built. \
             Run <code>npm ci &amp;&amp; npm run build</code> in <code>crates/gui/ui</code>, \
             then build again.</p>",
        )
        .expect("write placeholder index.html");
    }
    tauri_build::build();
}
