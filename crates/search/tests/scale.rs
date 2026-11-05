// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 1.9 done criterion: the WP 0.5 search targets, measured on the real
//! store, indexer and searcher with the WP 0.5 synthetic mailbox.
//!
//! The mailbox comes from `tools/synth` (same seed, same distribution as
//! WP 0.5), loaded through the store's writer with bodies for a one-year
//! window, as the daemon would keep them, then indexed by draining the
//! search queue. Each case runs `Searcher::search` for the 50 newest hits,
//! rows read from SQLite, as `fam mail search` does.
//!
//! CI runs 10,000 messages in a debug build with a loose limit, to catch a
//! regression in kind. The real measurement is 500,000 in release:
//!
//! ```sh
//! cargo test --release -p familiar-search --test scale -- --ignored --nocapture
//! ```
//!
//! `FAMILIAR_SCALE_MESSAGES` and `FAMILIAR_SCALE_ITERS` change the size.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use familiar_search::{Indexer, Query, Searcher};
use familiar_store::*;
use synth::{Generator, HISTORY_END, HISTORY_START, Rng};

const SEED: u64 = 42;
const WINDOW: i64 = 365 * 86_400;

fn env_or(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

struct Loaded {
    _dir: tempfile::TempDir,
    store: std::path::PathBuf,
    index: std::path::PathBuf,
    vocab: Vec<String>,
    surnames: Vec<String>,
    largest: (String, String),
    load: Duration,
    index_time: Duration,
}

fn load(n: usize) -> Loaded {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    let index = dir.path().join("search");
    let mut w = Writer::open(&store).unwrap();
    let gen_ = Generator::new(SEED, n);
    let vocab = gen_.vocab().to_vec();
    let surnames = gen_
        .contacts()
        .iter()
        .map(|c| c.name.split_whitespace().last().unwrap().to_lowercase())
        .collect();

    let mut accounts = HashMap::new();
    let mut keys = HashMap::new();
    for a in gen_.accounts() {
        let key = a.name.to_lowercase().replace(' ', "-");
        let id = w
            .write(|tx| {
                tx.upsert_account(&NewAccount {
                    key: key.clone(),
                    email: a.email.clone(),
                    ..Default::default()
                })
            })
            .unwrap();
        accounts.insert(a.id, id);
        keys.insert(a.id, key);
    }
    let mut folders = HashMap::new();
    let mut folder_info = HashMap::new();
    for f in gen_.folders() {
        let id = w
            .write(|tx| {
                tx.upsert_folder(&NewFolder {
                    account: accounts[&f.account_id],
                    remote_id: format!("folder-{}", f.id),
                    parent: f.parent_id.map(|p| folders[&p]),
                    name: f.name.clone(),
                    well_known: f.well_known.clone(),
                })
            })
            .unwrap();
        folders.insert(f.id, id);
        folder_info.insert(f.id, (f.name.clone(), keys[&f.account_id].clone()));
    }

    let t = Instant::now();
    let window_start = HISTORY_END - WINDOW;
    let mut per_folder: HashMap<u32, usize> = HashMap::new();
    let mut batch = Vec::with_capacity(2000);
    let flush = |w: &mut Writer, batch: &mut Vec<synth::Message>| {
        w.write(|tx| {
            for m in batch.iter() {
                let body = if m.received_at >= window_start {
                    let (bytes, is_html) = match &m.body_html {
                        Some(h) => (h.as_bytes(), true),
                        None => (m.body_text.as_bytes(), false),
                    };
                    Some(BodyRef {
                        hash: tx.put_body(bytes)?,
                        is_html,
                    })
                } else {
                    None
                };
                let addr = |a: &synth::Address| Address {
                    name: a.name.clone(),
                    addr: a.addr.clone(),
                };
                tx.upsert_message(&NewMessage {
                    account: accounts[&m.account_id],
                    folder: folders[&m.folder_id],
                    remote_id: m.graph_id.clone(),
                    internet_message_id: Some(m.internet_message_id.clone()),
                    conversation_id: m.conversation_id.clone(),
                    subject: m.subject.clone(),
                    from: addr(&m.from),
                    to: m.to.iter().map(addr).collect(),
                    cc: m.cc.iter().map(addr).collect(),
                    received_at: m.received_at,
                    sent_at: None,
                    is_read: m.is_read,
                    is_flagged: m.is_flagged,
                    importance: i64::from(m.importance),
                    has_attachments: m.has_attachments(),
                    preview: m.preview.clone(),
                    body,
                })?;
            }
            Ok(())
        })
        .unwrap();
        batch.clear();
    };
    for m in gen_ {
        *per_folder.entry(m.folder_id).or_default() += 1;
        batch.push(m);
        if batch.len() == 2000 {
            flush(&mut w, &mut batch);
        }
    }
    flush(&mut w, &mut batch);
    let load = t.elapsed();

    let t = Instant::now();
    let (mut ix, _) = Indexer::open_for(&index, &mut w).unwrap();
    let indexed = ix.drain(&mut w).unwrap();
    assert_eq!(indexed, n);
    let index_time = t.elapsed();

    let largest = per_folder
        .iter()
        .max_by_key(|(_, c)| **c)
        .map(|(f, _)| folder_info[f].clone())
        .unwrap();
    Loaded {
        _dir: dir,
        store,
        index,
        vocab,
        surnames,
        largest,
        load,
        index_time,
    }
}

struct Case {
    name: &'static str,
    p50: f64,
    p95: f64,
    max: f64,
    hits: f64,
}

fn time(iters: usize, mut f: impl FnMut(&mut Rng) -> usize) -> Case {
    let mut rng = Rng::new(7);
    for _ in 0..(iters / 10).max(3) {
        f(&mut rng);
    }
    let mut ms = Vec::with_capacity(iters);
    let mut hits = 0;
    for _ in 0..iters {
        let t = Instant::now();
        hits += f(&mut rng);
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((ms.len() as f64 - 1.0) * q).round() as usize];
    Case {
        name: "",
        p50: at(0.50),
        p95: at(0.95),
        max: ms[ms.len() - 1],
        hits: hits as f64 / iters as f64,
    }
}

fn run(n: usize, iters: usize) -> Vec<Case> {
    let l = load(n);
    let r = Reader::open(&l.store).unwrap();
    let s = Searcher::open(&l.index).unwrap();
    let docs = s.num_docs();
    let word = |rng: &mut Rng, lo: usize, hi: usize| {
        l.vocab[rng.range(lo as u64, hi.min(l.vocab.len() - 1) as u64) as usize].clone()
    };
    let surname = |rng: &mut Rng| {
        let i = (rng.f64().powi(3) * l.surnames.len() as f64) as usize;
        l.surnames[i.min(l.surnames.len() - 1)].clone()
    };
    let (big_folder, big_account) = l.largest.clone();
    type Gen<'a> = Box<dyn Fn(&mut Rng) -> String + 'a>;
    let cases: Vec<(&'static str, Gen)> = vec![
        ("common word (ranks 0-50)", Box::new(|r| word(r, 0, 50))),
        (
            "mid word (ranks 500-3000)",
            Box::new(|r| word(r, 500, 3000)),
        ),
        (
            "rare word (ranks 10k-39k)",
            Box::new(|r| word(r, 10_000, 39_999)),
        ),
        (
            "two words (AND)",
            Box::new(|r| format!("{} {}", word(r, 0, 50), word(r, 500, 3000))),
        ),
        (
            "phrase of two very common words",
            Box::new(|r| format!("\"{} {}\"", word(r, 0, 20), word(r, 0, 20))),
        ),
        (
            "from: surname",
            Box::new(|r| format!("from:{}", surname(r))),
        ),
        (
            "subject: word",
            Box::new(|r| format!("subject:{}", word(r, 200, 3000))),
        ),
        (
            "word + folder: (largest folder)",
            Box::new(|r| {
                format!(
                    "{} folder:\"{big_folder}\" account:{big_account}",
                    word(r, 500, 3000)
                )
            }),
        ),
        (
            "word + has:attachment",
            Box::new(|r| format!("{} has:attachment", word(r, 0, 50))),
        ),
        (
            "word + date range (one year)",
            Box::new(|r| {
                let y = 2006 + r.range(0, 19);
                format!("{} after:{y}-01-01 before:{}-01-01", word(r, 0, 50), y + 1)
            }),
        ),
        (
            "word + is:unread",
            Box::new(|r| format!("{} is:unread", word(r, 0, 50))),
        ),
        (
            "filters only: folder: is:unread",
            Box::new(|_| format!("folder:\"{big_folder}\" account:{big_account} is:unread")),
        ),
    ];
    let mut out = Vec::new();
    for (name, q) in &cases {
        let mut c = time(iters, |rng| {
            let q = Query::parse(&q(rng), HISTORY_END).unwrap();
            s.search(&r, &q, None, 50).unwrap().hits.len()
        });
        c.name = name;
        out.push(c);
    }
    let mut c = time(iters, |rng| {
        let r = Reader::open(&l.store).unwrap();
        let s = Searcher::open(&l.index).unwrap();
        let q = Query::parse(&word(rng, 0, 50), HISTORY_END).unwrap();
        s.search(&r, &q, None, 50).unwrap().hits.len()
    });
    c.name = "open store + index, then common word";
    out.push(c);

    let size = |p: &std::path::Path| -> u64 {
        fn walk(p: &std::path::Path) -> u64 {
            std::fs::read_dir(p)
                .map(|d| {
                    d.flatten()
                        .map(|e| {
                            let m = e.metadata().unwrap();
                            if m.is_dir() { walk(&e.path()) } else { m.len() }
                        })
                        .sum()
                })
                .unwrap_or(0)
        }
        walk(p)
    };
    println!(
        "\n{n} messages ({docs} indexed; bodies for the last year, {} of history), seed {SEED}, {iters} runs per case",
        (HISTORY_END - HISTORY_START) / (365 * 86_400)
    );
    println!(
        "load through the writer {:.1} s; index from the queue {:.1} s; index {:.0} MB",
        l.load.as_secs_f64(),
        l.index_time.as_secs_f64(),
        size(&l.index) as f64 / 1e6
    );
    println!("| Case | p50 ms | p95 ms | max ms | hits |\n| --- | --- | --- | --- | --- |");
    for c in &out {
        println!(
            "| {} | {:.2} | {:.2} | {:.1} | {:.0} |",
            c.name, c.p50, c.p95, c.max, c.hits
        );
    }
    out
}

/// The design doc's target: search results, first page, under 100 ms at p95.
const TARGET_MS: f64 = 100.0;

#[test]
fn search_targets_small() {
    // A debug build is several times slower than release; this guards
    // against a change of kind (a scan instead of an index lookup), not
    // the exact target.
    let cases = run(
        env_or("FAMILIAR_SCALE_MESSAGES", 10_000),
        env_or("FAMILIAR_SCALE_ITERS", 30),
    );
    for c in cases {
        assert!(c.p95 < TARGET_MS * 5.0, "{}: p95 {:.1} ms", c.name, c.p95);
    }
}

#[test]
#[ignore = "500k messages; run in release, see the module docs"]
fn search_targets_500k() {
    let cases = run(
        env_or("FAMILIAR_SCALE_MESSAGES", 500_000),
        env_or("FAMILIAR_SCALE_ITERS", 1000),
    );
    for c in cases {
        assert!(c.p95 < TARGET_MS, "{}: p95 {:.1} ms", c.name, c.p95);
    }
}
