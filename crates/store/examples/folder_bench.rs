// SPDX-License-Identifier: MIT OR Apache-2.0
//! Times the folder reads and writes the desktop app and `fam` lean on,
//! against a store filled by the `seed` example. Prints one JSON object.
//!
//! ```sh
//! cargo run --release -p familiar-store --example seed -- /tmp/bench/store 500000
//! cargo run --release -p familiar-store --example folder_bench -- /tmp/bench/store
//! ```
//!
//! The moves it times are undone as it goes; the sampled messages are left
//! unread.

use familiar_store::*;
use std::time::Instant;

/// Runs `f` `n` times and returns (p50, p95, max) in milliseconds.
fn time(n: usize, mut f: impl FnMut(usize)) -> serde_json::Value {
    let mut ms: Vec<f64> = (0..n)
        .map(|i| {
            let t = Instant::now();
            f(i);
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    serde_json::json!({
        "p50": (at(0.5) * 100.0).round() / 100.0,
        "p95": (at(0.95) * 100.0).round() / 100.0,
        "max": (at(1.0) * 100.0).round() / 100.0,
    })
}

fn main() {
    let Some(root) = std::env::args().nth(1) else {
        eprintln!("usage: folder_bench <store dir>");
        std::process::exit(2);
    };
    let root = std::path::Path::new(&root);
    // Opening the writer runs any pending migration; its time is the
    // upgrade cost on a store this size.
    let t = Instant::now();
    drop(Writer::open(root).expect("open writer"));
    let open_writer_ms = (t.elapsed().as_secs_f64() * 1000.0).round();
    let r = Reader::open(root).expect("open store");
    let accounts = r.accounts().expect("accounts");
    let folders: Vec<Folder> = accounts
        .iter()
        .flat_map(|a| r.folders(a.id).expect("folders"))
        .collect();
    let biggest = folders.iter().max_by_key(|f| f.total).expect("a folder");
    let inboxes: Vec<FolderId> = folders
        .iter()
        .filter(|f| f.well_known.as_deref() == Some("inbox"))
        .map(|f| f.id)
        .collect();
    let messages: i64 = folders.iter().map(|f| f.total).sum();
    let mut out = serde_json::Map::new();
    out.insert("open_writer_ms".into(), open_writer_ms.into());
    out.insert("folder_rows".into(), messages.into());
    out.insert("biggest_folder".into(), biggest.total.into());

    // Unread counts for the folder pane: every account's folder list.
    out.insert(
        "folder_counts_ms".into(),
        time(200, |_| {
            for a in &accounts {
                r.folders(a.id).unwrap();
            }
        }),
    );
    // Opening a folder: the first page, flat (`fam mail list`) and threaded.
    out.insert(
        "first_page_ms".into(),
        time(200, |i| {
            r.messages(folders[i % folders.len()].id, None, 50).unwrap();
        }),
    );
    out.insert(
        "first_page_unread_ms".into(),
        time(200, |i| {
            let f = MessageFilter {
                unread_only: true,
                ..Default::default()
            };
            r.messages_filtered(folders[i % folders.len()].id, f, None, 50)
                .unwrap();
        }),
    );
    out.insert(
        "first_page_threaded_ms".into(),
        time(200, |i| {
            r.conversations(folders[i % folders.len()].id, None, 50)
                .unwrap();
        }),
    );
    // Scrolling: the next page from a cursor deep in the biggest folder.
    let order = r.message_order(&[biggest.id], false).unwrap();
    out.insert(
        "scroll_page_ms".into(),
        time(200, |i| {
            let (id, at) = order[(i * 997) % order.len()];
            let after = Cursor {
                received_at: at,
                id: id.0,
            };
            r.messages(biggest.id, Some(after), 100).unwrap();
        }),
    );
    // The GUI's virtualised list orders the whole folder when it opens.
    out.insert(
        "order_biggest_ms".into(),
        time(20, |_| {
            r.message_order(&[biggest.id], false).unwrap();
        }),
    );
    out.insert(
        "order_unified_inbox_ms".into(),
        time(20, |_| {
            r.message_order(&inboxes, false).unwrap();
        }),
    );
    out.insert(
        "order_unread_biggest_ms".into(),
        time(20, |_| {
            r.message_order(&[biggest.id], true).unwrap();
        }),
    );
    drop(r);

    // Writes: what one click costs, triggers included.
    let mut w = Writer::open(root).expect("open writer");
    let sample: Vec<MessageId> = order
        .iter()
        .step_by(order.len() / 500 + 1)
        .map(|o| o.0)
        .collect();
    let others: Vec<FolderId> = folders
        .iter()
        .filter(|f| f.account == biggest.account && f.id != biggest.id)
        .map(|f| f.id)
        .collect();
    out.insert(
        "mark_read_ms".into(),
        time(sample.len() * 2, |i| {
            let id = sample[i / 2];
            w.write(|tx| tx.set_read(id, i % 2 == 0)).unwrap();
        }),
    );
    out.insert(
        "move_ms".into(),
        time(sample.len() * 2, |i| {
            let id = sample[i / 2];
            let to = if i % 2 == 0 {
                others[i % others.len()]
            } else {
                biggest.id
            };
            w.write(|tx| tx.move_message(id, to)).unwrap();
        }),
    );
    let t = Instant::now();
    let drift = w.repair_derived().unwrap();
    out.insert(
        "drift_check_ms".into(),
        ((t.elapsed().as_secs_f64() * 1000.0).round()).into(),
    );
    out.insert("drift".into(), drift.into());
    println!("{}", serde_json::Value::Object(out));
}
