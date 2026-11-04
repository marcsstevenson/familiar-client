//! WP 0.4 Tauri prototype backend. Rows are generated in Rust and sent to the
//! web UI a page at a time; message HTML is sanitised in Rust before the UI
//! puts it in a sandboxed iframe.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use bakeoff_common::{self as common, Bench, ROW_COUNT, now_ns};
use serde_json::{Value, json};

#[tauri::command]
fn info(bench: tauri::State<'_, Bench>) -> Value {
    json!({ "total": ROW_COUNT, "folders": common::FOLDERS, "bench": bench.enabled() })
}

/// Compact rows: [from, subject, preview, date, flags] with flags bit 0 = unread,
/// bit 1 = flagged, bit 2 = attachment.
#[tauri::command]
fn rows(start: usize, count: usize) -> Vec<(String, String, String, String, u8)> {
    (start..(start + count).min(ROW_COUNT))
        .map(|i| {
            let r = common::row(i);
            let flags = r.unread as u8 | (r.flagged as u8) << 1 | (r.attachment as u8) << 2;
            (r.from, r.subject, r.preview, r.date, flags)
        })
        .collect()
}

#[tauri::command]
fn message_html(ix: usize) -> (String, f64) {
    let t = now_ns();
    let html = common::sanitize(&common::newsletter_html(ix));
    (html, (now_ns() - t) as f64 / 1e6)
}

#[tauri::command]
fn bench_event(event: String, extra: Value, bench: tauri::State<'_, Bench>) {
    bench.event(&event, extra);
}

fn main() {
    let t_main = now_ns();
    let bench = Bench::from_env();
    bench.event("main", json!({ "main_ns": t_main.to_string() }));
    tauri::Builder::default()
        .manage(bench)
        .invoke_handler(tauri::generate_handler![
            info,
            rows,
            message_html,
            bench_event
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
